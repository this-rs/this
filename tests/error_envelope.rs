//! **Enveloppe d'erreur sur les voies REST restantes** : 401, 403, corps legacy de
//! `Validated<T>`, rejets d'axum (JSON, Path), 404 / 405 du routeur. Chaque voie porte
//! `code` / `kind` / `message_key` / `fields`, garde ses clés legacy, et un en-tête
//! `Accept-Language` donne le message dans la langue demandée.

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

use this::config::auth::AuthConfig;
use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthProvider};
use this::core::link::LinkDefinition;
use this::core::module::Module;
use this::core::validation::extractor::ValidatableEntity;
use this::core::validation::{EntityValidationConfig, Validated};
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::storage::in_memory::InMemoryLinkService;

/// Anonyme sans en-tête ; erreur d'authentification avec `x-bad-token`.
struct Provider;

#[async_trait]
impl AuthProvider for Provider {
    async fn extract_context(&self, headers: &HeaderMap) -> anyhow::Result<AuthContext> {
        if headers.contains_key("x-bad-token") {
            anyhow::bail!("jeton invalide");
        }
        Ok(AuthContext::Anonymous)
    }
    async fn is_owner(&self, _: &uuid::Uuid, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

struct Orders;
impl EntityDescriptor for Orders {
    fn entity_type(&self) -> &str {
        "order"
    }
    fn plural(&self) -> &str {
        "orders"
    }
    fn build_routes(&self) -> Router {
        Router::new().route("/orders", axum::routing::get(|| async { "[]" }))
    }
}

struct Shop;
impl Module for Shop {
    fn name(&self) -> &str {
        "shop"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["order"]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".into(),
                plural: "orders".into(),
                auth: EntityAuthConfig {
                    list: "authenticated".into(),
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

struct Doc;
impl ValidatableEntity for Doc {
    fn validation_config(_: &str) -> EntityValidationConfig {
        let mut c = EntityValidationConfig::new("doc");
        c.add_validator("name", |f, v| {
            if v.is_null() {
                Err(format!("{f} is required"))
            } else {
                Ok(())
            }
        });
        c
    }
}

fn custom() -> Router {
    Router::new()
        .route(
            "/docs",
            axum::routing::post(|v: Validated<Doc>| async move { axum::Json(v.into_inner()) }),
        )
        .route(
            "/raw",
            axum::routing::post(|axum::Json(v): axum::Json<Value>| async move { axum::Json(v) }),
        )
        .route(
            "/n/{n}",
            axum::routing::get(|Path(n): Path<u32>| async move { n.to_string() }),
        )
}

fn app(auth_default: Option<&str>) -> Router {
    build(auth_default, true)
}

/// Policy layer off: the custom routes reach their extractors.
fn open_app() -> Router {
    build(None, false)
}

fn build(auth_default: Option<&str>, authorize: bool) -> Router {
    let mut b = ServerBuilder::new()
        .register_module(Shop)
        .expect("module")
        .with_link_service(InMemoryLinkService::new());
    if let Some(p) = auth_default {
        b = b.with_auth_config(AuthConfig {
            default_policy: p.into(),
            ..Default::default()
        });
    }
    if !authorize {
        b = b.without_authorization();
    }
    let host = b
        .build_host()
        .expect("hôte")
        .with_auth_provider(Arc::new(Provider));
    RestExposure::build_router(Arc::new(host), vec![custom()]).expect("routeur")
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    lang: Option<&str>,
    extra: Option<(&str, &str)>,
    body: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut r = Request::builder().method(method).uri(uri);
    if let Some(l) = lang {
        r = r.header(header::ACCEPT_LANGUAGE, l);
    }
    if let Some((k, v)) = extra {
        r = r.header(k, v);
    }
    if body.is_some() {
        r = r.header(header::CONTENT_TYPE, "application/json");
    }
    let res = app
        .clone()
        .oneshot(r.body(Body::from(body.unwrap_or("").to_string())).unwrap())
        .await
        .unwrap();
    let (parts, b) = res.into_parts();
    let bytes = axum::body::to_bytes(b, usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| panic!("corps non JSON: {:?}", String::from_utf8_lossy(&bytes)));
    (parts.status, parts.headers, json)
}

/// Le contrat sur une voie : statut, code, clés d'enveloppe, et message qui suit
/// `Accept-Language` (fr ≠ en, `Content-Language` posé).
async fn assert_lane(
    app: &Router,
    method: Method,
    uri: &str,
    extra: Option<(&str, &str)>,
    body: Option<&str>,
    status: StatusCode,
    code: &str,
) -> (Value, Value) {
    let (s_fr, h_fr, fr) = call(app, method.clone(), uri, Some("fr"), extra, body).await;
    let (s_en, _, en) = call(app, method, uri, Some("en"), extra, body).await;
    assert_eq!(s_fr, status, "{uri} fr: {fr}");
    assert_eq!(s_en, status, "{uri} en");
    for b in [&fr, &en] {
        assert_eq!(b["code"], code, "{uri}: {b}");
        assert!(b["kind"].is_string() && b["message_key"].is_string(), "{b}");
        assert!(b["fields"].is_array(), "{b}");
        assert_eq!(b["status"], status.as_u16(), "{b}");
    }
    assert_eq!(h_fr["x-this-error"], code);
    assert_eq!(h_fr[header::CONTENT_LANGUAGE], "fr", "{uri}");
    assert_ne!(
        fr["message"], en["message"],
        "{uri}: fr et en doivent différer"
    );
    (fr, en)
}

#[tokio::test]
async fn rest_forbidden_403_is_enveloped_and_localized() {
    let app = app(None);
    let (fr, en) = assert_lane(
        &app,
        Method::GET,
        "/orders",
        None,
        None,
        StatusCode::FORBIDDEN,
        "this.auth.forbidden",
    )
    .await;
    // Clés legacy conservées.
    assert_eq!(fr["error"], "forbidden");
    assert!(fr["scope"].is_string() && fr["policy"].is_string());
    assert!(fr["message"].as_str().unwrap().contains("refusée"), "{fr}");
    assert!(en["message"].as_str().unwrap().contains("denied"), "{en}");
}

#[tokio::test]
async fn rest_unauthenticated_401_is_enveloped_and_localized() {
    let app = app(Some("authenticated"));
    let (fr, _) = assert_lane(
        &app,
        Method::GET,
        "/orders",
        Some(("x-bad-token", "1")),
        None,
        StatusCode::UNAUTHORIZED,
        "this.auth.unauthenticated",
    )
    .await;
    let (_, h, _) = call(
        &app,
        Method::GET,
        "/orders",
        None,
        Some(("x-bad-token", "1")),
        None,
    )
    .await;
    assert_eq!(h["www-authenticate"], "Bearer");
    assert!(fr["message"].is_string());
}

#[tokio::test]
async fn rest_validated_legacy_422_keeps_error_errors_and_adds_contract() {
    let app = open_app();
    let (fr, _) = assert_lane(
        &app,
        Method::POST,
        "/docs",
        None,
        Some(r#"{"name": null}"#),
        StatusCode::UNPROCESSABLE_ENTITY,
        "this.validation.failed",
    )
    .await;
    assert_eq!(fr["error"], "Validation failed");
    assert_eq!(fr["errors"][0], "name is required");
    assert!(fr["fields"].as_array().is_some());
}

#[tokio::test]
async fn rest_json_rejection_is_enveloped() {
    let app = open_app();
    let (fr, _) = assert_lane(
        &app,
        Method::POST,
        "/raw",
        None,
        Some("{not json"),
        StatusCode::BAD_REQUEST,
        "this.transport.bad_request",
    )
    .await;
    assert!(fr["params"]["reason"].is_string(), "{fr}");
}

#[tokio::test]
async fn rest_path_rejection_is_enveloped() {
    let app = open_app();
    assert_lane(
        &app,
        Method::GET,
        "/n/abc",
        None,
        None,
        StatusCode::BAD_REQUEST,
        "this.transport.bad_request",
    )
    .await;
}

#[tokio::test]
async fn rest_router_404_and_405_are_enveloped() {
    let app = app(None);
    let (fr, _) = assert_lane(
        &app,
        Method::GET,
        "/health/nope",
        None,
        None,
        StatusCode::FORBIDDEN, // refus par défaut (abstention) avant le routeur
        "this.auth.forbidden",
    )
    .await;
    assert_eq!(fr["verdict"], "deny_abstain");
    // 405 : la route existe (`/n/{n}` en GET), pas en POST.
    assert_lane(
        &app,
        Method::POST,
        "/health",
        None,
        None,
        StatusCode::METHOD_NOT_ALLOWED,
        "this.transport.method_not_allowed",
    )
    .await;
}

#[tokio::test]
async fn rest_unmatched_path_reaches_the_link_fallback_and_is_enveloped() {
    // The link router's fallback answers every unmatched path (`ExtractorError::InvalidPath`):
    // its legacy 400 now carries the envelope, and keeps its legacy `error` key.
    let host = ServerBuilder::new()
        .register_module(Shop)
        .expect("module")
        .with_link_service(InMemoryLinkService::new())
        .without_authorization()
        .build_host()
        .expect("hôte");
    let app = RestExposure::build_router(Arc::new(host), vec![]).expect("routeur");
    let (fr, _) = assert_lane(
        &app,
        Method::GET,
        "/a/b/c/d/e/f",
        None,
        None,
        StatusCode::BAD_REQUEST,
        "this.transport.bad_request",
    )
    .await;
    assert_eq!(fr["error"], "Invalid path format");
}

// ── Message bundles of compiled modules ───────────────────────────────

const PAID: &str = "billing.invoice.already_paid";

/// A compiled module that contributes its own translations.
struct Billing;
impl Module for Billing {
    fn name(&self) -> &str {
        "billing"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec![]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        })
    }
    fn register_entities(&self, _: &mut EntityRegistry) {}
    fn bundles(&self) -> Vec<this::descriptor::MessageBundle> {
        use this::descriptor::{Locale, MessageBundle, MessageKey};
        [("fr", "Facture déjà payée"), ("en", "Invoice already paid")]
            .into_iter()
            .map(|(l, t)| MessageBundle {
                module: "billing".into(),
                locale: Locale::new(l),
                messages: [(MessageKey::new(PAID), t.to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .collect()
    }
}

#[tokio::test]
async fn compiled_module_bundles_reach_host_locales_and_the_rest_envelope() {
    use this::core::errors::ApiError;
    use this::descriptor::{ContractError, ErrorKind, Locale};

    let host = ServerBuilder::new()
        .register_module(Billing)
        .expect("module")
        .with_link_service(InMemoryLinkService::new())
        .without_authorization()
        .build_host()
        .expect("hôte");

    // ServerHost::locales carries the module's bundles.
    let snap = host.locales.load();
    let err = ContractError::new(PAID, ErrorKind::Conflict);
    assert_eq!(
        snap.localize(err.clone(), &Locale::new("fr")).message,
        "Facture déjà payée"
    );
    assert_eq!(
        snap.localize(err.clone(), &Locale::new("en")).message,
        "Invoice already paid"
    );

    // And a REST handler of that module is rendered in the request locale.
    let e = err.clone();
    let route = Router::new().route(
        "/pay",
        axum::routing::post(move || {
            let e = e.clone();
            async move { Err::<String, _>(ApiError(e)) }
        }),
    );
    let app = RestExposure::build_router(Arc::new(host), vec![route]).expect("routeur");
    let (s, _, fr) = call(&app, Method::POST, "/pay", Some("fr"), None, Some("{}")).await;
    let (_, _, en) = call(&app, Method::POST, "/pay", Some("en"), None, Some("{}")).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(fr["code"], PAID);
    assert_eq!(fr["message"], "Facture déjà payée");
    assert_eq!(en["message"], "Invoice already paid");
}

#[test]
fn module_default_bundles_are_empty_and_a_bad_bundle_fails_the_build() {
    // Default: no contribution.
    assert!(Shop.bundles().is_empty());

    // A bundle the catalog refuses (a key outside its module namespace) fails `build_host` with the contract error, never silently dropped.
    struct Bad;
    impl Module for Bad {
        fn name(&self) -> &str {
            "bad"
        }
        fn entity_types(&self) -> Vec<&str> {
            vec![]
        }
        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            Billing.links_config()
        }
        fn register_entities(&self, _: &mut EntityRegistry) {}
        fn bundles(&self) -> Vec<this::descriptor::MessageBundle> {
            use this::descriptor::{Locale, MessageBundle, MessageKey};
            vec![MessageBundle {
                module: "bad".into(),
                locale: Locale::new("fr"),
                // A module cannot reword the framework's texts.
                messages: [(MessageKey::new("this.auth.forbidden"), "x".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            }]
        }
    }
    let r = ServerBuilder::new()
        .register_module(Bad)
        .expect("module")
        .with_link_service(InMemoryLinkService::new())
        .build_host();
    assert!(r.is_err(), "a refused bundle must fail the build");
}
