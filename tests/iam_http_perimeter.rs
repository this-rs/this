#![cfg(feature = "obrain")]
//! Autorisation en HTTP : le mode par défaut, et le branchement clé en main.
//!
//! # Ce que ce fichier refuse de croire sur parole
//!
//! Trois affirmations du contrat (`docs/contrats-p1-p2-identite-bail.md`) ne
//! sont vérifiables qu'**en HTTP, sur un routeur réellement construit** :
//!
//! 1. **Le mode par défaut arme l'autorisation.** `ServerBuilder::new()` puis
//!    `build_host()` doit rendre `host.authorize == true`, et une politique
//!    déclarée (`auth:`) doit produire un **403** sur une route d'entité — sans
//!    qu'aucun appel n'ait été fait pour l'armer.
//! 2. **`with_obrain_store` suffit.** Un seul appel doit donner le relief
//!    (`GET /graph`), l'identité canonique (`GET /graph/identity`), le plan IAM
//!    matérialisé, l'autorisation armée, et le **bail d'écriture** si le store
//!    est inscriptible — mais **pas** sur un store ouvert en lecture seule.
//!    ⚠️ **Lire le relief sous une couche armée demande de DÉCLARER
//!    `graph.policy`** : voir le point 3, et `with_graph_policy` plus bas.
//! 3. **L'abstention EST un refus** (contrat du 2026-09-13, cf.
//!    `tests/abstention_refusee.rs`). Ce fichier affirmait l'inverse — que
//!    `/health`, `/graph` et une route personnalisée n'étaient *jamais* refusés
//!    par la couche. C'était vrai jusqu'à la faille 4-segments : une route non
//!    reconnue traversait un service armé sans décision. Aujourd'hui, seuls les
//!    chemins **déclarés publics** passent (`/health`, `/healthz`, `/auth/…`) ;
//!    partout ailleurs, et notamment sur les lectures de graphe sans
//!    `graph.policy`, la réponse est **403 `deny_abstain`**.
//!
//! # Ce que ça change pour lire un graphe
//!
//! Un store attaché ne rend plus le relief public par accident : il faut
//! **déclarer** la politique de lecture (`security.graph.policy`), exactement ce
//! que le plan de déploiement réclame déjà (« mode `enforce` sans
//! `graph.policy` »). Les tests qui lisent le document le font donc avec
//! [`with_graph_policy`] **et** un contexte d'authentification : c'est le
//! parcours réel d'un consommateur.
//!
//! # Comment le contexte d'authentification est injecté
//!
//! Par `axum::Extension(AuthContext)` posée **sur le routeur construit** — donc
//! en couche *extérieure*, exécutée **avant** la couche d'autorisation, qui lit
//! l'extension. L'absence de contexte est un cas de test à part entière : elle
//! doit valoir `Anonymous` (fail-closed), jamais « on laisse passer ».
//!
//! # Deux défauts constatés vivent ici
//!
//! Les deux sont écrits, marqués `#[ignore]` et nommés :
//! `without_authorization_actually_disarms_the_enforcement` et
//! `a_garbage_policy_refuses_instead_of_degrading_to_authenticated`. Ils
//! **échouent** quand on les exécute (`-- --ignored`) : c'est la preuve.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use tower::ServiceExt;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::AuthContext;
use this::core::identity::ENTITY_ID_SCHEME;
use this::core::link::LinkDefinition;
use this::core::module::{EntityCreator, EntityFetcher, Module};
use this::graph::relief::RELIEF_SCHEMA;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::host::ServerHost;
use this::server::{RestExposure, ServerBuilder};
use this::storage::iam::RoleGrant;
use this::storage::{InMemoryLinkService, ObrainStore};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Le bac à sable : un module, ses routes, ses politiques
// ---------------------------------------------------------------------------

/// Un identifiant d'entité **réellement parsable en Uuid** : les routes de lien
/// réelles du framework désérialisent `{entity_id}` en `Uuid`, un `42` littéral
/// rendrait un 400 qui masquerait la couche d'autorisation.
const ORDER_ID: &str = "11111111-1111-4111-8111-111111111111";
const TARGET_ID: &str = "22222222-2222-4222-8222-222222222222";

/// Le module minimal : une entité `order` (pluriel `orders`), ses politiques,
/// et un lien `order → product` dont le nom de route publie la forme à trois
/// segments que l'autorisation doit résoudre en `list_links`.
struct OrdersModule {
    auth: EntityAuthConfig,
}

impl OrdersModule {
    fn new(auth: EntityAuthConfig) -> Self {
        Self { auth }
    }
}

/// `order → product` sous le nom de route `items`.
fn items_link() -> LinkDefinition {
    LinkDefinition {
        link_type: "item".to_string(),
        source_type: "order".to_string(),
        target_type: "product".to_string(),
        forward_route_name: "items".to_string(),
        reverse_route_name: "orders-of".to_string(),
        description: None,
        required_fields: None,
        auth: None,
    }
}

impl Module for OrdersModule {
    fn name(&self) -> &str {
        "orders"
    }

    fn entity_types(&self) -> Vec<&str> {
        vec!["order"]
    }

    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: self.auth.clone(),
            }],
            links: vec![items_link()],
            validation_rules: None,
            events: None,
            sinks: None,
            // Surtout PAS de section `auth` globale : elle auto-câblerait un
            // fournisseur d'authentification et brouillerait ce qu'on mesure.
            auth: None,
        })
    }

    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(OrdersDescriptor));
    }

    fn get_entity_fetcher(&self, _entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        None
    }

    fn get_entity_creator(&self, _entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        None
    }
}

/// Les routes CRUD réelles de l'entité — celles que la couche va juger.
struct OrdersDescriptor;

impl EntityDescriptor for OrdersDescriptor {
    fn entity_type(&self) -> &str {
        "order"
    }

    fn plural(&self) -> &str {
        "orders"
    }

    fn build_routes(&self) -> Router {
        Router::new()
            .route("/orders", get(list_orders).post(create_order))
            .route(
                "/orders/{id}",
                get(get_order)
                    .put(update_order)
                    .patch(update_order)
                    .delete(delete_order),
            )
    }
}

async fn list_orders() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "data": [] })))
}

async fn get_order() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "id": "one" })))
}

async fn create_order() -> impl IntoResponse {
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "created": true })),
    )
}

async fn update_order() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "updated": true })))
}

async fn delete_order() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "deleted": true })))
}

async fn ping() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "pong": true })))
}

/// Les deux formes d'écriture de lien que le routeur du framework ne monte pas
/// lui-même (sa route de lien s'arrête à quatre segments) et que l'autorisation,
/// elle, sait parfaitement résoudre (`create_link` / `delete_link`).
///
/// Aucun recouvrement avec les routes de lien réelles : cinq segments contre
/// trois et quatre — `RestExposure::build_router` fusionne sans paniquer.
fn link_mutation_routes() -> Router {
    Router::new().route(
        "/orders/{id}/items/product/{target}",
        post(create_item_link).delete(delete_item_link),
    )
}

async fn create_item_link() -> impl IntoResponse {
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "linked": true })),
    )
}

async fn delete_item_link() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "unlinked": true })),
    )
}

// ---------------------------------------------------------------------------
// Construction des hôtes et des routeurs
// ---------------------------------------------------------------------------

/// Un hôte bâti **sans jamais appeler `with_authorization()`** : c'est le mode
/// par défaut, celui qu'on veut vérifier.
fn host_with(auth: EntityAuthConfig) -> anyhow::Result<ServerHost> {
    ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(OrdersModule::new(auth))?
        .build_host()
}

/// Le chemin clé en main : brancher un store, et rien d'autre.
fn host_with_store(auth: EntityAuthConfig, store: Arc<ObrainStore>) -> anyhow::Result<ServerHost> {
    ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(OrdersModule::new(auth))?
        .with_obrain_store(store)
        .build_host()
}

fn router(host: ServerHost) -> Router {
    router_with(host, vec![link_mutation_routes()])
}

/// **Déclare `graph.policy` sur un hôte déjà bâti.**
///
/// Depuis que l'abstention est un refus, un `/graph*` sans politique déclarée
/// rend 403 — **même à un appelant identifié** : ce n'est pas une affaire
/// d'identité, la route n'a pas de garde. Lire le relief suppose donc de la
/// déclarer ; c'est le geste que `this deploy` réclame d'un projet en mode
/// `enforce`, et c'est la seule façon de vérifier le contenu du document
/// **sous la couche armée** plutôt qu'avec la couche retirée.
fn with_graph_policy(mut host: ServerHost, policy: &str) -> ServerHost {
    let mut security = (*host.security).clone();
    security.graph.policy = Some(policy.to_string());
    host.security = Arc::new(security);
    host
}

fn router_with(host: ServerHost, custom: Vec<Router>) -> Router {
    RestExposure::build_router(Arc::new(host), custom).expect("le routeur REST doit se construire")
}

/// Pose le contexte d'authentification **au-dessus** des couches internes : il
/// s'exécute donc avant la couche d'autorisation, qui le lit dans les extensions.
fn as_context(router: Router, context: AuthContext) -> Router {
    router.layer(Extension(context))
}

/// Exécute une requête et rend `(statut, corps JSON)`.
async fn call(router: &Router, method: Method, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("requête"),
        )
        .await
        .expect("réponse");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("corps");
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

// ---------------------------------------------------------------------------
// Les contextes d'authentification
// ---------------------------------------------------------------------------

fn ctx_user() -> AuthContext {
    AuthContext::User {
        user_id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(9),
        roles: vec![],
    }
}

fn ctx_user_with(role: &str) -> AuthContext {
    AuthContext::User {
        user_id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(9),
        roles: vec![role.to_string()],
    }
}

fn ctx_owner() -> AuthContext {
    AuthContext::Owner {
        user_id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(9),
        resource_id: Uuid::from_u128(42),
        resource_type: "order".to_string(),
    }
}

fn ctx_service() -> AuthContext {
    AuthContext::Service {
        service_name: "billing".to_string(),
        tenant_id: Some(Uuid::from_u128(9)),
    }
}

fn ctx_admin() -> AuthContext {
    AuthContext::Admin {
        admin_id: Uuid::from_u128(7),
    }
}

// ===========================================================================
// 1. LE MODE PAR DÉFAUT
// ===========================================================================

/// **La propriété qui porte tout le reste** : un hôte construit sans un seul
/// appel d'armement applique quand même ses politiques déclarées.
#[tokio::test]
async fn a_host_built_without_any_authorization_call_still_enforces_its_declared_policies() {
    let auth = EntityAuthConfig {
        list: "admin_only".to_string(),
        ..Default::default()
    };

    let host = host_with(auth.clone()).expect("hôte");

    assert!(
        host.authorize,
        "`ServerBuilder::new()` ⇒ `host.authorize == true` : l'application des \
         politiques est ARMÉE PAR DÉFAUT, sans aucun appel"
    );

    // Aucun contexte posé ⇒ anonyme ⇒ refus.
    let (status, body) = call(&router(host), Method::GET, "/orders").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "aucun contexte ⇒ anonyme ⇒ `admin_only` refuse : {body}"
    );

    // La même politique, sur un hôte bâti de la même façon, mais avec un
    // contexte qui la satisfait : la requête passe.
    let app = as_context(router(host_with(auth).expect("hôte")), ctx_admin());
    let (status, _) = call(&app, Method::GET, "/orders").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "un administrateur satisfait `admin_only` : c'est bien la politique qui décide, \
         et non un refus systématique"
    );
}

// ===========================================================================
// 2. TOUTES LES OPÉRATIONS — AUTORISÉ / REFUSÉ
// ===========================================================================

/// Les huit politiques du périmètre, une par opération, choisies pour que le
/// contexte « autorisé » et le contexte « refusé » soient **opposés** sur la
/// même politique.
fn perimeter_policies() -> EntityAuthConfig {
    EntityAuthConfig {
        list: "authenticated".to_string(),
        get: "role:reader".to_string(),
        create: "admin_only".to_string(),
        update: "owner".to_string(),
        delete: "service_only".to_string(),
        list_links: "role:linker".to_string(),
        create_link: "role:writer".to_string(),
        delete_link: "admin_only".to_string(),
        ..Default::default()
    }
}

struct Operation {
    name: &'static str,
    method: Method,
    path: String,
    allowed_status: StatusCode,
    allowed: AuthContext,
    denied: AuthContext,
}

fn perimeter() -> Vec<Operation> {
    vec![
        Operation {
            name: "list",
            method: Method::GET,
            path: "/orders".to_string(),
            allowed_status: StatusCode::OK,
            allowed: ctx_user(),
            denied: AuthContext::Anonymous,
        },
        Operation {
            name: "get",
            method: Method::GET,
            path: format!("/orders/{ORDER_ID}"),
            allowed_status: StatusCode::OK,
            allowed: ctx_user_with("reader"),
            denied: ctx_user(),
        },
        Operation {
            name: "create",
            method: Method::POST,
            path: "/orders".to_string(),
            allowed_status: StatusCode::CREATED,
            allowed: ctx_admin(),
            denied: ctx_user(),
        },
        Operation {
            name: "update (PUT)",
            method: Method::PUT,
            path: format!("/orders/{ORDER_ID}"),
            allowed_status: StatusCode::OK,
            allowed: ctx_owner(),
            denied: ctx_user(),
        },
        Operation {
            name: "update (PATCH)",
            method: Method::PATCH,
            path: format!("/orders/{ORDER_ID}"),
            allowed_status: StatusCode::OK,
            allowed: ctx_owner(),
            denied: ctx_user(),
        },
        Operation {
            name: "delete",
            method: Method::DELETE,
            path: format!("/orders/{ORDER_ID}"),
            allowed_status: StatusCode::OK,
            allowed: ctx_service(),
            denied: ctx_user(),
        },
        Operation {
            name: "list_links",
            method: Method::GET,
            path: format!("/orders/{ORDER_ID}/items"),
            allowed_status: StatusCode::OK,
            allowed: ctx_user_with("linker"),
            denied: ctx_user(),
        },
        Operation {
            name: "create_link",
            method: Method::POST,
            path: format!("/orders/{ORDER_ID}/items/product/{TARGET_ID}"),
            allowed_status: StatusCode::CREATED,
            allowed: ctx_user_with("writer"),
            denied: AuthContext::Anonymous,
        },
        Operation {
            name: "delete_link",
            method: Method::DELETE,
            path: format!("/orders/{ORDER_ID}/items/product/{TARGET_ID}"),
            allowed_status: StatusCode::OK,
            allowed: ctx_admin(),
            denied: ctx_owner(),
        },
    ]
}

#[tokio::test]
async fn every_operation_lets_through_the_context_that_satisfies_its_policy() {
    let base = router(host_with(perimeter_policies()).expect("hôte"));

    for op in perimeter() {
        let app = as_context(base.clone(), op.allowed.clone());
        let (status, body) = call(&app, op.method.clone(), &op.path).await;
        assert_eq!(
            status, op.allowed_status,
            "{} : un contexte qui satisfait la politique doit atteindre le handler — {} {} → {body}",
            op.name, op.method, op.path
        );
    }
}

#[tokio::test]
async fn every_operation_refuses_with_403_the_context_that_does_not() {
    let base = router(host_with(perimeter_policies()).expect("hôte"));

    for op in perimeter() {
        let app = as_context(base.clone(), op.denied.clone());
        let (status, body) = call(&app, op.method.clone(), &op.path).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{} : {} {} doit être refusé — {body}",
            op.name,
            op.method,
            op.path
        );
        // Le refus DIT ce qui a été appliqué : une portée, une politique, une
        // provenance. Un 403 muet obligerait l'exploitant à deviner.
        assert_eq!(body["error"], "forbidden");
        assert_eq!(
            body["scope"],
            format!("order · {}", op.name.split(' ').next().unwrap()),
            "la portée publiée doit nommer l'entité et l'opération"
        );
        assert_eq!(
            body["origin"], "declared",
            "les politiques viennent de la configuration : la provenance doit le dire"
        );
    }
}

/// Le corps d'un refus porte la **politique** appliquée, telle qu'écrite.
#[tokio::test]
async fn a_refusal_publishes_the_policy_that_was_applied() {
    let app = as_context(
        router(host_with(perimeter_policies()).expect("hôte")),
        ctx_user(),
    );
    let (status, body) = call(&app, Method::GET, &format!("/orders/{ORDER_ID}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["policy"], "role:reader");
    assert_eq!(body["scope"], "order · get");
}

// ===========================================================================
// 3. LE CONTEXTE D'AUTHENTIFICATION
// ===========================================================================

/// Toutes les variantes de contexte, jugées par la même politique
/// `authenticated` : seule la variante anonyme est refusée.
#[tokio::test]
async fn every_non_anonymous_context_satisfies_an_authenticated_policy() {
    let auth = EntityAuthConfig {
        list: "authenticated".to_string(),
        ..Default::default()
    };
    let base = router(host_with(auth).expect("hôte"));

    let accepted = [
        ("User sans rôle", ctx_user()),
        ("User avec rôle", ctx_user_with("anything")),
        ("Owner", ctx_owner()),
        ("Service", ctx_service()),
        ("Admin", ctx_admin()),
    ];
    for (label, context) in accepted {
        let (status, body) = call(&as_context(base.clone(), context), Method::GET, "/orders").await;
        assert_eq!(status, StatusCode::OK, "{label} est authentifié — {body}");
    }

    let (status, _) = call(
        &as_context(base.clone(), AuthContext::Anonymous),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "un contexte anonyme explicite n'est pas authentifié"
    );
}

/// **Fail-closed** : pas de contexte du tout vaut anonyme — jamais « on laisse
/// passer faute de savoir qui c'est ».
#[tokio::test]
async fn an_absent_context_is_treated_as_anonymous_and_refused() {
    let strict = EntityAuthConfig {
        list: "authenticated".to_string(),
        ..Default::default()
    };
    let (status, body) = call(
        &router(host_with(strict).expect("hôte")),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "contexte absent ⇒ anonyme ⇒ 403 : {body}"
    );

    // Le pendant qui prouve que c'est bien le contexte qui manque, et non la
    // route : la MÊME requête passe dès qu'un contexte est posé.
    let permissive = EntityAuthConfig {
        list: "public".to_string(),
        ..Default::default()
    };
    let (status, _) = call(
        &router(host_with(permissive).expect("hôte")),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`public` passe sans contexte : la couche n'exige pas d'identité, elle exige une politique satisfaite"
    );
}

// ===========================================================================
// 4. LES POLITIQUES
// ===========================================================================

#[tokio::test]
async fn a_public_policy_passes_without_any_context() {
    let auth = EntityAuthConfig {
        list: "public".to_string(),
        ..Default::default()
    };
    let (status, _) = call(
        &router(host_with(auth).expect("hôte")),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_role_policy_reads_the_roles_of_the_context() {
    let auth = EntityAuthConfig {
        list: "role:auditor".to_string(),
        ..Default::default()
    };
    let base = router(host_with(auth).expect("hôte"));

    let (status, _) = call(
        &as_context(base.clone(), ctx_user_with("auditor")),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "le rôle demandé est porté");

    let (status, _) = call(
        &as_context(base.clone(), ctx_user_with("reader")),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "un AUTRE rôle ne satisfait pas `role:auditor`"
    );

    let (status, _) = call(
        &as_context(base.clone(), ctx_user()),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "aucun rôle ⇒ refus");
}

/// L'asymétrie réelle du vocabulaire, épinglée ici pour qu'elle ne se découvre
/// pas en production : `HasRole` n'inspecte que `AuthContext::User`. Un
/// administrateur **ne satisfait donc pas** `role:admin` — il satisfait
/// `admin_only`, qui est la politique faite pour lui.
#[tokio::test]
async fn role_and_admin_only_are_not_interchangeable() {
    let role = EntityAuthConfig {
        list: "role:admin".to_string(),
        ..Default::default()
    };
    let base = router(host_with(role).expect("hôte"));

    let (status, _) = call(
        &as_context(base.clone(), ctx_user_with("admin")),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "un User portant le rôle `admin` passe"
    );

    let (status, _) = call(&as_context(base, ctx_admin()), Method::GET, "/orders").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`role:admin` n'est PAS satisfait par `AuthContext::Admin` : les rôles \
         ne sont lus que sur `AuthContext::User` (src/core/auth/mod.rs:371-374)"
    );

    let admin_only = EntityAuthConfig {
        list: "admin_only".to_string(),
        ..Default::default()
    };
    let base = router(host_with(admin_only).expect("hôte"));
    let (status, _) = call(
        &as_context(base.clone(), ctx_admin()),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`admin_only` est satisfait par Admin"
    );
    let (status, _) = call(
        &as_context(base, ctx_user_with("admin")),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`admin_only` n'est PAS satisfait par un User, même portant le rôle `admin`"
    );
}

#[tokio::test]
async fn an_owner_policy_needs_an_owner_context() {
    let auth = EntityAuthConfig {
        update: "owner".to_string(),
        ..Default::default()
    };
    let base = router(host_with(auth).expect("hôte"));
    let path = format!("/orders/{ORDER_ID}");

    let (status, _) = call(&as_context(base.clone(), ctx_owner()), Method::PUT, &path).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "un contexte Owner satisfait `owner`"
    );

    let (status, _) = call(&as_context(base.clone(), ctx_user()), Method::PUT, &path).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "un utilisateur authentifié n'est pas propriétaire"
    );

    let (status, _) = call(&as_context(base, ctx_admin()), Method::PUT, &path).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "être administrateur ne rend pas propriétaire de la ressource"
    );
}

#[tokio::test]
async fn any_and_all_compose_the_way_the_framework_parser_says() {
    let auth = EntityAuthConfig {
        // OU : le rôle suffit, l'administration aussi.
        create: "any:role:writer,admin_only".to_string(),
        // ET : authentifié **et** porteur du rôle.
        update: "all:authenticated,role:editor".to_string(),
        ..Default::default()
    };
    let base = router(host_with(auth).expect("hôte"));

    let (status, _) = call(
        &as_context(base.clone(), ctx_user_with("writer")),
        Method::POST,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "`any:` : la première branche suffit"
    );

    let (status, _) = call(
        &as_context(base.clone(), ctx_admin()),
        Method::POST,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "`any:` : la seconde branche suffit"
    );

    let (status, _) = call(
        &as_context(base.clone(), ctx_user()),
        Method::POST,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`any:` : aucune branche satisfaite"
    );

    let path = format!("/orders/{ORDER_ID}");
    let (status, _) = call(
        &as_context(base.clone(), ctx_user_with("editor")),
        Method::PATCH,
        &path,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`all:` : les deux branches satisfaites"
    );

    let (status, _) = call(&as_context(base.clone(), ctx_user()), Method::PATCH, &path).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`all:` : la seconde branche manque, donc refus"
    );

    let (status, _) = call(
        &as_context(base, AuthContext::Anonymous),
        Method::PATCH,
        &path,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`all:` : la première branche manque, donc refus"
    );
}

// -- `perm:` : tranché par la décision IAM matérialisée ----------------------

/// Matérialise la décision d'un principal pour une portée (le locataire).
fn materialise(store: &Arc<ObrainStore>, principal: Uuid, tenant: Uuid, permissions: &[&str]) {
    store
        .iam()
        .materialize(
            &principal.to_string(),
            &tenant.to_string(),
            &[RoleGrant {
                role: "r".to_string(),
                permissions: permissions.iter().map(|p| p.to_string()).collect(),
            }],
        )
        .expect("matérialisation de la décision");
}

fn perm_policies() -> EntityAuthConfig {
    EntityAuthConfig {
        delete: "perm:orders:write".to_string(),
        ..Default::default()
    }
}

/// **La propriété centrale du chantier IAM** : en HTTP, une politique
/// `perm:<permission>` est tranchée par la décision **matérialisée** — la même
/// requête passe ou tombe selon ce qui a été écrit dans le store.
#[tokio::test]
async fn a_perm_policy_is_answered_by_the_materialised_decision() {
    let store = ObrainStore::new_in_memory();
    let tenant = Uuid::from_u128(9);
    let user = Uuid::from_u128(1);
    let other = Uuid::from_u128(2);

    let host = host_with_store(perm_policies(), store.clone()).expect("hôte");
    let app = router(host);
    let path = format!("/orders/{ORDER_ID}");

    // Aucune décision matérialisée : refus, même pour un utilisateur authentifié.
    let (status, body) = call(
        &as_context(app.clone(), ctx_user_named(user, tenant)),
        Method::DELETE,
        &path,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "sans décision matérialisée, `perm:` refuse (fail-closed) — {body}"
    );

    // Une décision qui n'accorde PAS la permission demandée : refus.
    materialise(&store, user, tenant, &["orders:read"]);
    let (status, _) = call(
        &as_context(app.clone(), ctx_user_named(user, tenant)),
        Method::DELETE,
        &path,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`orders:read` n'accorde pas `orders:write`"
    );

    // La permission accordée : la MÊME requête passe.
    materialise(&store, user, tenant, &["orders:read", "orders:write"]);
    let (status, _) = call(
        &as_context(app.clone(), ctx_user_named(user, tenant)),
        Method::DELETE,
        &path,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "la décision matérialisée accorde `orders:write`"
    );

    // Un autre principal, pour la même portée : rien.
    let (status, _) = call(
        &as_context(app, ctx_user_named(other, tenant)),
        Method::DELETE,
        &path,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "la décision est indexée par (portée, principal) : celle d'un autre ne vaut rien"
    );
}

fn ctx_user_named(user: Uuid, tenant: Uuid) -> AuthContext {
    AuthContext::User {
        user_id: user,
        tenant_id: tenant,
        roles: vec![],
    }
}

/// **Le cloisonnement par locataire** : la décision du locataire A ne vaut rien
/// pour B — c'est la portée qui isole, et elle est dans la clé d'index.
#[tokio::test]
async fn a_materialised_decision_is_scoped_to_its_tenant() {
    let store = ObrainStore::new_in_memory();
    let tenant_a = Uuid::from_u128(9);
    let tenant_b = Uuid::from_u128(10);
    let user = Uuid::from_u128(1);

    let app = router(host_with_store(perm_policies(), store.clone()).expect("hôte"));
    let path = format!("/orders/{ORDER_ID}");

    materialise(&store, user, tenant_a, &["orders:write"]);

    let (status, _) = call(
        &as_context(app.clone(), ctx_user_named(user, tenant_a)),
        Method::DELETE,
        &path,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "le locataire A a la décision");

    let (status, _) = call(
        &as_context(app, ctx_user_named(user, tenant_b)),
        Method::DELETE,
        &path,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "le même utilisateur, dans le locataire B, n'a AUCUNE décision"
    );
}

/// La provenance d'un refus tranché par l'IAM doit être **publiée** : `iam`.
/// Un refus qui ne dit pas qui a tranché oblige à relire le code.
#[tokio::test]
async fn a_perm_refusal_says_the_iam_decided() {
    let store = ObrainStore::new_in_memory();
    let app = router(host_with_store(perm_policies(), store).expect("hôte"));

    let (status, body) = call(
        &as_context(app, ctx_user_named(Uuid::from_u128(1), Uuid::from_u128(9))),
        Method::DELETE,
        &format!("/orders/{ORDER_ID}"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["policy"], "perm:orders:write");
    assert_eq!(
        body["origin"], "iam",
        "la décision a été tranchée par la lecture matérialisée — et le dit"
    );
}

// ===========================================================================
// 5. ABSTENTION = REFUS
// ===========================================================================

/// **L'abstention est un refus** (contrat du 2026-09-13), vérifié sur le routeur
/// REST réel, avec un store attaché.
///
/// Ce test affirmait l'inverse — son nom le disait. C'était le contrat d'avant
/// la faille 4-segments : une route non reconnue traversait un service armé
/// **sans décision**. Le contrat actuel :
///
/// * seuls les chemins **déclarés publics** passent (`/health`) ;
/// * une route inconnue, une route personnalisée, ou une lecture de graphe
///   **sans `graph.policy`** rendent **403**, et la réponse le **nomme**
///   (`verdict: deny_abstain`) au lieu de laisser deviner ;
/// * conséquence assumée du fail-closed : « route absente » et « route sans
///   garde » ne se distinguent plus par le **statut** (403 dans les deux cas) —
///   c'est le corps qui les distingue, et `no_graph_surface_exists_when_no_store_is_attached`
///   montre l'absence avec la couche retirée.
#[tokio::test]
async fn an_abstention_is_a_refusal_on_the_real_router() {
    // Politiques volontairement strictes : tout ce qui est jugé tombe.
    let strict = EntityAuthConfig {
        list: "admin_only".to_string(),
        get: "admin_only".to_string(),
        create: "admin_only".to_string(),
        update: "admin_only".to_string(),
        delete: "admin_only".to_string(),
        list_links: "admin_only".to_string(),
        create_link: "admin_only".to_string(),
        delete_link: "admin_only".to_string(),
        ..Default::default()
    };
    let store = ObrainStore::new_in_memory();
    let host = host_with_store(strict, store).expect("hôte");
    let custom = Router::new().route("/custom/ping", get(ping));
    let app = router_with(host, vec![custom, link_mutation_routes()]);

    // `/health` : chemin **déclaré public** par la couche elle-même — le seul
    // passage qui reste, et c'est volontaire (liveness).
    let (status, _) = call(&app, Method::GET, "/health").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`/health` est un chemin public déclaré : c'est le seul qui passe sans garde"
    );

    // `/graph` : la route est bien montée (un store est branché) — mais aucune
    // `graph.policy` n'est déclarée, donc abstention, donc refus.
    let (status, body) = call(&app, Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "`/graph` sans `graph.policy` : l'abstention refuse — {body}"
    );
    assert_eq!(
        body["verdict"], "deny_abstain",
        "le refus se nomme : {body}"
    );
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|r| r.contains("graph.policy")),
        "et il nomme le REMÈDE : la raison générique (« n'est le pluriel d'aucune \
         entité ») ferait chercher une entité manquante là où il manque une \
         politique — un opérateur qui cherche au mauvais endroit désarme la \
         couche pour faire taire le symptôme : {body}"
    );

    // Une route personnalisée : le framework ne fabrique pas de politique pour
    // ce qu'il ne connaît pas — et ne pas en fabriquer, désormais, refuse.
    let (status, body) = call(&app, Method::GET, "/custom/ping").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "route personnalisée : {body}"
    );
    assert_eq!(body["verdict"], "deny_abstain");

    // `/graph/identity` : même famille, même sort.
    let (status, _) = call(&app, Method::GET, "/graph/identity").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Une route inexistante sous un pluriel inconnu : refusée aussi — et la
    // raison dit que le pluriel est inconnu, pas que l'identité manque.
    let (status, body) = call(&app, Method::GET, &format!("/inconnus/{ORDER_ID}/items")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|r| r.contains("inconnus")),
        "le refus dit POURQUOI : {body}"
    );

    // Et une forme à un seul segment, que le framework ne reconnaît pas du tout.
    let (status, body) = call(&app, Method::GET, "/inconnu").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

// ===========================================================================
// 5 bis. DÉCLARER LA POLITIQUE ROUVRE LA LECTURE
// ===========================================================================

/// **Le remède est réel, pas théorique.** Même hôte, même store : on déclare
/// `graph.policy` et on présente une identité, et le document sort.
///
/// C'est le parcours d'un consommateur de la base (un moteur d'inférence, un
/// tableau de bord) : il ne peut plus lire par accident, il lit **par droit
/// déclaré**.
#[tokio::test]
async fn declaring_a_graph_policy_serves_the_relief_under_the_armed_layer() {
    let store = ObrainStore::new_in_memory();
    let host = with_graph_policy(
        host_with_store(EntityAuthConfig::default(), store).expect("hôte"),
        "authenticated",
    );
    assert!(
        host.authorize,
        "la couche reste armée : c'est la déclaration qui ouvre"
    );
    let app = as_context(router(host), ctx_user());

    let (status, relief) = call(&app, Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "politique déclarée + appelant identifié : le relief est servi"
    );
    assert_eq!(relief["schema"], RELIEF_SCHEMA);

    let (status, identity) = call(&app, Method::GET, "/graph/identity").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["scheme"], ENTITY_ID_SCHEME);

    // Et toujours aucune lecture **anonyme**, même politique déclarée : la
    // politique est `authenticated`, l'absence de contexte vaut `Anonymous`.
    let anonyme = router(with_graph_policy(
        host_with_store(EntityAuthConfig::default(), ObrainStore::new_in_memory()).expect("hôte"),
        "authenticated",
    ));
    let (status, _) = call(&anonyme, Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "sans contexte, un droit `authenticated` ne s'ouvre pas"
    );
}

// ===========================================================================
// 6. L'OPT-OUT DÉLIBÉRÉ — dégradation ASSUMÉE
// ===========================================================================

/// **Défaut constaté.** `ServerBuilder::without_authorization()` documente un
/// désarmement explicite, et `build_host` ne le transfère pas : il n'y a que
/// `if self.authorize { host.authorize = true; }` (`src/server/builder.rs:492-494`),
/// sans branche `else`. Or `ServerHost::from_builder_components` pose
/// `authorize: true` (`src/server/host.rs:193`) : `false` n'est donc **jamais**
/// écrit, et l'opt-out est un no-op silencieux.
///
/// Conséquence : un service qui se croit publiquement ouvert après avoir appelé
/// `without_authorization()` **refuse** en réalité ses clients en 403.
#[tokio::test]
async fn without_authorization_actually_disarms_the_enforcement() {
    let auth = EntityAuthConfig {
        list: "admin_only".to_string(),
        ..Default::default()
    };

    let host = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(OrdersModule::new(auth))
        .expect("module")
        .without_authorization()
        .build_host()
        .expect("hôte");

    assert!(
        !host.authorize,
        "`without_authorization()` doit rendre `host.authorize == false`"
    );

    // La même requête que dans le test du mode par défaut doit maintenant passer.
    let (status, body) = call(&router(host), Method::GET, "/orders").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "désarmé : la politique déclarée est décorative, la requête anonyme passe — {body}"
    );
}

// ===========================================================================
// 7. FAIL-CLOSED
// ===========================================================================

/// Une politique adossée à un résolveur **inconnu** refuse tout le monde —
/// administrateur compris. C'est le cas où le vocabulaire reconnaît la forme
/// (`resolver:`) sans pouvoir la trancher.
#[tokio::test]
async fn an_unreadable_resolver_policy_refuses_every_context() {
    let auth = EntityAuthConfig {
        create: "resolver:inexistant".to_string(),
        ..Default::default()
    };
    let base = router(host_with(auth).expect("hôte"));

    let contexts = [
        ("anonyme", AuthContext::Anonymous),
        ("utilisateur", ctx_user()),
        ("propriétaire", ctx_owner()),
        ("service", ctx_service()),
        ("administrateur", ctx_admin()),
    ];
    for (label, context) in contexts {
        let (status, body) =
            call(&as_context(base.clone(), context), Method::POST, "/orders").await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "résolveur inconnu ⇒ refus, y compris pour {label} — {body}"
        );
        assert_eq!(body["policy"], "resolver:inexistant");
    }

    // Sans contexte non plus : jamais un passage.
    let (status, _) = call(&base, Method::POST, "/orders").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// **Défaut constaté.** Le contrat annonce qu'« une politique déclarée
/// illisible refuse, jamais ne laisse passer », et le commentaire du middleware
/// affirme que `parse_policy` retombe sur `Resolver("<texte>")`
/// (`src/server/middleware/authorization.rs:411-413`). C'est **faux** : le
/// `match` de `AuthPolicy::parse_policy` retombe sur
/// `_ => AuthPolicy::Authenticated` (`src/core/auth/mod.rs:506`).
///
/// Conséquence : une faute de frappe dans `auth:` (`"admin-onli"`,
/// `"authentificated"`, `"role: adm in"`) ne refuse pas — elle **dégrade
/// silencieusement en `authenticated`**, et tout utilisateur authentifié passe
/// là où l'on croyait exiger un administrateur. C'est un fail-**open**.
#[tokio::test]
async fn a_garbage_policy_refuses_instead_of_degrading_to_authenticated() {
    let auth = EntityAuthConfig {
        list: "n'importe quoi".to_string(),
        ..Default::default()
    };
    let app = router(host_with(auth).expect("hôte"));

    let (status, body) = call(&as_context(app.clone(), ctx_user()), Method::GET, "/orders").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "une politique illisible doit REFUSER un utilisateur authentifié, pas l'accepter — {body}"
    );

    let (status, _) = call(
        &as_context(app.clone(), ctx_admin()),
        Method::GET,
        "/orders",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "…ni laisser passer un administrateur"
    );

    let (status, _) = call(&app, Method::GET, "/orders").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "…ni un contexte absent");
}

// ===========================================================================
// 8. LE BRANCHEMENT CLÉ EN MAIN — `with_obrain_store`
// ===========================================================================

/// Un seul appel, cinq effets : relief, identité canonique, plan IAM,
/// autorisation armée, bail d'écriture.
#[tokio::test]
async fn attaching_an_obrain_store_wires_relief_identity_iam_and_the_write_lease() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    let store = ObrainStore::open(dir.path()).expect("store inscriptible");

    let host = host_with_store(EntityAuthConfig::default(), store.clone()).expect("hôte");

    assert!(
        host.graph_store().is_some(),
        "le store est attaché : le relief a une source"
    );
    assert!(
        host.authorize,
        "brancher un store ne désarme pas l'autorisation — elle reste armée"
    );
    assert_eq!(
        host.identity_registry().motives().len(),
        2,
        "un store attaché auto-câble `IdentityRegistry::obrain_default` : convention PUIS \
         propriétés plates"
    );
    assert!(
        store.lease_info().is_some(),
        "un store INSCRIPTIBLE prend le bail d'écriture dès la construction de l'hôte"
    );

    // Le relief se lit **sous la couche armée** : politique de lecture déclarée
    // et appelant identifié (voir le point 3 de l'en-tête de ce fichier).
    let app = as_context(
        router_with(with_graph_policy(host, "authenticated"), vec![]),
        ctx_user(),
    );

    // Le relief, 200, et son schéma publié tel quel.
    let (status, relief) = call(&app, Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /graph doit répondre dès que la lecture est déclarée"
    );
    assert_eq!(
        relief["schema"], RELIEF_SCHEMA,
        "le document doit se nommer par son schéma"
    );
    assert_eq!(
        relief["lease"]["armed"], true,
        "le bail armé est PUBLIÉ, pas seulement pris"
    );
    assert_eq!(
        relief["lease"]["epoch"], 1,
        "le premier bail est d'époque 1"
    );
    assert_eq!(
        relief["iam"]["acl_index"], true,
        "le plan IAM est matérialisé : l'index de décision existe (posé par `Iam::new`)"
    );
    assert_eq!(
        relief["identity"]["full_contract_at"], "/graph/identity",
        "le relief renvoie vers le contrat d'identité COMPLET au lieu de l'inventer"
    );
    assert!(
        relief["identity"].get("motives").is_none(),
        "…et il ne le recopie pas partiellement : la route qui sait est nommée"
    );

    // Le contrat complet, à l'adresse annoncée.
    let (status, identity) = call(&app, Method::GET, "/graph/identity").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        identity["scheme"], ENTITY_ID_SCHEME,
        "le schéma d'identité en vigueur doit être publié"
    );
    assert_eq!(
        identity["motives"].as_array().map(Vec::len),
        Some(2),
        "les deux motifs sont publiés, dans l'ordre : convention puis propriétés plates"
    );
    assert!(
        identity["counts"].is_null(),
        "sans `?counts=true`, pas de compteur — et surtout pas un zéro muet"
    );
}

/// Un store ouvert en **lecture seule** ne prend **jamais** de bail : il n'écrit
/// pas, donc il n'y a rien à arbitrer. Et le relief doit le dire.
#[tokio::test]
async fn a_read_only_store_is_never_given_a_write_lease() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    // Une base doit exister avant d'être ouverte en lecture seule.
    {
        let writable = ObrainStore::open(dir.path()).expect("store inscriptible");
        assert!(writable.lease_info().is_none(), "aucun bail avant l'hôte");
    }

    let read_only = ObrainStore::open_read_only(dir.path()).expect("store en lecture seule");
    assert!(
        read_only.is_read_only(),
        "le store est bien ouvert en lecture seule"
    );

    let host = host_with_store(EntityAuthConfig::default(), read_only.clone()).expect("hôte");

    assert!(
        read_only.lease_info().is_none(),
        "un store ouvert en lecture seule ne prend PAS de bail, même avec le fencing armé"
    );

    let app = as_context(
        router_with(with_graph_policy(host, "authenticated"), vec![]),
        ctx_user(),
    );
    let (status, relief) = call(&app, Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "publier le relief d'un store qu'on ne possède pas est le cas d'usage visé \
         (lecture déclarée + appelant identifié)"
    );
    assert_eq!(
        relief["engine"]["read_only"], true,
        "le relief publie le mode d'ouverture : un lecteur ne doit pas croire pouvoir écrire"
    );
    assert_eq!(
        relief["lease"]["armed"], false,
        "aucun bail sur un store en lecture seule — et le relief le dit"
    );
}

/// **Le registre d'identité obrain est auto-câblé** — la preuve en HTTP : un
/// nœud à **propriétés plates** (ce qu'écrit un outil tiers, sans `_uuid`) est
/// compté par la route publique.
#[tokio::test]
async fn the_attached_store_identity_counts_are_served_over_http() {
    let store = ObrainStore::new_in_memory();

    // Un nœud écrit par un outil qui n'est pas this-rs : label + propriété plate,
    // aucun `_uuid`. On passe par l'API publique du moteur, comme lui.
    store.db().session().create_node_with_props(
        &["Page"],
        [(
            "qualified_name",
            obrain_common::types::Value::from("entity:page-a"),
        )],
    );

    let host = host_with_store(EntityAuthConfig::default(), store.clone()).expect("hôte");
    let app = as_context(
        router_with(with_graph_policy(host, "authenticated"), vec![]),
        ctx_user(),
    );

    let (status, identity) = call(&app, Method::GET, "/graph/identity?counts=true").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "le contrat d'identité se lit comme le relief : politique déclarée + identité"
    );
    assert_eq!(
        identity["counts"]["total_resolved"], 1,
        "le nœud plat est résolu par le motif des propriétés plates — donc le registre \
         obrain est bien auto-câblé par `with_obrain_store` : {identity}"
    );
    assert_eq!(
        identity["count_bounds"]["full_scan_served"], false,
        "le comptage est borné, et le dit"
    );
    assert_eq!(
        identity["index_len"], 1,
        "l'index d'identité du store a bien été constitué par la requête"
    );
}

/// Le pendant négatif : **sans** store attaché, aucune route `/graph` n'existe.
/// Pas de relief partiel, pas d'invention.
///
/// **Comment on l'observe maintenant** : avec la couche armée, une route
/// *absente* et une route *sans garde* rendent le même 403 (abstention = refus),
/// donc le statut ne prouve plus l'absence. On retire la couche pour voir le
/// **404 du routeur** — puis on vérifie, couche armée, que le refus est bien un
/// refus d'abstention et **jamais un 200**.
#[tokio::test]
async fn no_graph_surface_exists_when_no_store_is_attached() {
    // 1) Couche retirée : ce que rend le routeur lui-même.
    let mut host = host_with(EntityAuthConfig::default()).expect("hôte");
    host.authorize = false;
    let nu = router(host);

    let (status, body) = call(&nu, Method::GET, "/graph").await;
    assert_ne!(
        status,
        StatusCode::OK,
        "sans store attaché, aucun relief n'est servi — {body}"
    );
    assert!(
        body["schema"].is_null(),
        "…et surtout aucun document de relief inventé : {body}"
    );
    // Ce que le routeur répond vraiment, nommé : `/graph` est un chemin à un
    // segment, et le **repli** du routeur de liens le réclame avant de constater
    // qu'il n'est pas une route d'entité. Ce n'est ni un relief, ni un 200.
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "réponse du repli du routeur de liens (`Invalid path format`) : {body}"
    );

    // 2) Couche armée : refus d'abstention, jamais un document.
    let arme = router(host_with(EntityAuthConfig::default()).expect("hôte"));
    let (status, body) = call(&arme, Method::GET, "/graph").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "aucun relief n'est servi non plus avec la couche armée — {body}"
    );
    assert_eq!(body["verdict"], "deny_abstain");
}
