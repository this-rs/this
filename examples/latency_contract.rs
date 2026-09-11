//! latency_contract — le contrat de latence de this-rs, **par surface, mesuré**.
//!
//! L'objectif demandait un contrat chiffré : ce que coûte chaque surface, et ce
//! que chaque chiffre **inclut ou exclut**. Les pièces étaient mesurées
//! séparément au fil du programme ; ce banc les réunit en un seul tableau, avec
//! deux familles :
//!
//! - **in-process** (ns–µs) : projection d'identité, contrôle de bail, décision
//!   IAM, journal ;
//! - **HTTP** (µs) : requêtes réelles sur une **socket réelle**, pile complète.
//!
//! ```text
//! cargo run -p this-rs --features obrain --release --example latency_contract
//! ```
//!
//! # Honnêteté des mesures
//!
//! - Les chiffres HTTP passent par un **vrai** `TcpStream` : ils incluent la
//!   sérialisation, la pile HTTP et l'aller-retour noyau. La connexion est
//!   établie **une fois** puis réutilisée — donc le `connect` n'est **pas**
//!   inclus, et c'est dit.
//! - Les chiffres in-process incluent un tour de chauffe, mais la machine est
//!   **partagée** : les valeurs absolues varient avec la charge. Ce qui est
//!   stable, c'est l'**ordre de grandeur** entre surfaces.
//! - Aucune de ces mesures n'est une garantie de production : ce sont des
//!   ordres de grandeur sur un store en mémoire ou local.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthResolverRegistry};
use this::core::identity::IdentityRegistry;
use this::core::link::LinkDefinition;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::middleware::authorization::Authorization;
use this::storage::ObrainStore;
use this::storage::iam::{PendingDecision, RoleGrant};

/// Échantillonne une opération et rend la médiane en secondes.
/// Le coût d'une opération : **minimum** et **médiane**.
///
/// # Pourquoi les deux
///
/// La machine est partagée : sous charge, la médiane d'une même opération
/// mesurée à deux minutes d'intervalle a varié d'un **facteur 3** (6 µs puis
/// 16 µs pour la même lecture indexée). Un contrat qui bouge comme ça n'est pas
/// un contrat. Le **minimum** est le coût de l'opération sans interférence : il
/// tient d'une exécution à l'autre, et c'est lui qu'on peut citer.
#[derive(Clone, Copy)]
struct Stats {
    min: f64,
    median: f64,
}

fn stats(iters: u32, mut op: impl FnMut()) -> Stats {
    op(); // chauffe
    let mut samples = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let start = Instant::now();
        op();
        samples.push(start.elapsed().as_secs_f64());
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Stats {
        min: samples[0],
        median: samples[samples.len() / 2],
    }
}

/// Envoie une requête et lit la réponse **complète**.
///
/// # Pourquoi ce n'est pas un simple `read`
///
/// Un `read` rend dès que des octets sont disponibles : sur une socket locale,
/// c'est l'arrivée des **en-têtes**, pendant que le handler écrit encore le
/// corps. Mesurer là donnerait un chiffre flatteur et **faux** — c'est
/// exactement l'erreur que ce banc a commise avant correction. On lit donc
/// jusqu'au dernier octet du corps, annoncé par `Content-Length`.
///
/// Rend `(octets lus, en-têtes seuls ?)`. Un `Content-Length` absent (chunked,
/// réponse inattendue) est signalé plutôt que silencieusement mal mesuré.
fn http_get(stream: &mut TcpStream, path: &str) -> (usize, bool) {
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("écriture");

    let mut buf = vec![0u8; 65536];
    let mut total = 0usize;
    // 1) jusqu'à la fin des en-têtes
    let head_end = loop {
        let n = stream.read(&mut buf[total..]).expect("lecture");
        assert!(n > 0, "connexion fermée avant les en-têtes");
        total += n;
        if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        assert!(total < buf.len(), "en-têtes trop longs");
    };

    // 2) le corps annoncé, en entier
    let headers = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
    let content_length = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok());
    let Some(content_length) = content_length else {
        // Pas de Content-Length : on le DIT, on ne fait pas semblant.
        return (total, true);
    };

    let mut body = total - head_end;
    while body < content_length {
        let n = stream.read(&mut buf[..]).expect("lecture");
        assert!(n > 0, "connexion fermée avant la fin du corps");
        body += n;
    }
    (head_end + body, false)
}

/// Formate une durée selon son ordre de grandeur.
fn dur(secs: f64) -> String {
    if secs < 1e-6 {
        format!("{:>9.0} ns", secs * 1e9)
    } else if secs < 1e-3 {
        format!("{:>9.2} µs", secs * 1e6)
    } else {
        format!("{:>9.2} ms", secs * 1e3)
    }
}

/// Une ligne du contrat.
struct Row {
    surface: String,
    cost: Stats,
    includes: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let iters: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(300);
    let http_iters: u32 = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(200);

    println!("contrat de latence — this-rs, mesures sur cette machine");
    println!("  {iters} itérations in-process, {http_iters} requêtes HTTP par surface (médianes)");
    println!();

    // ── Le store de travail : un nœud plat, indexé, avec une décision IAM ──
    let dir = std::env::temp_dir().join(format!("this_latency_{}", uuid::Uuid::new_v4()));
    let store = ObrainStore::open(&dir)?;

    {
        let session = store.db().session();
        session.create_node_with_props(
            &["Page"],
            [(
                "qualified_name",
                obrain_common::types::Value::from("entity:mp_hodge"),
            )],
        );
    }

    let identity = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&identity);
    assert_eq!(report.total_resolved, 1, "le nœud plat doit être indexé");

    let grants = vec![RoleGrant {
        role: "reader".into(),
        permissions: vec!["orders:read".into(), "catalog:read".into()],
    }];
    let iam = store.iam();
    iam.materialize("user:1", "tenant-a", &grants)?;

    // Le journal : quelques entrées pour que la lecture ait de la matière.
    let journal =
        this::events::log_obrain::ObrainJournalLog::open(Arc::clone(&store), "latency-chan")?;
    let mut batch: Vec<this::events::log_obrain::JournalEntry> = (0..64)
        .map(|i| this::events::log_obrain::JournalEntry {
            seq: 0,
            kind: "event".to_string(),
            at_ms: 0,
            payload: serde_json::json!({ "i": i }),
        })
        .collect();
    journal.append_entries(&mut batch)?;

    // Le bail, armé sur ce store.
    let authority = this::storage::writer_lease::FileAuthority::new(&dir);
    store.enable_write_fencing_with(&authority)?;
    let lease = store.lease_info().expect("bail armé");

    // La clé **par le contrat**, jamais recopiée à la main : sa forme a changé
    // avec le schéma v2 (séparateur US), et une clé codée en dur dans un banc
    // devient un banc qui mesure autre chose — ou qui panique.
    let key = this::storage::iam::Iam::acl_key("tenant-a", "user:1");
    // Par la **façade publique** (`GraphBackend`), qui est cloisonnée par espace :
    // `active_store()` est privé hors du crate, et la façade est justement le
    // contrat public qui fait la même chose.
    let acl_node = this::storage::GraphBackend::find_nodes_by_property(
        store.as_ref(),
        this::storage::iam::ACL_KEY_PROP,
        &obrain_common::types::Value::from(key.as_str()),
    )
    .into_iter()
    .next()
    .expect("nœud de décision");

    // ── Surfaces in-process ───────────────────────────────────────────────
    let mut rows: Vec<Row> = Vec::new();

    let node = store
        .db()
        .find_nodes_by_property(
            "qualified_name",
            &obrain_common::types::Value::from("entity:mp_hodge"),
        )
        .into_iter()
        .next()
        .expect("nœud plat");

    let projection = stats(iters, || {
        let n = store.db().session().get_node(node).expect("nœud");
        let _ = identity.project(&this::storage::obrain_identity::node_view(&n, None));
    });
    rows.push(Row {
        surface: "identité · projection d'un nœud".into(),
        cost: (projection),
        includes: "lecture du nœud + chaîne de motifs + hachage SHA-256".into(),
    });

    let resolved = stats(iters, || {
        let hits = store.db().find_nodes_by_property(
            this::storage::iam::PRINCIPAL_QN_PROP,
            &obrain_common::types::Value::from("entity:mp_hodge"),
        );
        let _ = hits;
    });
    rows.push(Row {
        surface: "identité · recherche indexée".into(),
        cost: (resolved),
        includes: "find_nodes_by_property (index)".into(),
    });

    let live = stats(iters, || {
        let _ = store.lease_info().is_some();
    });
    rows.push(Row {
        surface: "bail · état publié".into(),
        cost: (live),
        includes: "lecture du verrou + époque".into(),
    });

    let decision = stats(iters, || {
        let _ = iam.decision("user:1", "tenant-a");
    });
    rows.push(Row {
        surface: "IAM · décision (matérialisée)".into(),
        cost: (decision),
        includes: "find_nodes_by_property + 1 lecture de nœud".into(),
    });

    let stamp = stats(iters, || {
        let _ = iam.stamp_of(acl_node);
    });
    rows.push(Row {
        surface: "IAM · lecture d'un nœud de décision".into(),
        cost: (stamp),
        includes: "1 lecture de nœud (table de propriétés incluse)".into(),
    });

    let append = stats(iters, || {
        let mut v = vec![this::events::log_obrain::JournalEntry {
            seq: 0,
            kind: "event".to_string(),
            at_ms: 0,
            payload: serde_json::json!({ "x": 1 }),
        }];
        let _ = journal.append_entries(&mut v);
    });
    rows.push(Row {
        surface: "journal · append d'une entrée".into(),
        cost: (append),
        includes: "attribution du seq + écriture de zone".into(),
    });

    let read = stats(iters, || {
        let _ = journal.read_from(0, 1024);
    });
    rows.push(Row {
        surface: "journal · relecture depuis 0".into(),
        cost: (read),
        includes: "lecture de zone + tri + dédup (64 entrées ici)".into(),
    });

    // ── La décision d'autorisation : sur CHAQUE requête, par défaut ──────
    //
    // Depuis que l'application des politiques est le mode par défaut, ce coût
    // est payé sur chaque requête vers une route d'entité connue. On mesure les
    // deux formes : une politique du vocabulaire, et une politique `perm:` qu'il
    // faut trancher par la décision matérialisée.
    struct Orders;
    impl EntityDescriptor for Orders {
        fn entity_type(&self) -> &str {
            "order"
        }
        fn plural(&self) -> &str {
            "orders"
        }
        fn build_routes(&self) -> axum::Router {
            axum::Router::new()
        }
    }

    let mut auth_cfg = EntityAuthConfig::default();
    auth_cfg.list = "public".to_string();
    auth_cfg.delete = "perm:orders:write".to_string();
    let mut registry_entities = EntityRegistry::new();
    registry_entities.register(Box::new(Orders));
    let links_config = LinksConfig {
        entities: vec![EntityConfig {
            singular: "order".to_string(),
            plural: "orders".to_string(),
            auth: auth_cfg,
        }],
        links: Vec::<LinkDefinition>::new(),
        validation_rules: None,
        events: None,
        sinks: None,
        auth: None,
    };
    // Le contexte ci-dessous se mappe en `(user_id, tenant_id)` **en texte** —
    // le mappage par défaut de this-rs. La décision doit donc être matérialisée
    // sous ces noms-là : sinon la politique `perm:` mesurerait le chemin
    // « aucune décision » (une recherche indexée qui ne trouve rien), plus
    // rapide et représentatif de rien.
    let principal = uuid::Uuid::from_u128(1).to_string();
    let scope = uuid::Uuid::from_u128(0xa).to_string();
    iam.materialize(
        &principal,
        &scope,
        &[RoleGrant {
            role: "writer".into(),
            permissions: vec!["orders:write".into()],
        }],
    )?;
    let authz = Authorization::new(
        Arc::new(links_config),
        &registry_entities,
        Arc::new(AuthResolverRegistry::new()),
    )
    .with_iam(&store);

    let context = AuthContext::User {
        user_id: uuid::Uuid::from_u128(1),
        tenant_id: uuid::Uuid::from_u128(0xa),
        roles: vec![],
    };

    let plain_policy = stats(iters, || {
        let _ = authz.decide(&axum::http::Method::GET, "/orders", &context);
    });
    rows.push(Row {
        surface: "autorisation · politique du vocabulaire".into(),
        cost: (plain_policy),
        includes: "résolution de route + évaluation (`public`)".into(),
    });

    let iam_policy = stats(iters, || {
        let _ = authz.decide(&axum::http::Method::DELETE, "/orders/1", &context);
    });
    rows.push(Row {
        surface: "autorisation · politique `perm:`".into(),
        cost: (iam_policy),
        includes: "résolution + mappage d'identité + décision IAM indexée".into(),
    });

    println!("── in-process ───────────────────────────────────────────────────────");
    println!(
        "  {:<38} {:>12} {:>12}   {}",
        "surface", "minimum", "médiane", "ce que ça inclut"
    );
    for row in &rows {
        println!(
            "  {:<38} {} {}   {}",
            row.surface,
            dur(row.cost.min),
            dur(row.cost.median),
            row.includes
        );
    }
    println!(
        "  {:<38} {}   {}",
        "(hors contrat)",
        "-",
        "la traversée des rôles n'est PAS mesurée ici : ce store ne porte que les \
décisions matérialisées, pas les arêtes de rôles — la mesurer donnerait un chiffre \
flatteur et faux. La comparaison décision/traversée vit dans `iam_bench`, où le graphe existe."
    );
    println!();

    // ── Surfaces HTTP : socket réelle ─────────────────────────────────────
    let router = this::graph::routes::graph_routes(this::graph::routes::GraphState {
        store: Arc::clone(&store),
        identity: Arc::new(IdentityRegistry::obrain_default()),
    });

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;

    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    // Une seule connexion, réutilisée : le `connect` n'est donc pas mesuré.
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;

    let http_rows = [
        (
            "HTTP · GET /graph",
            "/graph",
            "pile complète : socket, HTTP, handler, sérialisation",
        ),
        (
            "HTTP · GET /graph/identity",
            "/graph/identity",
            "idem + introspection des labels + notes + pièges",
        ),
        (
            "HTTP · GET /graph/identity/{id}",
            "", // rempli ci-dessous depuis l'index réel
            "idem + résolution indexée (200, entité servie)",
        ),
    ];

    // L'identifiant réellement indexé : on sert un 200, pas un 404 — un 404
    // mesurerait le chemin d'erreur et le présenterait comme le chemin nominal.
    let real_id = store
        .resolve_identity(&identity, &{
            let key = this::core::identity::EntityKey::new(
                None,
                "Page",
                this::core::identity::Qualifier::Natural {
                    property: "qualified_name".into(),
                    value: "entity:mp_hodge".into(),
                },
            )
            .expect("clé canonique");
            key.uuid()
        })
        .ok()
        .flatten()
        .map(|e| e.id.to_string())
        .expect("identité indexée");

    println!("── HTTP (socket réelle, connexion réutilisée) ───────────────────────");
    for (label, path, includes) in http_rows {
        let owned;
        let path = if path.is_empty() {
            owned = format!("/graph/identity/{real_id}");
            owned.as_str()
        } else {
            path
        };
        let (bytes, headers_only) = http_get(&mut stream, path);
        assert!(
            !headers_only,
            "réponse sans Content-Length : mesure invalide"
        );
        let cost = stats(http_iters, || {
            let (n, _) = http_get(&mut stream, path);
            assert!(n > 0, "réponse vide");
        });
        println!(
            "  {label:<38} {} {}   {includes}",
            dur(cost.min),
            dur(cost.median)
        );
    }
    println!();

    // ── Le contrat, en une phrase ─────────────────────────────────────────
    println!("── lecture ──────────────────────────────────────────────────────────");
    println!(
        "  bail (chemin chaud)   {}  ← un load atomique, appelé à CHAQUE écriture",
        dur(live.min)
    );
    println!(
        "  décision IAM          {}  ← appelée à chaque requête autorisée",
        dur(decision.min)
    );
    let http_identity = stats(http_iters, || {
        let (n, _) = http_get(&mut stream, "/graph/identity");
        assert!(n > 0);
    })
    .min;
    println!(
        "  HTTP /graph/identity  {}  ← ×{:.0} la décision IAM : le réseau et le document \
         dominent, pas l'autorisation",
        dur(http_identity),
        http_identity / decision.min
    );
    println!();
    println!(
        "  état : bail armé epoch={} shard={} · index d'identité {} entrée(s) · journal 64 entrées",
        lease.epoch,
        lease.shard.as_str(),
        store.identity_len()
    );

    server.abort();
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
