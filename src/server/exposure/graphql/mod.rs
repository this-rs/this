//! GraphQL API exposure for the framework
//!
//! This module provides GraphQL-specific routing and schema generation.
//! It is completely separate from the core framework logic.

mod executor;
mod schema_generator; // Now a directory with sub-modules
mod subscription_handler;

#[cfg(feature = "graphql")]
use crate::core::auth::AuthContext;
#[cfg(feature = "graphql")]
use crate::server::exposure::guard::SurfaceGuard;
#[cfg(feature = "graphql")]
use crate::server::host::ServerHost;
#[cfg(feature = "graphql")]
use anyhow::Result;
#[cfg(feature = "graphql")]
use async_graphql::http::{GraphQLPlaygroundConfig, playground_source};
#[cfg(feature = "graphql")]
use axum::{
    Router,
    extract::{Extension, Json as AxumJson},
    response::{Html, IntoResponse},
    routing::{get, post},
};
#[cfg(feature = "graphql")]
use executor::GraphQLExecutor;
#[cfg(feature = "graphql")]
use serde::Deserialize;
#[cfg(feature = "graphql")]
use std::sync::Arc;

#[cfg(feature = "graphql")]
#[derive(Debug, Deserialize)]
struct GraphQLRequestBody {
    query: String,
    variables: Option<std::collections::HashMap<String, serde_json::Value>>,
    #[allow(dead_code)]
    operation_name: Option<String>,
}

#[cfg(feature = "graphql")]
/// GraphQL API exposure implementation
///
/// This struct encapsulates all GraphQL-specific logic for exposing the API.
/// It is completely separate from the framework core.
pub struct GraphQLExposure;

#[cfg(feature = "graphql")]
impl GraphQLExposure {
    /// Build the GraphQL router from a host
    ///
    /// This method takes a `ServerHost` (which is transport-agnostic) and
    /// builds an Axum router with all GraphQL endpoints.
    ///
    /// # Arguments
    ///
    /// * `host` - The server host containing all framework state
    ///
    /// # Returns
    ///
    /// Returns a fully configured Axum router with:
    /// - GraphQL query endpoint
    /// - GraphQL mutation endpoint
    /// - GraphQL subscription endpoint (optional)
    /// - GraphQL playground (optional)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let host = builder.build_host()?;
    /// let graphql_app = GraphQLExposure::build_router(host)?;
    /// ```
    ///
    /// # Policies (armed host)
    ///
    /// Every resolver applies the declared policy of its `(entity, operation)` —
    /// queries (`list`, `get`), mutations (`create`, `update`, `delete`), link
    /// mutations and nested relations (`list_links`/`create_link`/`delete_link` on the
    /// link, `get` on every entity read through it) — with the same resolution as
    /// REST (descriptor, then YAML `auth:`, then the default). Subscriptions are
    /// filtered by the same policies; notification fields (which take the user from
    /// an argument) are refused while enforced. A refusal is a GraphQL error with
    /// `extensions.code = "FORBIDDEN"` and `extensions.this_code = "this.auth.forbidden"`.
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        // Every part of this surface is guarded or refused (fail-closed): nothing
        // left uncovered, so the armed host is served.
        crate::server::exposure::refuse_uncovered_surface(
            &host,
            "GraphQL",
            &[],
            "GraphQLExposure::build_router_unenforced",
        )?;
        Ok(Self::router(host, true))
    }

    /// Comme [`GraphQLExposure::build_router`], en **acceptant explicitement**
    /// que les politiques `auth:` ne s'appliquent pas ici.
    ///
    /// # Ce que vous acceptez
    ///
    /// Un client GraphQL atteint les entités — **lecture et mutation** — sans
    /// que les politiques déclarées soient consultées. Si vos entités portent
    /// des `auth:` (`admin_only`, `perm:…`), elles ne valent que pour le REST.
    ///
    /// À n'utiliser que si (a) toutes vos entités sont publiques pour qui
    /// atteint le port, ou (b) vous appliquez vous-même le contrôle dans vos
    /// résolveurs. L'appel est journalisé en avertissement, à chaque montage.
    pub fn build_router_unenforced(host: Arc<ServerHost>) -> Result<Router> {
        crate::server::exposure::warn_unenforced_surface(
            "GraphQL",
            "GraphQLExposure::build_router_unenforced",
        );
        Ok(Self::router(host, false))
    }

    fn router(host: Arc<ServerHost>, enforce: bool) -> Router {
        let mut router = Router::new()
            .route("/graphql", post(graphql_handler_custom))
            .route("/graphql/playground", get(graphql_playground))
            .route("/graphql/schema", get(graphql_dynamic_schema));

        // Add WebSocket subscription endpoint if EventBus or NotificationStore is configured
        if host.event_bus().is_some() || host.notification_store().is_some() {
            router = router.route("/graphql/ws", get(subscription_handler::graphql_ws_handler));
        }

        router
            .layer(Extension(Enforcement(enforce)))
            .layer(Extension(host))
    }
}

#[cfg(feature = "graphql")]
/// Was this router built armed (`build_router`) or as an explicit opt-out?
#[derive(Debug, Clone, Copy)]
pub(crate) struct Enforcement(pub bool);

#[cfg(feature = "graphql")]
/// Guard of one GraphQL request (or WS connection).
///
/// Unenforced router or disarmed host: unenforced guard, identity taken only from an
/// `AuthContext` already in the extensions (historical behavior). Otherwise the
/// caller is the extension set by an outer auth layer, or is resolved from the
/// headers (and handshake query); a failed authentication with a non-public
/// default policy is a 401 with a GraphQL error body.
pub(crate) async fn request_guard(
    host: &Arc<ServerHost>,
    enforcement: Option<Enforcement>,
    auth: Option<AuthContext>,
    headers: &axum::http::HeaderMap,
    query: Option<&str>,
) -> std::result::Result<SurfaceGuard, axum::response::Response> {
    // Absent marker = a router not built by this module: fail-closed (enforce).
    let enforce = enforcement.map(|e| e.0).unwrap_or(true);
    if !enforce || !host.authorize {
        return Ok(SurfaceGuard::unenforced(
            auth.unwrap_or(AuthContext::Anonymous),
        ));
    }
    let ctx = match auth {
        Some(ctx) => ctx,
        None => match crate::server::exposure::resolve_identity(host, headers, query).await {
            Ok(ctx) => ctx,
            Err(e) => {
                let e = localize_in(host, headers, query, e);
                return Err((
                    e.http_status_code(),
                    AxumJson(serde_json::json!({
                        "errors": [{ "message": e.message, "extensions": e.graphql_extensions() }]
                    })),
                )
                    .into_response());
            }
        },
    };
    Ok(SurfaceGuard::for_host(host, ctx, "GraphQL"))
}

#[cfg(feature = "graphql")]
/// `err` rendered in the locale of the request (`?lang=`, `x-this-locale`, cookie,
/// `Accept-Language`) from the host catalog, the same negotiation as REST and gRPC.
fn localize_in(
    host: &ServerHost,
    headers: &axum::http::HeaderMap,
    query: Option<&str>,
    err: crate::descriptor::ContractError,
) -> crate::descriptor::ContractError {
    let catalog = host.locales.load();
    let locale = crate::core::i18n::RequestHints::from_http(headers, query).resolve(&catalog);
    catalog.localize(err, &locale)
}

#[cfg(feature = "graphql")]
/// GraphQL error object of an executor failure: a contract error (refusal,
/// validation...) keeps its code in `extensions`; anything else keeps the
/// historical `{ message }` shape.
fn error_object(
    e: &anyhow::Error,
    host: &ServerHost,
    headers: &axum::http::HeaderMap,
    query: Option<&str>,
) -> serde_json::Value {
    for cause in e.chain() {
        if let Some(c) = cause.downcast_ref::<crate::descriptor::ContractError>() {
            let c = localize_in(host, headers, query, c.clone());
            return serde_json::json!({ "message": c.message, "extensions": c.graphql_extensions() });
        }
    }
    serde_json::json!({ "message": e.to_string() })
}

#[cfg(feature = "graphql")]
/// Handler for GraphQL queries and mutations using custom executor
async fn graphql_handler_custom(
    Extension(host): Extension<Arc<ServerHost>>,
    enforcement: Option<Extension<Enforcement>>,
    auth: Option<Extension<AuthContext>>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
    AxumJson(request): AxumJson<GraphQLRequestBody>,
) -> axum::response::Response {
    let guard = match request_guard(
        &host,
        enforcement.map(|Extension(e)| e),
        auth.map(|Extension(c)| c),
        &headers,
        None,
    )
    .await
    {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    let executor = GraphQLExecutor::new(host.clone()).await.with_guard(guard);

    match executor.execute(&request.query, request.variables).await {
        Ok(response) => AxumJson(response).into_response(),
        Err(e) => AxumJson(
            serde_json::json!({ "errors": [error_object(&e, &host, &headers, uri.query())] }),
        )
        .into_response(),
    }
}

#[cfg(feature = "graphql")]
/// Handler for GraphQL playground UI
async fn graphql_playground() -> impl IntoResponse {
    Html(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}

#[cfg(feature = "graphql")]
/// Handler for GraphQL schema SDL export
/// This generates the schema dynamically from entity introspection
async fn graphql_dynamic_schema(Extension(host): Extension<Arc<ServerHost>>) -> impl IntoResponse {
    use schema_generator::SchemaGenerator;

    let generator = SchemaGenerator::new(host);
    let sdl = generator.generate_sdl().await;

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        sdl,
    )
}

#[cfg(not(feature = "graphql"))]
mod graphql_placeholder {
    use super::super::super::host::ServerHost;
    use anyhow::Result;
    use axum::Router;

    pub struct GraphQLExposure;

    impl GraphQLExposure {
        pub fn build_router(_host: ServerHost) -> Result<Router> {
            anyhow::bail!(
                "GraphQL support is not enabled. Enable the 'graphql' feature to use GraphQL."
            );
        }
    }
}

#[cfg(all(test, feature = "graphql"))]
mod auth_tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::auth::AuthProvider;
    use crate::core::{EntityCreator, EntityFetcher};
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode};
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use tower::ServiceExt;
    use uuid::Uuid;

    struct Creator;
    #[async_trait]
    impl EntityCreator for Creator {
        async fn create_from_json(&self, mut v: Value) -> anyhow::Result<Value> {
            v["id"] = json!(Uuid::new_v4().to_string());
            Ok(v)
        }
        async fn update_from_json(&self, _: &Uuid, v: Value) -> anyhow::Result<Value> {
            Ok(v)
        }
        async fn delete(&self, _: &Uuid) -> anyhow::Result<()> {
            Ok(())
        }
    }
    struct Fetcher;
    #[async_trait]
    impl EntityFetcher for Fetcher {
        async fn fetch_as_json(&self, id: &Uuid) -> anyhow::Result<Value> {
            Ok(json!({"id": id.to_string()}))
        }
        async fn list_as_json(&self, _: Option<i32>, _: Option<i32>) -> anyhow::Result<Vec<Value>> {
            Ok(vec![])
        }
    }

    struct Stub;
    impl crate::server::entity_registry::EntityDescriptor for Stub {
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

    /// `Bearer admin` → user with role `admin`; `Bearer reader` → user without it.
    struct Tokens;
    #[async_trait]
    impl AuthProvider for Tokens {
        async fn extract_context(&self, h: &HeaderMap) -> anyhow::Result<AuthContext> {
            let roles = match h.get("authorization").and_then(|v| v.to_str().ok()) {
                Some("Bearer admin") => vec!["admin".to_string()],
                Some("Bearer reader") => vec!["reader".to_string()],
                _ => anyhow::bail!("no token"),
            };
            Ok(AuthContext::User {
                user_id: Uuid::from_u128(1),
                tenant_id: Uuid::from_u128(2),
                roles,
            })
        }
        async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn has_role(&self, _: &Uuid, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
    }

    fn armed_host() -> Arc<ServerHost> {
        Arc::new(build_host())
    }

    fn build_host() -> ServerHost {
        let config = LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".into(),
                plural: "orders".into(),
                auth: EntityAuthConfig {
                    create: "role:admin".into(),
                    ..Default::default()
                },
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".into(), Arc::new(Fetcher));
        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".into(), Arc::new(Creator));
        let mut registry = EntityRegistry::new();
        registry.register(Box::new(Stub));
        let host = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            config,
            registry,
            fetchers,
            creators,
        )
        .unwrap()
        .with_auth_provider(Arc::new(Tokens))
        .with_event_bus(crate::core::events::EventBus::new(16));
        assert!(host.authorize, "policies are armed by default");
        host
    }

    async fn post(app: &Router, token: Option<&str>, query: &str) -> (StatusCode, Value) {
        let mut req = Request::post("/graphql").header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let resp = app
            .clone()
            .oneshot(
                req.body(Body::from(json!({"query": query}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    const CREATE: &str = r#"mutation { createOrder(data: {name: "x"}) { id name } }"#;

    #[tokio::test]
    async fn graphql_mutation_denied_without_role() {
        let app = GraphQLExposure::build_router(armed_host()).unwrap();

        for token in [None, Some("reader")] {
            let (_, body) = post(&app, token, CREATE).await;
            let err = &body["errors"][0];
            assert_eq!(err["extensions"]["code"], "FORBIDDEN", "{token:?}: {body}");
            assert_eq!(err["extensions"]["this_code"], "this.auth.forbidden");
            assert!(body.get("data").is_none(), "no data on refusal: {body}");
        }

        let (_, body) = post(&app, Some("admin"), CREATE).await;
        assert_eq!(body["data"]["createOrder"]["name"], "x", "{body}");

        // A bad token with a non-public default policy is never "anonymous".
        let mut host = build_host();
        let mut cfg = (*host.config).clone();
        cfg.auth = Some(serde_yaml::from_str("default_policy: authenticated").unwrap());
        host.config = Arc::new(cfg);
        let strict = GraphQLExposure::build_router(Arc::new(host)).unwrap();
        let (status, _) = post(&strict, Some("forged"), CREATE).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn graphql_served_with_armed_policies_when_covered() {
        // Armed host WITH an event bus (subscriptions mounted): every part is guarded,
        // so the surface is served instead of refused.
        let host = armed_host();
        let app = GraphQLExposure::build_router(host.clone()).expect("covered: served armed");
        // Reads follow the policies too (default `authenticated`).
        let q = "{ orders { id } }";
        let (_, anon) = post(&app, None, q).await;
        assert_eq!(
            anon["errors"][0]["extensions"]["code"], "FORBIDDEN",
            "{anon}"
        );

        // Notification fields cannot apply policies: with a store, refused when
        // enforced (fail-closed), while the explicit opt-out still serves them.
        let store = Arc::new(crate::events::sinks::in_app::NotificationStore::new());
        let with_store = Arc::new(build_host().with_notification_store(store));
        let app = GraphQLExposure::build_router(with_store.clone()).unwrap();
        let (_, body) = post(
            &app,
            Some("admin"),
            r#"{ notifications(userId: "u") { total } }"#,
        )
        .await;
        assert_eq!(
            body["errors"][0]["extensions"]["code"], "FORBIDDEN",
            "{body}"
        );
        let open = GraphQLExposure::build_router_unenforced(with_store).unwrap();
        let (_, body) = post(&open, None, r#"{ notifications(userId: "u") { total } }"#).await;
        assert!(body.get("errors").is_none(), "{body}");

        // Disarmed host: unenforced guard, historical behavior.
        let mut disarmed = build_host();
        disarmed.authorize = false;
        let app = GraphQLExposure::build_router(Arc::new(disarmed)).unwrap();
        let (_, body) = post(&app, None, CREATE).await;
        assert_eq!(body["data"]["createOrder"]["name"], "x", "{body}");
    }
}
