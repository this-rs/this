//! Adoption **native** par un projet qui existe déjà.
//!
//! Ce fichier décrit le parcours réel d'un projet plus ancien : il a ses
//! entités, son modèle d'authentification, sa base — et il veut gagner
//! l'identité canonique, l'IAM matérialisé et l'application des politiques
//! **sans réécrire** ce qu'il a.
//!
//! Il prouve les quatre propriétés qui rendent cette adoption possible :
//!
//! 1. **un seul appel** branche obrain (relief, identité, plan IAM, bail) —
//!    `ServerBuilder::with_obrain_store` ;
//! 2. **l'observation d'abord** (`authorization_report_only`) : les refus sont
//!    calculés, journalisés et publiés dans la réponse, mais **rien n'est
//!    bloqué** ; c'est ce qui permet d'adopter en production sans la casser ;
//! 3. **l'IAM épouse le modèle d'identité du projet** (`with_principal_mapping`) :
//!    une décision matérialisée sous `account:…`/`org:…` est trouvée, alors que
//!    le mappage par défaut de this-rs ne la voit pas ;
//! 4. **une base écrite par un autre outil est lue** : des nœuds à propriétés
//!    plates (sans `_uuid`) sont résolus par identité dérivée.

#![cfg(feature = "obrain")]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Method, Request, StatusCode};
use axum::routing::{delete, get};
use serde_json::Value as Json;
use tower::ServiceExt;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig, ServerSecurityConfig};
use this::core::auth::AuthContext;
use this::core::link::LinkDefinition;
use this::core::module::Module;
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::storage::ObrainStore;
use this::storage::iam::RoleGrant;

// ---------------------------------------------------------------------------
// Le projet « existant » : ses entités, ses routes, son modèle d'auth
// ---------------------------------------------------------------------------

/// Le descripteur d'entité du projet — il monte **ses** routes, comme avant.
struct OrderDescriptor;

impl EntityDescriptor for OrderDescriptor {
    fn entity_type(&self) -> &str {
        "order"
    }
    fn plural(&self) -> &str {
        "orders"
    }
    fn build_routes(&self) -> Router {
        Router::new()
            .route("/orders", get(|| async { "[]" }))
            .route("/orders/{id}", delete(|| async { "supprimé" }))
    }
}

/// Le module du projet : il déclare ses politiques, comme il le ferait dans son
/// `links.yaml` d'origine.
struct LegacyModule {
    delete_policy: String,
}

impl Module for LegacyModule {
    fn name(&self) -> &str {
        "legacy"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["order"]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        let auth = EntityAuthConfig {
            list: "authenticated".to_string(),
            delete: self.delete_policy.clone(),
            ..Default::default()
        };
        Ok(LinksConfig {
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
        })
    }
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(OrderDescriptor));
    }
}

/// Le mappage d'identité **du projet** : il nomme ses principaux `account:<id>`
/// et ses portées `org:<id>`. C'est ce qui rend l'IAM natif chez lui.
fn legacy_mapping() -> impl Fn(&AuthContext) -> Option<(String, String)> + Send + Sync + 'static {
    |ctx: &AuthContext| match ctx {
        AuthContext::User {
            user_id, tenant_id, ..
        } => Some((format!("account:{user_id}"), format!("org:{tenant_id}"))),
        _ => None,
    }
}

/// Un contexte d'utilisateur du projet.
fn user(tenant: u128, id: u128) -> AuthContext {
    AuthContext::User {
        user_id: uuid::Uuid::from_u128(id),
        tenant_id: uuid::Uuid::from_u128(tenant),
        roles: vec![],
        scopes: Vec::new(),
    }
}

/// Construit l'hôte du projet, avec ou sans observation.
fn host(
    store: Arc<ObrainStore>,
    delete_policy: &str,
    report_only: bool,
) -> anyhow::Result<Arc<this::server::host::ServerHost>> {
    host_with_graph_policy(store, delete_policy, report_only, None)
}

/// Le même hôte, avec une **politique de lecture de graphe** déclarée.
///
/// Depuis que l'abstention est un refus (2026-09-13), `/graph*` sans
/// `graph.policy` est refusé — **même à un appelant authentifié** : ce n'est pas
/// une affaire d'identité, c'est une garde absente. Lire le relief sous une
/// couche armée suppose donc de la **déclarer** — le geste que le plan de
/// déploiement réclame déjà d'un projet en mode `enforce`.
fn host_with_graph_policy(
    store: Arc<ObrainStore>,
    delete_policy: &str,
    report_only: bool,
    graph_policy: Option<&str>,
) -> anyhow::Result<Arc<this::server::host::ServerHost>> {
    let mut security = ServerSecurityConfig::default();
    security.graph.policy = graph_policy.map(str::to_string);
    let builder = ServerBuilder::new()
        .with_server_security(security)
        .register_module(LegacyModule {
            delete_policy: delete_policy.to_string(),
        })?
        .with_obrain_store(store)
        .with_principal_mapping(legacy_mapping());
    let builder = if report_only {
        builder.authorization_report_only()
    } else {
        builder.with_authorization()
    };
    Ok(Arc::new(builder.build_host()?))
}

/// Envoie une requête avec le contexte du projet attaché.
async fn call(
    app: Router,
    context: Option<AuthContext>,
    method: Method,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap) {
    let app = match context {
        Some(ctx) => app.layer(Extension(ctx)),
        None => app,
    };
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    (response.status(), response.headers().clone())
}

// ---------------------------------------------------------------------------
// 1. Un seul appel branche obrain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_call_wires_obrain_identity_iam_and_enforcement() {
    let store = ObrainStore::new_in_memory();

    // ── 1) Le chemin clé en main, **tel quel** ───────────────────────────────
    let host = host(Arc::clone(&store), "authenticated", false).expect("hôte");

    // Le store est attaché, et l'application des politiques est armée sans
    // qu'on l'ait demandée.
    assert!(host.graph_store().is_some());
    assert!(host.authorize);

    let app = RestExposure::build_router(Arc::clone(&host), vec![]).expect("routeur");

    // Les lectures de graphe n'ont **pas** de politique : depuis que
    // l'abstention est un refus (2026-09-13), elles sont refusées — le relief
    // n'est plus public par accident. C'est le prix, assumé, du fail-closed :
    // lire la base demande de le DÉCLARER (étape 2 ci-dessous).
    let (status, _) = call(app.clone(), None, Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "sans `graph.policy` déclarée, l'abstention refuse les lectures de graphe"
    );

    assert_eq!(
        call(app, None, Method::DELETE, "/orders/1").await.0,
        StatusCode::FORBIDDEN,
        "une route d'entité sans contexte est refusée"
    );

    // ── 2) La lecture DÉCLARÉE : le relief et l'identité sont servis ─────────
    let host = host_with_graph_policy(
        Arc::clone(&store),
        "authenticated",
        false,
        Some("authenticated"),
    )
    .expect("hôte");
    let app = RestExposure::build_router(host, vec![]).expect("routeur");

    let (status, _) = call(app.clone(), Some(user(9, 1)), Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "politique déclarée + appelant identifié : le relief d'obrain est publié"
    );

    let (status, _) = call(app, Some(user(9, 1)), Method::GET, "/graph/identity").await;
    assert_eq!(status, StatusCode::OK, "le contrat d'identité est publié");
}

// ---------------------------------------------------------------------------
// 2. L'observation d'abord : rien n'est bloqué, tout est publié
// ---------------------------------------------------------------------------

#[tokio::test]
async fn report_only_adopts_without_breaking_the_running_service() {
    let store = ObrainStore::new_in_memory();
    let host = host(Arc::clone(&store), "admin_only", true).expect("hôte");
    let app = RestExposure::build_router(host, vec![]).expect("routeur");

    // Un utilisateur ordinaire : il SERAIT refusé par `admin_only`.
    let (status, headers) = call(app.clone(), Some(user(7, 42)), Method::DELETE, "/orders/1").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "en observation, la requête passe — c'est ce qui permet d'adopter en production"
    );
    assert_eq!(
        headers.get("x-this-rs-authz").map(|v| v.to_str().unwrap()),
        Some("observed-deny"),
        "la réponse publie le refus observé, pour qu'un proxy puisse les compter"
    );
    assert_eq!(
        headers
            .get("x-this-rs-authz-policy")
            .map(|v| v.to_str().unwrap()),
        Some("admin_only")
    );

    // Une requête qui SERAIT autorisée ne publie rien : pas de bruit.
    let (status, headers) = call(app, Some(user(7, 42)), Method::GET, "/orders").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers.get("x-this-rs-authz").is_none(),
        "aucun en-tête quand la décision est favorable"
    );
}

/// Le contraste, dans un seul test : mêmes politiques, deux modes.
#[tokio::test]
async fn the_same_policy_observes_then_blocks() {
    let store = ObrainStore::new_in_memory();

    let observing = RestExposure::build_router(
        host(Arc::clone(&store), "admin_only", true).expect("hôte"),
        vec![],
    )
    .expect("routeur");
    assert_eq!(
        call(observing, Some(user(7, 42)), Method::DELETE, "/orders/1")
            .await
            .0,
        StatusCode::OK
    );

    let armed = RestExposure::build_router(
        host(Arc::clone(&store), "admin_only", false).expect("hôte"),
        vec![],
    )
    .expect("routeur");
    assert_eq!(
        call(armed, Some(user(7, 42)), Method::DELETE, "/orders/1")
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

// ---------------------------------------------------------------------------
// 3. L'IAM épouse le modèle d'identité du projet
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_iam_is_native_to_the_projects_own_identity_model() {
    let store = ObrainStore::new_in_memory();
    let tenant = uuid::Uuid::from_u128(7);
    let account = uuid::Uuid::from_u128(42);

    // La décision est matérialisée sous les noms **du projet**.
    store
        .iam()
        .materialize(
            &format!("account:{account}"),
            &format!("org:{tenant}"),
            &[RoleGrant {
                role: "operator".into(),
                permissions: vec!["orders:write".into()],
            }],
        )
        .expect("matérialisation");

    let host = host(Arc::clone(&store), "perm:orders:write", false).expect("hôte");
    let app = RestExposure::build_router(host, vec![]).expect("routeur");

    // Le principal du projet est trouvé par le mappage qu'il a fourni…
    assert_eq!(
        call(app.clone(), Some(user(7, 42)), Method::DELETE, "/orders/1")
            .await
            .0,
        StatusCode::OK,
        "la décision `account:…`/`org:…` doit être trouvée par le mappage du projet"
    );

    // …et un AUTRE locataire ne bénéficie de rien (cloisonnement).
    assert_eq!(
        call(app, Some(user(8, 42)), Method::DELETE, "/orders/1")
            .await
            .0,
        StatusCode::FORBIDDEN,
        "aucune décision pour ce locataire : refus"
    );
}

#[tokio::test]
async fn without_a_mapping_a_foreign_identity_model_is_refused_not_guessed() {
    let store = ObrainStore::new_in_memory();
    let tenant = uuid::Uuid::from_u128(7);
    let account = uuid::Uuid::from_u128(42);
    store
        .iam()
        .materialize(
            &format!("account:{account}"),
            &format!("org:{tenant}"),
            &[RoleGrant {
                role: "operator".into(),
                permissions: vec!["orders:write".into()],
            }],
        )
        .expect("matérialisation");

    // Sans mappage : le modèle de this-rs cherche `(user_id, tenant_id)`.
    let host = Arc::new(
        ServerBuilder::new()
            .register_module(LegacyModule {
                delete_policy: "perm:orders:write".to_string(),
            })
            .expect("module")
            .with_obrain_store(Arc::clone(&store))
            .build_host()
            .expect("hôte"),
    );
    let app = RestExposure::build_router(host, vec![]).expect("routeur");
    assert_eq!(
        call(app, Some(user(7, 42)), Method::DELETE, "/orders/1")
            .await
            .0,
        StatusCode::FORBIDDEN,
        "sans mappage, l'identité étrangère n'est pas devinée : refus (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// 4. Une base écrite par un autre outil est lue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_base_written_by_another_tool_is_adopted_through_the_turnkey_path() {
    let store = ObrainStore::new_in_memory();

    // Des nœuds à propriétés **plates** : aucun `_uuid`, c'est une base écrite
    // par un autre outil (le cas mesuré sur les vraies bases d'aletheia).
    {
        let session = store.db().session();
        for name in ["entity:a", "entity:b"] {
            session.create_node_with_props(
                &["Page"],
                [("qualified_name", obrain_common::types::Value::from(name))],
            );
        }
    }

    // La lecture est **déclarée** (`graph.policy`) et l'appelant est identifié :
    // depuis que l'abstention est un refus, un relief sans politique déclarée
    // serait refusé même avec un store attaché.
    let host = host_with_graph_policy(
        Arc::clone(&store),
        "authenticated",
        false,
        Some("authenticated"),
    )
    .expect("hôte");
    let app = RestExposure::build_router(host, vec![]).expect("routeur");
    let app = app.layer(Extension(user(9, 1)));

    // Le comptage borné passe par l'identité dérivée : les deux nœuds sont
    // résolus alors qu'aucun ne porte `_uuid`.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/graph/identity?counts=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("corps");
    let json: Json = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(
        json["counts"]["total_resolved"], 2,
        "deux nœuds plats doivent être résolus : {json}"
    );
    assert_eq!(json["counts"]["labels"][0]["by_convention"], 0);
    assert_eq!(json["counts"]["labels"][0]["by_derivation"], 2);
}

// ---------------------------------------------------------------------------
// 5. Le bail : un store inscriptible est arbitré, l'observation ne change rien
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_writable_store_is_fenced_even_in_report_only() {
    let dir = tempfile::tempdir().expect("répertoire");
    let store = ObrainStore::open(dir.path()).expect("store");

    let host = host(Arc::clone(&store), "authenticated", true).expect("hôte");
    assert!(
        store.lease_info().is_some(),
        "un store inscriptible prend un bail, même en observation : l'arbitrage \
         d'écriture n'a rien à voir avec le mode d'autorisation"
    );
    assert!(host.authorize_report_only);
}
