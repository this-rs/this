//! **Chaîne d'autorisation — les chemins qui ne sont pas des routes d'entité.**
//!
//! Les politiques `auth:` se rattachent à une **route d'entité** : le résolveur
//! lit le pluriel, la méthode et le nombre de segments, et en déduit
//! l'opération. Tout ce que le routeur sert **en dehors** de ces formes
//! s'abstient donc — et c'est là que se logent les trous.
//!
//! Ce fichier vérifie chaque famille de chemins :
//!
//! * ce qui est **jugé** par une politique (et laquelle) ;
//! * ce qui **s'abstient**, et ce qui le protège alors ;
//! * ce qui **refuse de se monter** quand il ne peut pas appliquer les
//!   politiques.

#![cfg(feature = "obrain")]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthResolverRegistry};
use this::core::link::LinkDefinition;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::rest::notifications::{NotificationState, notification_routes};
use this::server::middleware::authorization::{Authorization, Requirement};
use this::storage::ObrainStore;

// ---------------------------------------------------------------------------
// Outils
// ---------------------------------------------------------------------------

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

fn authorization(policy: &str) -> Authorization {
    let auth = EntityAuthConfig {
        list: policy.to_string(),
        get: policy.to_string(),
        create: policy.to_string(),
        update: policy.to_string(),
        delete: policy.to_string(),
        list_links: policy.to_string(),
        create_link: policy.to_string(),
        delete_link: policy.to_string(),
        ..Default::default()
    };
    let config = LinksConfig {
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
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Orders));
    Authorization::new(
        Arc::new(config),
        &registry,
        Arc::new(AuthResolverRegistry::new()),
    )
    .with_iam(&ObrainStore::new_in_memory())
}

/// La classification d'un chemin, en clair — c'est **la carte** que ces tests
/// vérifient.
fn classify(authz: &Authorization, method: Method, path: &str) -> String {
    match authz.requirement(&method, path) {
        Requirement::Policy { operation, .. } => format!("policy:{operation}"),
        Requirement::Abstain { .. } => "abstain".to_string(),
    }
}

fn user(id: u128) -> AuthContext {
    AuthContext::User {
        user_id: uuid::Uuid::from_u128(id),
        tenant_id: uuid::Uuid::from_u128(1),
        roles: vec![],
    }
}

async fn call(app: Router, context: Option<AuthContext>, method: Method, uri: &str) -> StatusCode {
    let app = match context {
        Some(ctx) => app.layer(Extension(ctx)),
        None => app,
    };
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .expect("réponse")
    .status()
}

/// Le routeur des notifications, comme le monte l'exposition REST.
fn notifications_app() -> Router {
    notification_routes(NotificationState {
        notification_store: Arc::new(this::events::sinks::in_app::NotificationStore::new()),
        preferences_store: Arc::new(
            this::events::sinks::preferences::NotificationPreferencesStore::new(),
        ),
        device_token_store: Arc::new(this::events::sinks::device_tokens::DeviceTokenStore::new()),
    })
}

// ===========================================================================
// 1. LA CARTE DES CHEMINS JUGÉS (routes d'entité)
// ===========================================================================

/// **Chaque forme servie par le routeur d'entités est jugée.** Ce test est la
/// carte : s'il casse, une route a été ajoutée sans opération correspondante —
/// et elle serait servie sans politique.
#[test]
fn every_entity_route_shape_carries_a_policy() {
    let authz = authorization("admin_only");

    // Les formes que `build_entity_routes` monte.
    let judged = [
        ("list", Method::GET, "/orders"),
        ("create", Method::POST, "/orders"),
        ("get", Method::GET, "/orders/42"),
        ("update", Method::PUT, "/orders/42"),
        ("update", Method::PATCH, "/orders/42"),
        ("delete", Method::DELETE, "/orders/42"),
        ("list_links", Method::GET, "/orders/42/links"),
    ];
    for (operation, method, path) in judged {
        assert_eq!(
            classify(&authz, method.clone(), path),
            format!("policy:{operation}"),
            "{method} {path} doit être jugé comme `{operation}`"
        );
        assert!(
            matches!(authz.requirement(&method, path), Requirement::Policy { .. }),
            "{method} {path} ne doit JAMAIS s'abstenir"
        );
    }
}

/// Les formes de **liens** que le routeur sert, une par une. Le résolveur
/// attendait 5 segments ; le routeur en sert aussi à 3 et 4 — d'où l'audit.
#[test]
fn every_link_route_shape_is_classified_and_only_the_known_ones_abstain() {
    let authz = authorization("admin_only");

    let map = [
        // /links/{id} — la ressource est le lien lui-même, hors entité.
        ("abstain", Method::GET, "/links/abc"),
        // 3 segments : lecture de liens (jugé) et création d'entité liée.
        ("policy:list_links", Method::GET, "/orders/42/invoices"),
        // 4 segments : les opérations de lien.
        ("policy:create_link", Method::POST, "/orders/42/invoices/7"),
        // 5 segments : la forme « canonique » du résolveur.
        (
            "policy:create_link",
            Method::POST,
            "/orders/42/links/invoices/7",
        ),
    ];

    for (expected, method, path) in map {
        assert_eq!(
            classify(&authz, method.clone(), path),
            expected,
            "{method} {path} : classification attendue `{expected}`"
        );
    }
}

// ===========================================================================
// 2. LES CHEMINS QUI S'ABSTIENNENT — et ce qui les protège alors
// ===========================================================================

/// **Faille fermée** : les routes de notifications ne sont pas des routes
/// d'entité (donc aucune politique ne s'y rattache) et portaient l'identifiant
/// de l'utilisateur **dans le chemin**. Sans garde, n'importe quel appelant
/// lisait, effaçait ou modifiait les notifications d'un autre — et détournait
/// ses jetons d'appareil.
#[tokio::test]
async fn personal_resources_are_refused_to_another_user() {
    let app = notifications_app();

    // Un utilisateur identifié lit SES notifications : autorisé.
    assert_ne!(
        call(
            app.clone(),
            Some(user(1)),
            Method::GET,
            &format!("/notifications/{}", uuid::Uuid::from_u128(1)),
        )
        .await,
        StatusCode::FORBIDDEN
    );

    // …et celles d'un AUTRE : refusé.
    assert_eq!(
        call(
            app.clone(),
            Some(user(1)),
            Method::GET,
            &format!("/notifications/{}", uuid::Uuid::from_u128(2)),
        )
        .await,
        StatusCode::FORBIDDEN,
        "lire les notifications d'un autre doit être refusé"
    );

    // Idem pour les jetons d'appareil : les détourner, c'est détourner les
    // notifications poussées.
    assert_eq!(
        call(
            app.clone(),
            Some(user(1)),
            Method::GET,
            &format!("/device-tokens/{}", uuid::Uuid::from_u128(2)),
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_ne!(
        call(
            app.clone(),
            Some(user(1)),
            Method::GET,
            &format!("/device-tokens/{}", uuid::Uuid::from_u128(1)),
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

/// Sans contexte, les données personnelles sont **refusées** : fail-closed
/// plutôt que fuite.
#[tokio::test]
async fn personal_resources_require_an_identified_caller() {
    let app = notifications_app();
    for path in [
        format!("/notifications/{}", uuid::Uuid::from_u128(2)),
        format!("/device-tokens/{}", uuid::Uuid::from_u128(2)),
        format!("/notifications/{}/preferences", uuid::Uuid::from_u128(2)),
    ] {
        assert_eq!(
            call(app.clone(), None, Method::GET, &path).await,
            StatusCode::UNAUTHORIZED,
            "{path} sans appelant identifié doit être refusé"
        );
    }
}

/// L'administrateur peut inspecter, et un justificatif de service peut agir :
/// c'est leur rôle, et c'est dit.
#[tokio::test]
async fn an_admin_may_inspect_another_users_resources() {
    let app = notifications_app();
    let admin = AuthContext::Admin {
        admin_id: uuid::Uuid::from_u128(9),
    };
    assert_ne!(
        call(
            app,
            Some(admin),
            Method::GET,
            &format!("/notifications/{}", uuid::Uuid::from_u128(2))
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

/// La garde couvre **toutes** les méthodes et sous-chemins de la collection,
/// pas seulement la lecture : marquer lu, effacer, muter, préférences, jetons.
#[tokio::test]
async fn the_guard_covers_every_method_of_the_collection() {
    let app = notifications_app();
    let other = uuid::Uuid::from_u128(2);
    let cases = [
        (Method::GET, format!("/notifications/{other}/unread-count")),
        (Method::POST, format!("/notifications/{other}/read")),
        (Method::POST, format!("/notifications/{other}/read-all")),
        (Method::GET, format!("/notifications/{other}/preferences")),
        (Method::POST, format!("/notifications/{other}/mute")),
        (Method::POST, format!("/notifications/{other}/unmute")),
        (Method::GET, format!("/device-tokens/{other}")),
        (Method::DELETE, format!("/device-tokens/{other}/jeton")),
    ];
    for (method, path) in cases {
        assert_eq!(
            call(app.clone(), Some(user(1)), method.clone(), &path).await,
            StatusCode::FORBIDDEN,
            "{method} {path} doit être refusé à un autre utilisateur"
        );
    }
}

// ===========================================================================
// 3. LES SURFACES HORS REST REFUSENT DE SE MONTER
// ===========================================================================

/// **Fail-closed** : les surfaces qui ne peuvent pas appliquer les politiques
/// (elles ne sont pas résolues par route) **refusent de se monter** quand elles
/// sont armées, au lieu de publier les entités sans leur politique.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn graphql_refuses_to_mount_when_policies_are_armed() {
    use this::server::GraphQLExposure;
    use this::server::builder::ServerBuilder;

    let host = Arc::new(
        ServerBuilder::new()
            .register_module(ModuleImpose)
            .expect("module")
            .with_obrain_store(ObrainStore::new_in_memory())
            .build_host()
            .expect("hôte"),
    );
    assert!(host.authorize, "les politiques sont armées par défaut");

    let refused = GraphQLExposure::build_router(Arc::clone(&host));
    let message = format!("{:#}", refused.expect_err("doit refuser"));
    assert!(
        message.contains("ARMÉES") && message.contains("build_router_unenforced"),
        "le refus doit dire pourquoi et nommer l'acceptation explicite : {message}"
    );

    // L'acceptation explicite monte la surface.
    assert!(
        GraphQLExposure::build_router_unenforced(host).is_ok(),
        "l'opt-in explicite doit monter la surface"
    );
}

/// Et une application qui a **désarmé** l'application des politiques peut monter
/// la surface sans rien accepter de particulier : il n'y a rien à appliquer.
#[cfg(feature = "graphql")]
#[tokio::test]
async fn graphql_mounts_when_nothing_is_enforced() {
    use this::server::GraphQLExposure;
    use this::server::builder::ServerBuilder;

    let host = Arc::new(
        ServerBuilder::new()
            .register_module(ModuleImpose)
            .expect("module")
            .with_obrain_store(ObrainStore::new_in_memory())
            .without_authorization()
            .build_host()
            .expect("hôte"),
    );
    assert!(!host.authorize);
    assert!(
        GraphQLExposure::build_router(host).is_ok(),
        "sans politiques armées, la surface se monte normalement"
    );
}

struct ModuleImpose;

impl this::core::module::Module for ModuleImpose {
    fn name(&self) -> &str {
        "impose"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["order"]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: EntityAuthConfig {
                    list: "admin_only".to_string(),
                    ..Default::default()
                },
            }],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        })
    }
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(Orders));
    }
}

/// Le raccourci **du framework** `build_with_grpc()` montait REST (appliqué) et
/// gRPC (non appliqué) dans le même routeur, sans un mot. Il refuse désormais.
#[cfg(feature = "grpc")]
#[tokio::test]
async fn the_framework_grpc_shortcut_refuses_instead_of_half_protecting() {
    use this::server::builder::ServerBuilder;

    let refused = ServerBuilder::new()
        .register_module(ModuleImpose)
        .expect("module")
        .with_obrain_store(ObrainStore::new_in_memory())
        .build_with_grpc();
    let message = format!("{:#}", refused.expect_err("doit refuser"));
    assert!(
        message.contains("gRPC") && message.contains("ARMÉES"),
        "le refus doit nommer la surface en cause : {message}"
    );
}
