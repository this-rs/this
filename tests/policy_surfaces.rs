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
#[cfg(feature = "obrain")]
use this::storage::ObrainStore;
use this::ui::routes::{UiStateCtx, ui_routes};
use this::ui::state::UiStateStore;

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
    let authz = Authorization::new(
        Arc::new(config),
        &registry,
        Arc::new(AuthResolverRegistry::new()),
    );
    // The IAM plan only exists with obrain; the path classification does not need it.
    #[cfg(feature = "obrain")]
    let authz = authz.with_iam(&ObrainStore::new_in_memory());
    authz
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

/// GraphQL applique désormais les politiques **par opération** (flux auth du
/// ModuleDescriptor) : armé, il se MONTE, et chaque résolveur juge son
/// `(entité, opération)` comme le REST — ici `list: admin_only` refuse l'anonyme.
#[cfg(all(feature = "graphql", feature = "obrain"))]
#[tokio::test]
async fn graphql_mounts_armed_and_enforces_per_operation() {
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

    let app = GraphQLExposure::build_router(Arc::clone(&host)).expect("armé et couvert : monté");
    let response = app
        .oneshot(
            Request::post("/graphql")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":"{ orders { id } }"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["errors"][0]["extensions"]["this_code"], "this.auth.forbidden",
        "list: admin_only doit refuser l'anonyme sur GraphQL comme en REST : {body}"
    );

    // L'acceptation explicite monte toujours la surface.
    assert!(
        GraphQLExposure::build_router_unenforced(host).is_ok(),
        "l'opt-in explicite doit monter la surface"
    );
}

/// Et une application qui a **désarmé** l'application des politiques peut monter
/// la surface sans rien accepter de particulier : il n'y a rien à appliquer.
#[cfg(all(feature = "graphql", feature = "obrain"))]
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

#[cfg(all(any(feature = "graphql", feature = "grpc"), feature = "obrain"))]
struct ModuleImpose;

#[cfg(all(any(feature = "graphql", feature = "grpc"), feature = "obrain"))]
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
/// gRPC (non appliqué) dans le même routeur. gRPC applique désormais les
/// politiques par appel : le raccourci se monte armé — SAUF si une partie non
/// couverte est montée (`NotificationService`), où il refuse toujours.
#[cfg(all(feature = "grpc", feature = "obrain"))]
#[tokio::test]
async fn the_framework_grpc_shortcut_refuses_instead_of_half_protecting() {
    use this::server::builder::ServerBuilder;

    assert!(
        ServerBuilder::new()
            .register_module(ModuleImpose)
            .expect("module")
            .with_obrain_store(ObrainStore::new_in_memory())
            .build_with_grpc()
            .is_ok(),
        "gRPC appliqué par opération : servi armé"
    );

    let host = ServerBuilder::new()
        .register_module(ModuleImpose)
        .expect("module")
        .with_obrain_store(ObrainStore::new_in_memory())
        .build_host()
        .expect("hôte")
        .with_notification_store(Arc::new(
            this::events::sinks::in_app::NotificationStore::new(),
        ));
    let refused =
        this::server::exposure::grpc::GrpcExposure::build_router_no_fallback(Arc::new(host));
    let message = format!("{:#}", refused.expect_err("doit refuser"));
    assert!(
        message.contains("gRPC")
            && message.contains("ARMÉES")
            && message.contains("NotificationService"),
        "le refus doit nommer la partie non couverte : {message}"
    );
}

/// **La même faille, refermée sur l'état d'interface.**
///
/// `/ui/state/{portee}` n'est pas une route d'entité : le résolveur de politiques
/// **s'abstient** dessus. La portée est dans le CHEMIN, et la croire sur parole
/// laisserait n'importe quel appelant lire — et réécrire — l'interface d'un
/// autre : sa face, son échelle, ses panneaux, son sujet. C'est le même défaut
/// que sur `/notifications/*`, et il est fermé par la MÊME garde, étendue d'un
/// segment plutôt que recopiée.
#[tokio::test]
async fn l_etat_d_interface_d_un_autre_est_refuse() {
    let ctx = UiStateCtx {
        store: Arc::new(UiStateStore::new()),
    };
    let app = ui_routes(ctx);

    let moi = uuid::Uuid::from_u128(1).to_string();
    let autre = uuid::Uuid::from_u128(2).to_string();

    // Sans appelant identifié : refus (fail-closed), jamais un document par défaut.
    let nu = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/ui/state/{moi}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(nu.status(), axum::http::StatusCode::UNAUTHORIZED);

    // Un utilisateur identifié écrit SA portée : autorisé, et le document relu est
    // celui qu'il a écrit (le store rend un défaut, jamais un vide silencieux).
    let siens = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri(format!("/ui/state/{moi}"))
                .header("content-type", "application/json")
                .extension(user(1))
                .body(axum::body::Body::from(
                    r#"{"vue":"cosmos","echelle":"groupe","panneaux":{"fil":false}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(siens.status(), axum::http::StatusCode::OK);

    // La portée d'un AUTRE : refusé, avec ou sans corps.
    let autre_refuse = app
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/ui/state/{autre}"))
                .extension(user(1))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(autre_refuse.status(), axum::http::StatusCode::FORBIDDEN);
}

/// Without `--features obrain` (a local path dependency, absent from CI) the three
/// tests that build a host on an `ObrainStore` are not compiled. This stub makes the
/// gap VISIBLE in every run (`ignored, ...` in the output) instead of a silent
/// "0 tests"; the rest of this file runs on any feature set.
#[cfg(not(feature = "obrain"))]
#[test]
#[ignore = "NOT RUN: graphql/grpc host tests of this file need --features obrain"]
fn obrain_gated_tests_were_not_run() {}
