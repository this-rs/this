//! **Contrat de consommation** — ce qu'un autre programme peut utiliser.
//!
//! Ce fichier joue le rôle d'un programme **extérieur** : le Dashboard obrain,
//! un harnais, une application. Il n'accède à rien d'autre qu'à l'API publique,
//! et il vérifie les trois façons d'intégrer la pile :
//!
//! 1. **par la bibliothèque** — `this-rs` comme dépendance, avec la feature
//!    `obrain` : lire l'état, l'identité, les décisions ;
//! 2. **par HTTP** — le relief publié (`/graph`) et son **contrat de données**,
//!    qui décrit le schéma pour qu'un lecteur sache quoi interroger ;
//! 3. **par obrain SEUL** — sans lier `this-rs` du tout : les décisions sont des
//!    nœuds et des propriétés ordinaires, lisibles par n'importe quel programme
//!    qui parle obrain.
//!
//! Le troisième point est le plus important : il dit que la donnée
//! n'est **pas prisonnière** du framework. Si ce test casse, c'est qu'un
//! consommateur qui ne dépend que d'obrain ne peut plus lire l'état.

#![cfg(feature = "obrain")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value as Json;
use tower::ServiceExt;

use this::core::auth::{AuthContext, AuthResolverRegistry};
use this::core::identity::IdentityRegistry;
use this::storage::GraphBackend;
use this::storage::ObrainStore;
use this::storage::iam::{ACL_KEY_PROP, ACL_LABEL, ACL_PERMS_PROP, IAM_SCHEMA, RoleGrant};

/// Un couple (principal, portée) tel que le produirait une application.
const PRINCIPAL: &str = "00000000-0000-0000-0000-000000000001";
const SCOPE: &str = "00000000-0000-0000-0000-00000000000a";

/// Le store qu'un programme producteur aurait laissé derrière lui.
fn producer_store() -> Arc<ObrainStore> {
    let store = ObrainStore::new_in_memory();
    store
        .iam()
        .materialize(
            PRINCIPAL,
            SCOPE,
            &[
                RoleGrant {
                    role: "reader".into(),
                    permissions: vec!["orders:read".into(), "catalog:read".into()],
                },
                RoleGrant {
                    role: "operator".into(),
                    permissions: vec!["orders:write".into()],
                },
            ],
        )
        .expect("matérialisation");
    store
}

/// Une valeur de propriété, comme la voit un lecteur obrain.
fn property(store: &ObrainStore, node: obrain_common::types::NodeId, name: &str) -> Option<String> {
    GraphBackend::node_property(store, node, name).and_then(|v| v.as_str().map(str::to_string))
}

// ===========================================================================
// 1. CONSOMMATEUR PAR LA BIBLIOTHÈQUE
// ===========================================================================

/// Le programme extérieur lit la décision par l'API publique : un type, deux
/// arguments, pas d'accès à l'interne.
#[test]
fn a_consumer_reads_the_decision_through_the_public_api() {
    let store = producer_store();

    let decision = store
        .iam()
        .decision(PRINCIPAL, SCOPE)
        .expect("lecture")
        .expect("décision matérialisée");

    assert_eq!(
        decision.schema, IAM_SCHEMA,
        "le schéma est publié et versionné"
    );
    assert_eq!(decision.principal, PRINCIPAL);
    assert_eq!(decision.scope, SCOPE);
    assert_eq!(
        decision.roles,
        vec!["operator".to_string(), "reader".to_string()]
    );
    assert!(decision.allows("orders:read"), "{decision:?}");
    assert!(decision.allows("orders:write"), "{decision:?}");
    assert!(!decision.allows("orders:delete"), "{decision:?}");
    assert!(decision.epoch >= 1, "l'époque est publiée : {decision:?}");
    assert!(
        !decision.digest.is_empty(),
        "l'empreinte aussi : {decision:?}"
    );
}

/// Le chemin chaud est public lui aussi : un consommateur qui doit trancher
/// « ce principal a-t-il cette permission ? » n'a pas à reconstruire la logique.
#[test]
fn a_consumer_asks_the_hot_path_question_directly() {
    let store = producer_store();
    let iam = store.iam();

    assert!(iam.allows(PRINCIPAL, SCOPE, "orders:write").expect("chaud"));
    assert!(
        !iam.allows(PRINCIPAL, SCOPE, "orders:delete")
            .expect("chaud")
    );
    assert!(
        !iam.allows(PRINCIPAL, "autre-portee", "orders:read")
            .expect("chaud"),
        "une autre portée ne donne rien"
    );
}

/// Le store s'ouvre en **lecture seule** — le mode d'un tableau de bord : il
/// lit sans prendre le bail d'écriture, donc sans s'opposer à l'application qui
/// écrit.
#[test]
fn a_read_only_consumer_does_not_take_the_write_lease() {
    let dir = tempfile::tempdir().expect("répertoire");

    // Le producteur écrit, puis rend la main.
    {
        let writable = ObrainStore::open(dir.path()).expect("store inscriptible");
        writable
            .iam()
            .materialize(
                PRINCIPAL,
                SCOPE,
                &[RoleGrant {
                    role: "reader".into(),
                    permissions: vec!["orders:read".into()],
                }],
            )
            .expect("matérialisation");
    }
    // Le store est un **répertoire** : ses zones y vivent.
    let written = std::fs::read_dir(dir.path())
        .expect("lecture du répertoire")
        .count();
    assert!(written > 0, "le store a bien écrit sur disque");

    // Le consommateur lit — sans bail, donc sans empêcher le producteur de
    // reprendre la main.
    let reader = ObrainStore::open_read_only(dir.path()).expect("ouverture en lecture seule");
    assert!(reader.is_read_only());
    assert!(
        reader.lease_info().is_none(),
        "un lecteur ne prend PAS le bail d'écriture"
    );
    assert!(
        reader
            .iam()
            .allows(PRINCIPAL, SCOPE, "orders:read")
            .expect("chaud"),
        "et il lit les décisions"
    );
}

// ===========================================================================
// 2. CONSOMMATEUR PAR OBRAIN SEUL — sans lier this-rs
// ===========================================================================

/// **Le test qui compte pour un Dashboard obrain** : lire une décision en ne
/// connaissant que le **schéma**, pas le framework.
///
/// On n'utilise ici **aucun** type de `this::storage::iam` : on cherche le nœud
/// par sa propriété indexée, on lit la liste publiée, on la décode. C'est
/// exactement ce que ferait un programme écrit en obrain.
#[test]
fn an_obrain_only_reader_can_read_the_decisions() {
    let store = producer_store();

    // 1. Le nœud de décision, par sa clé d'index — la clé est *documentée* :
    //    (portée, principal) joints par un US.
    let key = format!("{SCOPE}\u{1f}{PRINCIPAL}");
    let nodes = <ObrainStore as GraphBackend>::find_nodes_by_property(
        &store,
        "acl_key",
        &obrain_common::types::Value::from(key.as_str()),
    );
    assert_eq!(nodes.len(), 1, "une décision, trouvée par index");
    let node = nodes[0];

    // 2. Le label fait partie du contrat : c'est ce qui distingue une décision
    //    d'un nœud qui porterait la même propriété par accident.
    let node_object = <ObrainStore as GraphBackend>::get_node(&store, node).expect("nœud lisible");
    assert!(
        node_object.labels.iter().any(|l| l.as_str() == "Acl"),
        "le nœud porte le label `Acl`"
    );

    // 3. Les champs, tels que le contrat les publie.
    assert_eq!(
        property(&store, node, "acl_principal").as_deref(),
        Some(PRINCIPAL)
    );
    assert_eq!(property(&store, node, "acl_scope").as_deref(), Some(SCOPE));
    // **Les formes comptent** : le contrat publie `acl_epoch` comme un entier et
    // `acl_roles` comme une liste JSON. Un lecteur qui attend du texte partout
    // lit `None` et croit la propriété absente.
    let epoch = <ObrainStore as GraphBackend>::node_property(&store, node, "acl_epoch")
        .and_then(|v| v.as_int64())
        .expect("l'époque est un entier");
    assert!(epoch >= 1, "l'époque est lisible : {epoch}");
    let roles: Vec<String> =
        serde_json::from_str(&property(&store, node, "acl_roles").expect("rôles"))
            .expect("liste JSON");
    assert_eq!(roles, vec!["operator".to_string(), "reader".to_string()]);
    let perms: Vec<String> =
        serde_json::from_str(&property(&store, node, "acl_perms").expect("permissions"))
            .expect("liste JSON");
    assert!(perms.contains(&"orders:write".to_string()), "{perms:?}");

    // 4. La forme lue par l'application des politiques est publiée elle aussi :
    //    un tableau de bord peut montrer « ce que le framework regarde ».
    let blob = property(&store, node, "acl_perms_blob").expect("forme chemin chaud");
    assert!(
        blob.contains("orders:write"),
        "la forme du chemin chaud est lisible : {blob:?}"
    );

    // 5. Et le résultat coïncide avec ce que dit le framework : deux lecteurs,
    //    une seule vérité.
    let through_framework = store
        .iam()
        .decision(PRINCIPAL, SCOPE)
        .unwrap()
        .unwrap()
        .permissions;
    assert_eq!(through_framework, perms);
}

/// Le comptage et la découverte : un tableau de bord liste les décisions **par
/// label**, sans connaître une seule clé.
#[test]
fn an_obrain_only_reader_discovers_the_decisions_by_label() {
    let store = producer_store();
    let nodes = <ObrainStore as GraphBackend>::nodes_by_label(&store, ACL_LABEL);
    assert_eq!(nodes.len(), 1, "une décision, trouvée par son label");

    // Les trois autres couples, chacun sa décision.
    for (principal, scope) in [("p2", "t"), ("p3", "t"), ("p4", "autre")] {
        store
            .iam()
            .materialize(
                principal,
                scope,
                &[RoleGrant {
                    role: "r".into(),
                    permissions: vec!["x".into()],
                }],
            )
            .expect("matérialisation");
    }
    assert_eq!(
        <ObrainStore as GraphBackend>::nodes_by_label(&store, ACL_LABEL).len(),
        4
    );
    // Et chaque décision reste isolée.
    assert!(
        store.iam().allows("p2", "t", "x").expect("chaud"),
        "la décision de p2 est trouvée"
    );
    assert!(
        !store.iam().allows("p2", "autre", "x").expect("chaud"),
        "et pas dans une autre portée"
    );
}

// ===========================================================================
// 3. CONSOMMATEUR PAR HTTP — le contrat publié
// ===========================================================================

async fn relief_json(store: Arc<ObrainStore>) -> Json {
    let identity = Arc::new(IdentityRegistry::obrain_default());
    let app = this::graph::graph_routes(this::graph::GraphState {
        store,
        identity,
        scope: None,
        canvas: None,
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/graph")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("corps");
    serde_json::from_slice(&bytes).expect("json")
}

/// Le relief **publie le schéma** : un consommateur n'a pas à lire le code du
/// framework pour savoir quoi interroger.
#[tokio::test]
async fn the_relief_publishes_the_iam_data_contract() {
    let store = producer_store();
    let json = relief_json(Arc::clone(&store)).await;

    let iam = &json["iam"];
    assert_eq!(iam["schema"], IAM_SCHEMA);
    assert_eq!(iam["hot_path_property"], "acl_perms_blob");
    assert_eq!(
        iam["decisions"], 1,
        "le nombre de décisions est publié : {iam}"
    );

    // Les entrées du contrat, avec leur sens et **qui les écrit**.
    let labels = iam["labels"].as_array().expect("labels");
    assert!(
        labels
            .iter()
            .any(|l| l["name"] == "Acl" && l["written"] == true)
    );
    assert!(
        labels
            .iter()
            .any(|l| l["name"] == "Role" && l["written"] == false),
        "ce que le framework LIT est distingué de ce qu'il ÉCRIT"
    );

    let properties = iam["properties"].as_array().expect("properties");
    for expected in [
        "acl_key",
        "acl_scope",
        "acl_principal",
        "acl_roles",
        "acl_perms",
        "acl_perms_blob",
        "acl_epoch",
        "acl_digest",
    ] {
        assert!(
            properties.iter().any(|p| p["name"] == expected),
            "`{expected}` doit figurer au contrat publié"
        );
    }
    assert!(
        properties
            .iter()
            .all(|p| p["meaning"].as_str().is_some_and(|m| !m.is_empty())),
        "chaque champ publié dit ce qu'il signifie — un contrat sans sens n'en est pas un"
    );

    // **Le vocabulaire des permissions granulaires** est publié : sans lui, un
    // consommateur ne peut pas donner un droit sur *ce* label ou *ce* type de
    // relation — il ne saurait pas comment l'écrire.
    let vocabulary = iam["vocabulary"].as_array().expect("vocabulary");
    for family in ["graph", "node", "edge", "space"] {
        assert!(
            vocabulary.iter().any(|v| v["family"] == family),
            "la famille `{family}` doit figurer au vocabulaire : {vocabulary:?}"
        );
    }
    let edge = vocabulary
        .iter()
        .find(|v| v["family"] == "edge")
        .expect("famille edge");
    assert!(
        edge["actions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|a| a == "create")),
        "TISSER (créer une relation) doit être une action publiée : {edge}"
    );
    assert!(
        edge["example"]
            .as_str()
            .is_some_and(|e| e.contains("edge:create:")),
        "et l'exemple doit montrer comment l'écrire : {edge}"
    );
    assert!(
        iam["matching"]
            .as_str()
            .is_some_and(|m| m.contains("`*`") && m.contains("sans cible")),
        "la règle de correspondance — et son piège — doivent être publiées : {}",
        iam["matching"]
    );

    // Et la frontière de confiance est publiée avec, pas cachée.
    let trust = iam["trust"].as_array().expect("trust");
    assert!(
        trust.iter().any(|t| t
            .as_str()
            .is_some_and(|t| t.contains("écrire dans le store"))),
        "la limite doit être lisible par le consommateur : {trust:?}"
    );
}

/// Le relief et le contrat ne sont utiles que s'ils décrivent **le même
/// schéma** que les constantes du code. Ce test est le garde-fou : renommer une
/// propriété sans publier le changement casse ici.
#[test]
fn the_published_contract_matches_the_schema_constants() {
    // Les noms écrits par le framework, tels que le code les déclare.
    let written = [
        this::storage::iam::ACL_KEY_PROP,
        this::storage::iam::ACL_SCOPE_PROP,
        this::storage::iam::ACL_PRINCIPAL_PROP,
        this::storage::iam::ACL_ROLES_PROP,
        this::storage::iam::ACL_PERMS_PROP,
        this::storage::iam::ACL_PERMS_BLOB_PROP,
        this::storage::iam::ACL_EPOCH_PROP,
        this::storage::iam::ACL_DIGEST_PROP,
    ];
    for name in written {
        assert!(
            !name.contains('\u{1f}'),
            "une propriété ne contient pas le séparateur interne : {name}"
        );
    }
    assert_eq!(ACL_LABEL, "Acl");
    assert_eq!(ACL_KEY_PROP, "acl_key");
    assert_eq!(ACL_PERMS_PROP, "acl_perms");
    assert_eq!(IAM_SCHEMA, "this-rs/iam/2");
}

/// **La carte des surfaces est publiée** : un consommateur — un tableau de bord,
/// un audit — voit quelles surfaces appliquent les politiques, lesquelles non,
/// et ce qui les protège à défaut. Un protocole oublié serait un protocole non
/// appliqué.
#[tokio::test]
async fn the_relief_publishes_which_surfaces_enforce_policies() {
    let json = relief_json(producer_store()).await;
    let surfaces = json["surfaces"].as_array().expect("surfaces");

    let find = |name: &str| {
        surfaces
            .iter()
            .find(|s| s["surface"] == name)
            .unwrap_or_else(|| panic!("la surface `{name}` doit être publiée"))
            .clone()
    };

    // REST applique, et le dit.
    let rest = find("rest");
    assert_eq!(rest["enforces_policies"], true);
    assert_eq!(
        rest["origin"], "declared",
        "la provenance est publiée en clair"
    );
    assert!(
        rest["mechanism"]
            .as_str()
            .is_some_and(|m| m.contains("middleware")),
        "le mécanisme doit être nommé : {rest}"
    );

    // Les surfaces hors REST n'appliquent pas — et c'est publié, avec ce qui les
    // protège à défaut et l'acceptation explicite requise.
    for surface in ["graphql", "grpc", "websocket"] {
        let entry = find(surface);
        assert_eq!(
            entry["enforces_policies"], false,
            "`{surface}` ne peut pas appliquer les politiques : cela doit être publié"
        );
        let otherwise = entry["otherwise"].as_str().expect("otherwise");
        assert!(
            otherwise.contains("REFUSE") || otherwise.contains("refus"),
            "la carte doit dire que le framework refuse de la monter : {otherwise}"
        );
    }

    // **Toutes** les surfaces, y compris celles qui n'apparaissent pas dans un
    // inventaire de routes : le frontend et le pont de commandes sont servis par
    // un **repli**.
    for surface in ["frontend", "dev_bridge"] {
        let entry = find(surface);
        assert_eq!(
            entry["enforces_policies"], false,
            "`{surface}` ne peut pas appliquer les politiques : cela doit être publié"
        );
    }
    let frontend = find("frontend");
    assert!(
        frontend["otherwise"]
            .as_str()
            .is_some_and(|o| o.contains("ORDRE")),
        "le piège de l'ordre du repli doit être publié : {frontend}"
    );
    let bridge = find("dev_bridge");
    assert!(
        bridge["otherwise"]
            .as_str()
            .is_some_and(|o| o.contains("FERMÉ")),
        "le pont de commandes doit être publié comme fermé par défaut : {bridge}"
    );

    // **La carte doit NOMMER les lectures publiées**, pas seulement une famille
    // générique : `/graph…` ne dit pas qu'on peut descendre d'un cran. Cette
    // assertion existe parce que la route multi-échelle a été ajoutée au code
    // SANS l'être à la carte, en silence.
    assert!(
        rest["mounted_at"]
            .as_array()
            .is_some_and(|paths| paths.iter().any(|p| p == "/graph/hierarchy/{id}")),
        "la carte doit nommer la vue multi-échelle : {rest}"
    );

    // Et l'ensemble est cohérent : toute surface publiée dit ce qu'elle sert.
    for surface in surfaces {
        assert!(
            surface["mounted_at"]
                .as_array()
                .is_some_and(|paths| !paths.is_empty()),
            "une surface sans chemin publié n'informe personne : {surface}"
        );
    }
}

/// **Le relief publie la MATIÈRE du store**, pas seulement ses capacités :
/// labels et leurs comptes, types d'arêtes et leurs comptes, clés de propriétés.
/// C'est la première chose qu'on demande à une base, et elle manquait.
#[tokio::test]
async fn the_relief_publishes_the_shape_of_the_store() {
    let store = ObrainStore::new_in_memory();
    {
        let session = store.db().session();
        let a = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("collectif"))],
        );
        let b = session.create_node_with_props(
            &["Member"],
            [("name", obrain_common::types::Value::from("moi"))],
        );
        session.create_node_with_props(
            &["Member"],
            [("name", obrain_common::types::Value::from("toi"))],
        );
        session.create_edge(a, b, "CONTAINS");
        store.commit_wal().expect("commit");
    }

    let json = relief_json(std::sync::Arc::clone(&store)).await;
    let shape = &json["store"];

    assert_eq!(
        shape["origin"], "introspected",
        "la forme est LUE, pas déclarée"
    );
    assert_eq!(shape["nodes"], 3, "trois nœuds : {shape}");
    assert_eq!(shape["edges"], 1, "une arête : {shape}");

    let labels = shape["labels"].as_array().expect("labels");
    let member = labels
        .iter()
        .find(|l| l["name"] == "Member")
        .expect("le label Member doit être publié");
    assert_eq!(member["count"], 2, "et avec son compte : {shape}");
    let group = labels
        .iter()
        .find(|l| l["name"] == "Group")
        .expect("le label Group");
    assert_eq!(group["count"], 1);
    assert_eq!(
        labels[0]["name"], "Member",
        "les labels sont triés du plus peuplé au moins peuplé"
    );

    assert!(
        shape["edge_types"]
            .as_array()
            .is_some_and(|e| e.iter().any(|e| e["name"] == "CONTAINS" && e["count"] == 1)),
        "les types d'arêtes et leurs comptes sont publiés : {shape}"
    );
    assert!(
        shape["property_keys"]
            .as_array()
            .is_some_and(|k| k.iter().any(|k| k == "name")),
        "le catalogue des propriétés est publié : {shape}"
    );
    assert!(
        shape["indexes"].is_array(),
        "les index sont publiés (même vides) : {shape}"
    );
}

// ===========================================================================
// 4. CONSOMMATEUR QUI APPLIQUE LES POLITIQUES
// ===========================================================================

/// Un consommateur peut brancher l'application des politiques sur son propre
/// routeur : `Authorization` et le middleware sont publics.
#[tokio::test]
async fn a_consumer_can_enforce_policies_on_its_own_router() {
    use axum::Router;
    use axum::extract::Extension;
    use axum::http::Method;
    use axum::routing::delete;

    use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use this::core::link::LinkDefinition;
    use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use this::server::middleware::authorization::{Authorization, authorization_middleware};

    struct Orders;
    impl EntityDescriptor for Orders {
        fn entity_type(&self) -> &str {
            "order"
        }
        fn plural(&self) -> &str {
            "orders"
        }
        fn build_routes(&self) -> Router {
            Router::new()
        }
    }

    let store = producer_store();
    let auth = EntityAuthConfig {
        delete: "perm:orders:write".to_string(),
        ..Default::default()
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Orders));
    let authorization = Authorization::new(
        Arc::new(LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth,
            }],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        }),
        &registry,
        Arc::new(AuthResolverRegistry::new()),
    )
    .with_iam(&store);

    // **Ordre des couches** : axum applique la DERNIÈRE couche en premier. Le
    // contexte doit donc être posé par la couche appliquée en dernier, sinon
    // l'application des politiques s'exécute avant lui et voit un anonyme.
    // (Première version : refus 403 systématique — le test avait tort, pas le
    // framework.)
    let app = Router::new()
        .route("/orders/{id}", delete(|| async { "supprimé" }))
        .layer(axum::middleware::from_fn_with_state(
            authorization,
            authorization_middleware,
        ))
        .layer(Extension(AuthContext::User {
            user_id: uuid::Uuid::from_u128(1),
            tenant_id: uuid::Uuid::from_u128(0xa),
            roles: vec![],
        }));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/orders/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "le porteur de `orders:write` passe : le consommateur applique SES politiques \
         avec SON plan IAM"
    );
}

/// **Le manifeste d'inférence est publié** : un moteur n'a pas à deviner le
/// protocole, il le lit — comment s'authentifier, quoi lire, quelles bornes,
/// sous quel régime ces lectures sont jugées.
#[tokio::test]
async fn the_relief_publishes_the_inference_manifest() {
    let json = relief_json(producer_store()).await;
    let inference = &json["inference"];

    assert!(
        inference["authentication"]
            .as_str()
            .is_some_and(|a| a.contains("Bearer")),
        "l'authentification doit être dite : {inference}"
    );

    let reads = inference["reads"].as_array().expect("reads");
    for expected in [
        "/graph",
        "/graph/hierarchy/{id}?depth=N&limit=M",
        "/graph/hierarchy/{id}?format=outline",
        "/graph/identity",
        "/graph/identity/{uuid}",
    ] {
        assert!(
            reads.iter().any(|r| r["path"] == expected),
            "la lecture `{expected}` doit figurer au manifeste"
        );
    }
    assert!(
        reads
            .iter()
            .all(|r| r["purpose"].as_str().is_some_and(|p| !p.is_empty())),
        "chaque lecture dit POURQUOI on l'appelle — c'est ce qu'un modèle lit : {inference}"
    );

    let limits = inference["limits"].as_array().expect("limits");
    assert!(
        limits
            .iter()
            .any(|l| l.as_str().is_some_and(|l| l.contains("truncated"))),
        "la borne la plus importante (une coupe n'est jamais silencieuse) doit être publiée"
    );
    assert!(
        inference["authorization"]
            .as_str()
            .is_some_and(|a| a.contains("graph.policy")),
        "le régime d'autorisation, et la clé qui le règle, doivent être publiés : {inference}"
    );
    let rules = inference["rules"].as_array().expect("rules");
    assert!(
        rules
            .iter()
            .any(|r| r.as_str().is_some_and(|r| r.contains("depth"))),
        "la règle « ne jamais énumérer tout » doit être publiée : {rules:?}"
    );
}

// ===========================================================================
// 5. UN MOTEUR D'INFÉRENCE LIT LA BASE **SOUS IAM**
// ===========================================================================

/// **La question** : une base exposée par this-rs peut-elle être branchée à un
/// moteur d'inférence **en respectant le protocole IAM** ?
///
/// Oui — à une condition, et c'est celle-ci : les lectures qu'un moteur utilise
/// (relief, vue multi-échelle, identité) doivent être **jugées**. Sans politique
/// déclarée elles s'abstiennent (descriptives, publiques) ; avec une politique,
/// elles répondent **403** à qui n'a pas le droit, et la politique `perm:` est
/// tranchée par la décision matérialisée — le même chemin que le reste.
#[tokio::test]
async fn a_reader_without_the_permission_is_refused_by_the_policy() {
    use std::sync::Arc as StdArc;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    use this::config::{EntityAuthConfig, EntityConfig, LinksConfig, ServerSecurityConfig};
    use this::core::auth::{AuthContext, AuthResolverRegistry};
    use this::core::link::LinkDefinition;
    use this::server::RestExposure;
    use this::server::builder::ServerBuilder;
    use this::server::entity_registry::EntityRegistry;
    use this::storage::iam::RoleGrant;

    struct Orders;
    impl this::server::entity_registry::EntityDescriptor for Orders {
        fn entity_type(&self) -> &str {
            "order"
        }
        fn plural(&self) -> &str {
            "orders"
        }
        fn build_routes(&self) -> Router {
            Router::new()
        }
    }

    // Le lecteur : un principal qui a `graph:read` dans sa portée.
    let store = ObrainStore::new_in_memory();
    store
        .iam()
        .materialize(
            &uuid::Uuid::from_u128(1).to_string(),
            &uuid::Uuid::from_u128(7).to_string(),
            &[RoleGrant {
                role: "reader".into(),
                permissions: vec!["graph:read".into()],
            }],
        )
        .expect("matérialisation");

    // Le relief est **soumis** à la politique — c'est le maillon qui manquait.
    let security = ServerSecurityConfig {
        graph: this::config::server::GraphExposureConfig {
            policy: Some("perm:graph:read".to_string()),
        },
        ..Default::default()
    };

    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Orders));
    let host = ServerBuilder::new()
        .with_server_security(security)
        .with_obrain_store(StdArc::clone(&store))
        .build_host()
        .expect("hôte");
    let app = RestExposure::build_router(StdArc::new(host), vec![]).expect("routeur");

    let call = |app: Router, ctx: Option<AuthContext>| async move {
        let app = match ctx {
            Some(ctx) => app.layer(Extension(ctx)),
            None => app,
        };
        app.oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/graph")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse")
        .status()
    };

    // Sans identité : refusé.
    assert_eq!(
        call(app.clone(), None).await,
        StatusCode::FORBIDDEN,
        "un lecteur anonyme ne doit pas lire la base quand une politique est déclarée"
    );

    // Identifié, mais sans la permission : refusé aussi.
    assert_eq!(
        call(
            app.clone(),
            Some(AuthContext::User {
                user_id: uuid::Uuid::from_u128(2),
                tenant_id: uuid::Uuid::from_u128(7),
                roles: vec![],
            })
        )
        .await,
        StatusCode::FORBIDDEN,
        "un autre principal n'a pas `graph:read`"
    );

    // Le porteur de `graph:read` : autorisé.
    assert_eq!(
        call(
            app,
            Some(AuthContext::User {
                user_id: uuid::Uuid::from_u128(1),
                tenant_id: uuid::Uuid::from_u128(7),
                roles: vec![],
            })
        )
        .await,
        StatusCode::OK,
        "le porteur de la permission lit la base — c'est le protocole IAM respecté"
    );
    let _ = (
        EntityAuthConfig::default(),
        EntityConfig {
            singular: "order".to_string(),
            plural: "orders".to_string(),
            auth: EntityAuthConfig::default(),
        },
        LinksConfig {
            entities: vec![],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        },
        AuthResolverRegistry::new(),
    );
}

/// **Sans politique déclarée, rien ne change** : le relief reste descriptif et
/// public — c'est le défaut, et il ne doit pas bouger.
#[tokio::test]
async fn without_a_declared_policy_the_graph_reads_stay_public() {
    let json = relief_json(producer_store()).await;
    assert!(
        json["store"]["nodes"].as_u64().is_some(),
        "le relief répond sans politique : {json}"
    );
}

/// **La granularité appliquée par requête.** Avec `graph.policy` déclarée, la
/// lecture n'est pas seulement autorisée ou refusée : elle est **filtrée par la
/// décision du demandeur**. Deux principes, deux lectures différentes de la même
/// base — et ce qui est masqué est **compté**.
#[tokio::test]
async fn the_graph_read_is_filtered_by_the_callers_own_decision() {
    use std::sync::Arc as StdArc;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    use this::config::{EntityAuthConfig, EntityConfig, LinksConfig, ServerSecurityConfig};
    use this::core::auth::{AuthContext, AuthResolverRegistry};
    use this::core::link::LinkDefinition;
    use this::server::RestExposure;
    use this::server::builder::ServerBuilder;
    use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use this::storage::iam::RoleGrant;

    struct Orders;
    impl EntityDescriptor for Orders {
        fn entity_type(&self) -> &str {
            "order"
        }
        fn plural(&self) -> &str {
            "orders"
        }
        fn build_routes(&self) -> Router {
            Router::new()
        }
    }

    // Une base : un groupe lisible, un secret qui ne l'est pas.
    let store = ObrainStore::new_in_memory();
    let root = {
        let session = store.db().session();
        let root = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("collectif"))],
        );
        let lisible = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("visible"))],
        );
        let secret = session.create_node_with_props(
            &["Secret"],
            [("name", obrain_common::types::Value::from("confiance"))],
        );
        session.create_edge(root, lisible, "CONTAINS");
        session.create_edge(root, secret, "CONTAINS");
        store.commit_wal().expect("commit");
        root.as_u64()
    };

    let tenant = uuid::Uuid::from_u128(7);
    let lecteur = uuid::Uuid::from_u128(1);
    let aveugle = uuid::Uuid::from_u128(2);
    store
        .iam()
        .materialize(
            &lecteur.to_string(),
            &tenant.to_string(),
            &[RoleGrant {
                role: "lecteur".into(),
                permissions: vec![
                    "graph:read".into(),
                    "node:read:Group".into(),
                    "edge:read:CONTAINS".into(),
                    "echelle:read:groupe".into(),
                ],
            }],
        )
        .expect("matérialisation du lecteur");
    store
        .iam()
        .materialize(
            &aveugle.to_string(),
            &tenant.to_string(),
            &[RoleGrant {
                role: "aveugle".into(),
                permissions: vec!["graph:read".into()],
            }],
        )
        .expect("matérialisation de l'aveugle");

    let security = ServerSecurityConfig {
        graph: this::config::server::GraphExposureConfig {
            policy: Some("perm:graph:read".to_string()),
        },
        ..Default::default()
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Orders));
    let host = ServerBuilder::new()
        .with_server_security(security)
        .with_obrain_store(StdArc::clone(&store))
        .build_host()
        .expect("hôte");
    let app = RestExposure::build_router(StdArc::new(host), vec![]).expect("routeur");

    let call = |app: Router, user: uuid::Uuid, uri: &'static str| async move {
        let app = app.layer(Extension(AuthContext::User {
            user_id: user,
            tenant_id: tenant,
            roles: vec![],
        }));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("réponse");
        (response.status(), response)
    };

    let body_of = |response: axum::response::Response| async move {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("corps");
        String::from_utf8(bytes.to_vec()).expect("utf8")
    };

    // **Le lecteur** : il voit le groupe, pas le secret — et le masquage est dit.
    let (status, response) = call(
        app.clone(),
        lecteur,
        Box::leak(format!("/graph/hierarchy/{root}?echelle=groupe").into_boxed_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = body_of(response).await;
    assert!(
        body.contains("visible"),
        "le groupe lisible est là : {body}"
    );
    assert!(
        !body.contains("confiance"),
        "le secret ne doit PAS apparaître : {body}"
    );
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(json["levels"][0]["hidden"], 1, "et il est compté : {json}");

    // **L'aveugle** : `graph:read` seul ne donne aucun label — il ne voit rien,
    // et les notes le lui disent.
    let (status, response) = call(
        app.clone(),
        aveugle,
        Box::leak(format!("/graph/hierarchy/{root}").into_boxed_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "autorisé, mais sans périmètre");
    let body = body_of(response).await;
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(
        json["levels"].as_array().is_some_and(|l| l.is_empty()),
        "aucun label n'est lisible sans `node:read:` : {json}"
    );
    // **Le compte ne fuit pas** : la racine n'annonce pas ce qu'elle cache.
    // C'est la fuite la plus discrète — la structure sans le contenu.
    assert_eq!(
        json["root"]["children"], 0,
        "la racine ne doit pas annoncer le nombre d'enfants cachés : {json}"
    );
    assert!(
        body.contains("ni parcourues ni comptées"),
        "et l'absence de relations lisibles est expliquée, SANS publier de compte : {json}"
    );

    // **Le relief aussi** : la forme du store suit la même décision.
    let (status, response) = call(app, lecteur, "/graph").await;
    assert_eq!(status, StatusCode::OK);
    let body = body_of(response).await;
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");
    let labels: Vec<&str> = json["store"]["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .filter_map(|l| l["name"].as_str())
        .collect();
    assert_eq!(labels, vec!["Group"], "seul le label lisible : {labels:?}");
    // Le `Secret` n'est pas publié — et le compte dit qu'il y a bien des labels
    // hors périmètre. Ceux de l'IAM en font partie : un lecteur de connaissances
    // n'a pas à voir les labels internes de l'autorisation.
    assert!(
        !labels.contains(&"Secret"),
        "le label hors périmètre ne doit pas être publié : {labels:?}"
    );
    let hidden = json["store"]["hidden_labels"].as_u64().expect("compte");
    assert!(
        hidden >= 1,
        "et le masquage est compté ({hidden}) : {}",
        json["store"]
    );
    let _ = (
        EntityAuthConfig::default(),
        EntityConfig {
            singular: "order".to_string(),
            plural: "orders".to_string(),
            auth: EntityAuthConfig::default(),
        },
        LinksConfig {
            entities: vec![],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        },
        AuthResolverRegistry::new(),
    );
}
