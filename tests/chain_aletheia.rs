//! **La chaîne aletheia, vérifiée de bout en bout.**
//!
//! aletheia sert aujourd'hui **douze domaines** sous `aletheia-chain.com`,
//! chacun avec son contenu (site, console, explorateur, documentation, serveur
//! git, RPC, MCP…), derrière un edge Rust et un serveur git séparé. La question
//! est : cette application peut-elle **tout** basculer sur this-rs, avec une
//! **délégation propre de tout le domaine** ?
//!
//! Ce fichier en vérifie les trois maillons, sur les domaines **réels** :
//!
//! 1. **la garde** : les douze domaines sont servis, tout autre est refusé ;
//! 2. **la répartition** : chaque domaine sert **son** contenu, dans un seul
//!    processus ;
//! 3. **la délégation** : le `Caddyfile` décrit les douze sites, chacun avec
//!    **son** upstream (l'API, le RPC, le serveur git ne sont pas au même port).
//!
//! Ce qui n'est **pas** vérifiable ici est listé dans
//! `docs/migration-aletheia.md` : la sémantique métier (chaîne, portefeuille,
//! maillage P2P), le rendu SSR, et la migration des données.

#![cfg(feature = "frontend")]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use this::config::{
    CaddyConfig, CaddyTls, CorsConfig, DomainConfig, ProxyConfig, ServerSecurityConfig,
};
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::frontend::{Frontend, FrontendSites};
use this::storage::in_memory::InMemoryLinkService;

/// **Les douze domaines d'aletheia, et le port qui les sert.**
///
/// Relevés dans le dépôt d'aletheia (documents de déploiement, scripts,
/// launcher) : c'est la matière de la vérification, pas un exemple inventé.
const DOMAINS: &[(&str, u16)] = &[
    ("www.aletheia-chain.com", 8080),
    ("mathematics.aletheia-chain.com", 8080),
    ("chat.aletheia-chain.com", 8080),
    ("wallet.aletheia-chain.com", 8080),
    ("docs.aletheia-chain.com", 8080),
    ("install.aletheia-chain.com", 8080),
    ("x.aletheia-chain.com", 8080),
    ("explorer.aletheia-chain.com", 8080),
    ("mcp.aletheia-chain.com", 8081),
    ("rpc.aletheia-chain.com", 8082),
    ("git.aletheia-chain.com", 8086),
    ("git-api.aletheia-chain.com", 8086),
];

/// Un dossier de site par domaine, avec un marqueur reconnaissable.
fn sites() -> (tempfile::TempDir, Vec<(&'static str, Frontend)>) {
    let dir = tempfile::tempdir().expect("répertoire");
    let mut fronts = Vec::new();
    for (host, _) in DOMAINS {
        let root = dir.path().join(host);
        std::fs::create_dir_all(&root).expect("dossier");
        std::fs::write(root.join("index.html"), format!("<h1>{host}</h1>")).expect("index");
        // Le nom d'hôte est `'static` (constante) : on le reprend tel quel.
        let host: &'static str = DOMAINS
            .iter()
            .map(|(h, _)| *h)
            .find(|h| h == host)
            .expect("hôte connu");
        fronts.push((host, Frontend::disk(&root)));
    }
    (dir, fronts)
}

/// La configuration d'exposition d'aletheia : ses domaines, son CORS, sa
/// délégation — telle qu'elle s'écrirait dans `config/server.yaml`.
fn aletheia_config() -> ServerSecurityConfig {
    let upstreams: BTreeMap<String, String> = DOMAINS
        .iter()
        .filter(|(_, port)| *port != 8080)
        .map(|(host, port)| ((*host).to_string(), format!("127.0.0.1:{port}")))
        .collect();

    ServerSecurityConfig {
        cors: CorsConfig {
            enabled: true,
            origins: DOMAINS
                .iter()
                .map(|(host, _)| format!("https://{host}"))
                .collect(),
            credentials: true,
            ..Default::default()
        },
        domains: DomainConfig {
            // **La délégation du domaine** : tout `aletheia-chain.com` d'un
            // niveau, et rien d'autre. L'apex n'est pas servi — c'est `www` qui
            // l'est, et le Caddyfile redirige l'apex.
            allowed: vec![
                "*.aletheia-chain.com".to_string(),
                "aletheia-chain.com".to_string(),
            ],
            // **La liste explicite des sites** : `allowed` est un filtre, un
            // `Caddyfile` énumère des domaines.
            sites: DOMAINS
                .iter()
                .map(|(host, _)| (*host).to_string())
                .collect(),
            trust_forwarded_host: true,
            ..Default::default()
        },
        proxy: ProxyConfig {
            caddy: CaddyConfig {
                enabled: true,
                upstream: "127.0.0.1:8080".to_string(),
                email: Some("ops@aletheia-chain.com".to_string()),
                tls: CaddyTls::Acme,
                security_headers: true,
                upstreams,
                extra_directives: Vec::new(),
            },
        },
        ..Default::default()
    }
}

fn app(config: ServerSecurityConfig, fronts: Vec<(&'static str, Frontend)>) -> Router {
    let mut sites = FrontendSites::new().trust_forwarded_host(true);
    for (host, frontend) in fronts {
        sites = sites.site(host, frontend);
    }

    // **Le repli passe par le builder** : il est monté AVANT les couches, donc
    // le CORS, la garde de domaine, l'authentification et les politiques
    // s'appliquent AUSSI aux sites. Composer `attach` soi-même après
    // `build_router` les contournerait — c'est le piège que ce test aurait
    // reproduit.
    let host = ServerBuilder::new()
        .with_server_security(config)
        .with_frontend_sites(sites)
        .with_link_service(InMemoryLinkService::new())
        .build_host()
        .expect("hôte");

    RestExposure::build_router(Arc::new(host), vec![]).expect("routeur")
}

async fn get(app: Router, host: &str, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("corps");
    (status, String::from_utf8_lossy(&body).to_string())
}

// ===========================================================================
// 1. TOUS LES DOMAINES SONT SERVIS, ET EUX SEULS
// ===========================================================================

/// Les **douze** domaines d'aletheia sont servis ; tout autre hôte est refusé.
#[tokio::test]
async fn every_aletheia_domain_is_served_and_nothing_else_is() {
    let (_dir, fronts) = sites();
    let app = app(aletheia_config(), fronts);

    for (host, _) in DOMAINS {
        let (status, body) = get(app.clone(), host, "/").await;
        assert_eq!(status, StatusCode::OK, "`{host}` doit être servi : {body}");
        assert!(
            body.contains(host),
            "`{host}` doit servir SON contenu, obtenu : {body}"
        );
    }

    // **Deux niveaux, et c'est voulu.** La garde autorise des *familles*
    // d'hôtes (`*.aletheia-chain.com`) ; la liste des **sites** dit ce qui
    // existe. Un sous-domaine de la famille mais sans site obtient donc 404 —
    // pas 421, et surtout pas le contenu d'un autre domaine.
    let (status, body) = get(app.clone(), "evil.aletheia-chain.com", "/").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "un sous-domaine sans site ne sert rien : {body}"
    );

    // Hors de la famille : c'est la GARDE qui refuse.
    // `*.aletheia-chain.com` ne couvre pas deux niveaux.
    let (status, body) = get(app.clone(), "a.b.aletheia-chain.com", "/").await;
    assert_eq!(
        status,
        StatusCode::MISDIRECTED_REQUEST,
        "deux niveaux ne sont pas couverts par le joker : {body}"
    );

    // Un domaine qui ressemble : le suffixe trompeur ne passe pas.
    let (status, _) = get(app, "aletheia-chain.com.evil.fr", "/").await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
}

/// L'**apex** est autorisé par la configuration mais n'a pas de site : il ne doit
/// pas servir silencieusement le contenu de `www` — c'est une redirection, et
/// elle appartient à Caddy.
#[tokio::test]
async fn the_apex_is_authorised_but_has_no_site_of_its_own() {
    let (_dir, fronts) = sites();
    let app = app(aletheia_config(), fronts);

    let (status, body) = get(app, "aletheia-chain.com", "/").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "sans site déclaré pour l'apex, la réponse est 404 — jamais le contenu d'un autre \
         domaine : {body}"
    );
}

// ===========================================================================
// 2. CHAQUE DOMAINE SERT SON CONTENU
// ===========================================================================

/// La répartition par hôte : un seul processus, douze contenus. Un domaine ne
/// doit **jamais** servir le site d'un autre.
#[tokio::test]
async fn each_domain_serves_its_own_content_and_never_another() {
    let (_dir, fronts) = sites();
    let app = app(aletheia_config(), fronts);

    for (host, _) in DOMAINS {
        let (_, body) = get(app.clone(), host, "/").await;
        for (other, _) in DOMAINS {
            if other == host {
                continue;
            }
            assert!(
                !body.contains(other),
                "`{host}` ne doit pas servir le contenu de `{other}` : {body}"
            );
        }
    }
}

/// Le contenu statique suit : un fichier d'un site n'est pas servi par un autre.
#[tokio::test]
async fn a_static_asset_is_scoped_to_its_domain() {
    let (dir, fronts) = sites();
    std::fs::write(
        dir.path().join("rpc.aletheia-chain.com").join("seul.txt"),
        "rpc",
    )
    .expect("fichier");
    let app = app(aletheia_config(), fronts);

    let (status, body) = get(app.clone(), "rpc.aletheia-chain.com", "/seul.txt").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "rpc");

    // Le même chemin, sur un autre domaine : le fichier de `rpc` n'existe pas
    // là. Qu'il soit 404 (chemin d'actif) ou le SPA de `www`, l'essentiel est
    // qu'il ne serve PAS le contenu de `rpc`.
    let (status, body) = get(app, "www.aletheia-chain.com", "/seul.txt").await;
    assert!(
        !body.contains("rpc"),
        "un fichier de `rpc` ne doit jamais être servi par `www` : {status} {body}"
    );
}

// ===========================================================================
// 3. LA DÉLÉGATION CADDYFILE COUVRE TOUT LE DOMAINE
// ===========================================================================

/// Le `Caddyfile` décrit **les douze sites**, chacun avec **son** upstream :
/// l'API, le RPC et le serveur git ne sont pas au même port.
#[test]
fn the_caddyfile_delegates_all_twelve_domains_with_their_own_upstreams() {
    let config = aletheia_config();
    let caddyfile = config.caddyfile().expect("délégation demandée");

    for (host, port) in DOMAINS {
        assert!(
            caddyfile.contains(&format!("{host} {{")),
            "le site `{host}` doit avoir son bloc :\n{caddyfile}"
        );
        assert!(
            caddyfile.contains(&format!("reverse_proxy 127.0.0.1:{port}")),
            "`{host}` doit pointer sur le port {port} :\n{caddyfile}"
        );
    }

    // Le bloc global porte l'ACME.
    assert!(caddyfile.contains("email ops@aletheia-chain.com"));

    // Et le fichier dit que le CORS est posé par l'application : deux couches ne
    // doivent pas poser des en-têtes qui se contredisent.
    assert!(caddyfile.contains("posé par this-rs"), "{caddyfile}");
}

// ===========================================================================
// 4. LE CORS DES ORIGINES D'ALETHEIA
// ===========================================================================

/// Un pré-vol depuis l'une des origines d'aletheia est accepté **sans jeton** ;
/// une origine inconnue n'obtient rien.
#[tokio::test]
async fn the_spa_origins_are_accepted_and_others_are_not() {
    let (_dir, fronts) = sites();
    let app = app(aletheia_config(), fronts);

    let preflight = |origin: &'static str| {
        let app = app.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/")
                    .header("host", "www.aletheia-chain.com")
                    .header("origin", origin)
                    .header("access-control-request-method", "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("réponse")
            .headers()
            .clone()
        }
    };

    let headers = preflight("https://wallet.aletheia-chain.com").await;
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap()),
        Some("https://wallet.aletheia-chain.com"),
        "une origine d'aletheia doit être acceptée"
    );
    assert!(
        headers.contains_key("access-control-allow-methods"),
        "et le pré-vol doit annoncer les méthodes"
    );

    let headers = preflight("https://attaque.example.fr").await;
    assert!(
        !headers.contains_key("access-control-allow-origin"),
        "une origine inconnue n'obtient aucun en-tête"
    );
}

// ===========================================================================
// 5. LA CONFIGURATION TELLE QU'ELLE S'ÉCRIRAIT
// ===========================================================================

/// La configuration complète est **lisible** depuis un `YAML` : c'est ce qui
/// permet de la versionner et de la relire, plutôt que de la disperser dans le
/// code de déploiement.
#[test]
fn the_whole_domain_delegation_fits_in_one_readable_file() {
    let yaml = r#"
cors:
  enabled: true
  origins:
    - "https://www.aletheia-chain.com"
    - "https://wallet.aletheia-chain.com"
  credentials: true

domains:
  # `allowed` : les familles d'hôtes autorisées (un filtre)
  allowed: ["*.aletheia-chain.com", "aletheia-chain.com"]
  # `sites` : les domaines RÉELLEMENT servis (ce qu'un `Caddyfile` énumère)
  sites:
    - "www.aletheia-chain.com"
    - "wallet.aletheia-chain.com"
    - "rpc.aletheia-chain.com"
    - "git.aletheia-chain.com"
  trust_forwarded_host: true

proxy:
  caddy:
    enabled: true
    upstream: "127.0.0.1:8080"
    email: "ops@aletheia-chain.com"
    upstreams:
      "rpc.aletheia-chain.com": "127.0.0.1:8082"
      "git.aletheia-chain.com": "127.0.0.1:8086"
"#;
    let config = ServerSecurityConfig::from_yaml(yaml).expect("configuration lisible");
    config.cors.validate().expect("CORS cohérent");
    config.domains.validate().expect("domaines cohérents");

    let caddyfile = config.caddyfile().expect("délégation");
    assert!(
        caddyfile.contains("git.aletheia-chain.com {")
            && caddyfile.contains("reverse_proxy 127.0.0.1:8086"),
        "le serveur git est délégué à SON port :\n{caddyfile}"
    );
    assert!(
        caddyfile.contains("reverse_proxy 127.0.0.1:8080"),
        "et l'API garde le sien :\n{caddyfile}"
    );
}

// ===========================================================================
// 6. LA VUE MULTI-ÉCHELLE SUR L'EXPOSITION RÉELLE
// ===========================================================================

/// **L'exigence** : voir les sous-groupes des sous-groupes des sous-groupes, à
/// travers l'interface — servie par le routeur, bornée, et lisible par un moteur
/// d'inférence.
#[tokio::test]
async fn the_multi_scale_view_is_served_by_the_router() {
    use this::core::identity::IdentityRegistry;
    use this::storage::ObrainStore;

    // Un arbre : racine → 2 groupes → 4 sous-groupes → 8 sous-sous-groupes.
    let store = ObrainStore::new_in_memory();
    let root = {
        let session = store.db().session();
        let root = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("aletheia"))],
        );
        let mut frontier = vec![root];
        for niveau in 1..=3 {
            let mut next = Vec::new();
            for parent in &frontier {
                for i in 0..2 {
                    let child = session.create_node_with_props(
                        &["Group"],
                        [(
                            "name",
                            obrain_common::types::Value::from(format!("g{niveau}-{i}")),
                        )],
                    );
                    session.create_edge(*parent, child, "CONTAINS");
                    next.push(child);
                }
            }
            frontier = next;
        }
        store.commit_wal().expect("commit");
        root.as_u64()
    };

    let app = this::graph::graph_routes(this::graph::GraphState {
        store: std::sync::Arc::clone(&store),
        identity: std::sync::Arc::new(IdentityRegistry::obrain_default()),
    });

    // Les trois niveaux, en JSON.
    let (status, body) = get(
        app.clone(),
        "www.aletheia-chain.com",
        &format!("/graph/hierarchy/{root}?depth=3"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(json["depth"], 3, "{json}");
    assert_eq!(json["levels"][0]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(json["levels"][1]["nodes"].as_array().unwrap().len(), 4);
    assert_eq!(
        json["levels"][2]["nodes"].as_array().unwrap().len(),
        8,
        "les sous-groupes des sous-groupes des sous-groupes : {json}"
    );
    assert_eq!(json["levels"][0]["truncated"], false, "rien n'est coupé");

    // La forme lisible par un moteur d'inférence.
    let (status, outline) = get(
        app,
        "www.aletheia-chain.com",
        &format!("/graph/hierarchy/{root}?depth=2&format=outline"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(outline.contains("aletheia"), "{outline}");
    assert!(outline.contains("- g1-0"), "{outline}");
    assert!(
        outline.contains("↳ /graph/hierarchy/"),
        "un niveau non descendu donne son adresse : {outline}"
    );
}
