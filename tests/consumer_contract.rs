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
    let app = this::graph::graph_routes(this::graph::GraphState { store, identity });
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
