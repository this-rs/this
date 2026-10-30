//! Per-operation policies: the SAME verdict on every protocol.
//!
//! One host, one descriptor, one YAML table; REST (`Authorization`), GraphQL, gRPC
//! (real armed router, identity from the call metadata) and WebSocket (real server,
//! identity at the handshake) must allow and refuse the same callers.

#![cfg(all(feature = "graphql", feature = "grpc", feature = "websocket"))]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthProvider};
use this::core::events::{EntityEvent, EventBus, FrameworkEvent};
use this::core::link::{LinkAuthConfig, LinkDefinition, LinkEntity};
use this::core::{EntityCreator, EntityFetcher};
use this::descriptor::{DescriptorSet, EntityDesc, ModuleDescriptor, Operation, PolicyExpr};
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::grpc::proto::{CreateEntityRequest, GetEntityRequest};
use this::server::exposure::{GraphQLExposure, GrpcExposure, WebSocketExposure};
use this::server::host::ServerHost;
use this::server::middleware::authorization::{Authorization, Decision, PolicyOrigin};
use this::storage::InMemoryLinkService;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

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

struct Stub(&'static str, &'static str);
impl EntityDescriptor for Stub {
    fn entity_type(&self) -> &str {
        self.0
    }
    fn plural(&self) -> &str {
        self.1
    }
    fn build_routes(&self) -> Router {
        Router::new()
    }
}

/// `Bearer admin` → role `admin`; `Bearer reader` → role `reader`; `Bearer user` →
/// no role; anything else fails.
struct Tokens;
#[async_trait]
impl AuthProvider for Tokens {
    async fn extract_context(&self, h: &HeaderMap) -> anyhow::Result<AuthContext> {
        // `Bearer owner:<uuid>` → the OWNER of that one resource.
        if let Some(r) = h
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer owner:"))
        {
            return Ok(AuthContext::Owner {
                user_id: Uuid::from_u128(1),
                tenant_id: Uuid::from_u128(2),
                resource_id: Uuid::parse_str(r)?,
                resource_type: "order".into(),
            });
        }
        let roles = match h.get("authorization").and_then(|v| v.to_str().ok()) {
            Some("Bearer admin") => vec!["admin".to_string()],
            Some("Bearer reader") => vec!["reader".to_string()],
            Some("Bearer user") => vec![],
            _ => anyhow::bail!("no or bad token"),
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

fn entity(singular: &str, plural: &str, auth: EntityAuthConfig) -> EntityConfig {
    EntityConfig {
        singular: singular.into(),
        plural: plural.into(),
        auth,
    }
}

/// YAML: every operation `authenticated` (default), except `invoice.get` =
/// `role:admin` and `has_invoice` deletion = `role:admin`.
/// Descriptor: `order.create` = `role:admin`, `order.get` =
/// `any(role:reader, role:admin)` (function form, unreadable by the flat grammar).
fn host() -> Arc<ServerHost> {
    host_with_order_get(PolicyExpr::parse("any(role:reader, role:admin)").unwrap())
}

fn host_with_order_get(get: PolicyExpr) -> Arc<ServerHost> {
    host_with_ws_origins(get, vec![])
}

fn host_with_ws_origins(get: PolicyExpr, ws_origins: Vec<String>) -> Arc<ServerHost> {
    let config = LinksConfig {
        entities: vec![
            entity("order", "orders", EntityAuthConfig::default()),
            entity(
                "invoice",
                "invoices",
                EntityAuthConfig {
                    get: "role:admin".into(),
                    ..Default::default()
                },
            ),
        ],
        links: vec![LinkDefinition {
            link_type: "has_invoice".into(),
            source_type: "order".into(),
            target_type: "invoice".into(),
            forward_route_name: "invoices".into(),
            reverse_route_name: "order".into(),
            description: None,
            required_fields: None,
            auth: Some(LinkAuthConfig {
                delete: "role:admin".into(),
                ..Default::default()
            }),
        }],
        validation_rules: None,
        events: None,
        sinks: None,
        auth: Some(serde_yaml::from_str("default_policy: authenticated").unwrap()),
    };

    let mut module = ModuleDescriptor::new("shop", "1.0.0");
    let mut order = EntityDesc::new("order");
    order
        .permissions
        .ops
        .insert(Operation::Create, PolicyExpr::HasRole(vec!["admin".into()]));
    order.permissions.ops.insert(Operation::Get, get);
    module.entities.push(order);
    let mut set = DescriptorSet::new();
    set.insert(module).unwrap();

    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert("order".into(), Arc::new(Fetcher));
    fetchers.insert("invoice".into(), Arc::new(Fetcher));
    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert("order".into(), Arc::new(Creator));
    creators.insert("invoice".into(), Arc::new(Creator));
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Stub("order", "orders")));
    registry.register(Box::new(Stub("invoice", "invoices")));

    let mut host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        config,
        registry,
        fetchers,
        creators,
    )
    .unwrap()
    .with_descriptors(set)
    .with_auth_provider(Arc::new(Tokens))
    .with_event_bus(EventBus::new(64));
    host.security = Arc::new(this::config::ServerSecurityConfig {
        websocket_origins: ws_origins,
        ..Default::default()
    });
    assert!(host.authorize, "policies are armed by default");
    Arc::new(host)
}

fn ctx_of(token: &str) -> AuthContext {
    let roles: Vec<String> = match token {
        "admin" | "reader" => vec![token.to_string()],
        _ => vec![],
    };
    AuthContext::User {
        user_id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(2),
        roles,
    }
}

fn rest(host: &ServerHost) -> Authorization {
    Authorization::new(
        host.config.clone(),
        &host.entity_registry,
        host.resolver_registry.clone(),
    )
    .with_catalog(host.catalog.clone())
}

async fn graphql(app: &Router, token: Option<&str>, query: &str) -> Value {
    let mut req = Request::post("/graphql").header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(
            req.body(Body::from(json!({ "query": query }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Unary gRPC call through the REAL router (identity layer included). Returns the
/// `grpc-status` (0 when the call succeeded: the status then travels in trailers).
async fn grpc<M: prost::Message>(app: &Router, path: &str, token: Option<&str>, msg: M) -> i32 {
    let payload = msg.encode_to_vec();
    let mut frame = vec![0u8];
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend(payload);
    let mut req = Request::post(path)
        .header("content-type", "application/grpc")
        .header("te", "trailers");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(frame)).unwrap())
        .await
        .unwrap();
    let status = resp
        .headers()
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    match status {
        Some(code) => code,
        None => {
            assert!(body.len() > 5, "a successful call carries a message");
            0
        }
    }
}

const PERMISSION_DENIED: i32 = 7;
const UNAUTHENTICATED: i32 = 16;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The descriptor declares `order.create = role:admin` (YAML says `authenticated`)
/// and `order.get = any(role:reader, role:admin)`: REST, GraphQL and gRPC decide
/// the same way for every caller, and REST says the descriptor decided.
#[tokio::test]
async fn same_policy_same_verdict_across_protocols() {
    let host = host();
    let authz = rest(&host);
    let gql = GraphQLExposure::build_router(host.clone()).expect("GraphQL served armed");
    let rpc = GrpcExposure::build_router(host.clone()).expect("gRPC served armed");
    let id = Uuid::new_v4();

    for token in ["admin", "reader", "user"] {
        // create
        let expected = token == "admin";
        let d = authz.decide(&Method::POST, "/orders", &ctx_of(token));
        match &d {
            Decision::Allow { origin, .. } | Decision::Deny { origin, .. } => {
                assert_eq!(*origin, PolicyOrigin::Descriptor, "{token}: {d:?}")
            }
            other => panic!("{token}: REST abstained: {other:?}"),
        }
        assert_eq!(
            matches!(d, Decision::Allow { .. }),
            expected,
            "REST create {token}"
        );

        let body = graphql(
            &gql,
            Some(token),
            r#"mutation { createOrder(data: {name: "x"}) { id } }"#,
        )
        .await;
        let gql_allowed = body.get("errors").is_none();
        assert_eq!(gql_allowed, expected, "GraphQL create {token}: {body}");
        if !gql_allowed {
            assert_eq!(body["errors"][0]["extensions"]["code"], "FORBIDDEN");
        }

        let code = grpc(
            &rpc,
            "/this_grpc.EntityService/CreateEntity",
            Some(token),
            CreateEntityRequest {
                entity_type: "order".into(),
                data: None,
            },
        )
        .await;
        assert_eq!(
            code,
            if expected { 0 } else { PERMISSION_DENIED },
            "gRPC create {token}"
        );

        // get (function-form policy)
        let expected = token != "user";
        let d = authz.decide(&Method::GET, &format!("/orders/{id}"), &ctx_of(token));
        assert_eq!(
            matches!(d, Decision::Allow { .. }),
            expected,
            "REST get {token}: {d:?}"
        );
        let body = graphql(
            &gql,
            Some(token),
            &format!(r#"{{ order(id: "{id}") {{ id }} }}"#),
        )
        .await;
        assert_eq!(
            body.get("errors").is_none(),
            expected,
            "GraphQL get {token}: {body}"
        );
        let code = grpc(
            &rpc,
            "/this_grpc.EntityService/GetEntity",
            Some(token),
            GetEntityRequest {
                entity_type: "order".into(),
                entity_id: id.to_string(),
            },
        )
        .await;
        assert_eq!(
            code,
            if expected { 0 } else { PERMISSION_DENIED },
            "gRPC get {token}"
        );
    }

    // Not declared in the descriptor: the YAML table (`authenticated`) decides REST.
    let d = authz.decide(&Method::GET, "/orders", &ctx_of("user"));
    assert!(
        matches!(
            d,
            Decision::Allow {
                origin: PolicyOrigin::Declared,
                ..
            }
        ),
        "{d:?}"
    );
}

/// The armed gRPC router resolves the caller from the call METADATA (identity
/// layer): no token → anonymous → refused by the policy; a forged token with a
/// non-public default policy → UNAUTHENTICATED before any service runs.
#[tokio::test]
async fn grpc_armed_router_resolves_identity_end_to_end() {
    let rpc = GrpcExposure::build_router(host()).unwrap();
    let create = || CreateEntityRequest {
        entity_type: "order".into(),
        data: None,
    };
    let path = "/this_grpc.EntityService/CreateEntity";
    assert_eq!(grpc(&rpc, path, Some("admin"), create()).await, 0);
    assert_eq!(
        grpc(&rpc, path, Some("forged"), create()).await,
        UNAUTHENTICATED
    );
    // No token: the provider fails, default policy not public → UNAUTHENTICATED too.
    assert_eq!(grpc(&rpc, path, None, create()).await, UNAUTHENTICATED);

    // The no-fallback router (cohabitation with REST) is guarded the same way.
    let rpc = GrpcExposure::build_router_no_fallback(host()).unwrap();
    assert_eq!(
        grpc(&rpc, path, Some("user"), create()).await,
        PERMISSION_DENIED
    );
}

/// GraphQL `deleteLink` reaches the link mutation (it was swallowed by the generic
/// `delete*` prefix) and is judged by the link's `delete` policy; a nested relation
/// is refused when the target entity's `get` policy refuses the caller.
#[tokio::test]
async fn graphql_delete_link_is_routed_and_guarded() {
    let host = host();
    let order = Uuid::new_v4();
    let link = host
        .link_service
        .create(LinkEntity::new("has_invoice", order, Uuid::new_v4(), None))
        .await
        .unwrap();
    let app = GraphQLExposure::build_router(host.clone()).unwrap();
    let delete = format!(r#"mutation {{ deleteLink(id: "{}") }}"#, link.id);

    let body = graphql(&app, Some("reader"), &delete).await;
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{body}"
    );
    assert_eq!(
        body["errors"][0]["extensions"]["this_code"],
        "this.auth.forbidden"
    );

    // Nested relation: `order.get` allows a reader, `invoice.get` does not.
    let nested = format!(r#"{{ order(id: "{order}") {{ id invoices {{ id }} }} }}"#);
    let body = graphql(&app, Some("reader"), &nested).await;
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{body}"
    );
    let body = graphql(&app, Some("admin"), &nested).await;
    assert!(body.get("errors").is_none(), "{body}");
    assert_eq!(
        body["data"]["order"]["invoices"].as_array().map(Vec::len),
        Some(1)
    );

    let body = graphql(&app, Some("admin"), &delete).await;
    assert_eq!(body["data"]["deleteLink"], true, "{body}");
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_json(ws: &mut WsStream, wait: Duration) -> Option<Value> {
    loop {
        match tokio::time::timeout(wait, ws.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t)))) => {
                return Some(serde_json::from_str(&t).unwrap());
            }
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

async fn subscribe(addr: std::net::SocketAddr, token: &str) -> WsStream {
    let url = format!("ws://{addr}/ws?access_token={token}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let welcome = next_json(&mut ws, Duration::from_secs(2)).await.unwrap();
    assert_eq!(welcome["type"], "welcome");
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({"type": "subscribe", "filter": {}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let ack = next_json(&mut ws, Duration::from_secs(2)).await.unwrap();
    assert_eq!(ack["type"], "subscribed", "{ack}");
    ws
}

/// The armed WebSocket router is SERVED (it was refused): the caller is resolved at
/// the handshake (`access_token` query), a bad token is a 401, and an event reaches
/// only the connections whose caller may `get` the entity.
#[tokio::test]
async fn ws_subscription_denied_without_role() {
    let host = host();
    let app = WebSocketExposure::build_router(host.clone()).expect("WebSocket served armed");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Bad token, default policy `authenticated`: refused at the handshake.
    match tokio_tungstenite::connect_async(format!("ws://{addr}/ws?access_token=forged")).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 401)
        }
        other => panic!(
            "expected a 401 handshake refusal, got {:?}",
            other.map(|_| ())
        ),
    }

    let mut reader = subscribe(addr, "reader").await;
    let mut user = subscribe(addr, "user").await;

    host.event_bus()
        .unwrap()
        .publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".into(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

    let got = next_json(&mut reader, Duration::from_secs(2))
        .await
        .expect("reader may `get` orders: event delivered");
    assert_eq!(got["type"], "event", "{got}");
    assert!(
        next_json(&mut user, Duration::from_millis(300))
            .await
            .is_none(),
        "`user` may not `get` orders: no event"
    );
}

/// `owner` is satisfied by an `Owner` context for ITS resource only: the owner of
/// order A is refused on order B, on REST, GraphQL and gRPC alike (it used to be
/// satisfied by the context variant, whatever the resource asked for).
#[tokio::test]
async fn owner_policy_compares_resource_id() {
    let host = host_with_order_get(PolicyExpr::Owner);
    let authz = rest(&host);
    let gql = GraphQLExposure::build_router(host.clone()).expect("GraphQL served armed");
    let rpc = GrpcExposure::build_router(host.clone()).expect("gRPC served armed");
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let owner_of_a = AuthContext::Owner {
        user_id: Uuid::from_u128(1),
        tenant_id: Uuid::from_u128(2),
        resource_id: a,
        resource_type: "order".into(),
    };
    let token = format!("owner:{a}");

    // REST
    let own = authz.decide(&Method::GET, &format!("/orders/{a}"), &owner_of_a);
    assert!(matches!(own, Decision::Allow { .. }), "REST own: {own:?}");
    let other = authz.decide(&Method::GET, &format!("/orders/{b}"), &owner_of_a);
    assert!(
        matches!(other, Decision::Deny { .. }),
        "REST other: {other:?}"
    );

    // GraphQL
    let q = |id: Uuid| format!(r#"{{ order(id: "{id}") {{ id }} }}"#);
    let body = graphql(&gql, Some(&token), &q(a)).await;
    assert!(body.get("errors").is_none(), "GraphQL own: {body}");
    let body = graphql(&gql, Some(&token), &q(b)).await;
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "GraphQL other: {body}"
    );

    // gRPC
    let get = |id: Uuid| GetEntityRequest {
        entity_type: "order".into(),
        entity_id: id.to_string(),
    };
    let path = "/this_grpc.EntityService/GetEntity";
    assert_eq!(grpc(&rpc, path, Some(&token), get(a)).await, 0, "gRPC own");
    assert_eq!(
        grpc(&rpc, path, Some(&token), get(b)).await,
        PERMISSION_DENIED,
        "gRPC other"
    );
}

async fn serve_ws(host: Arc<ServerHost>) -> std::net::SocketAddr {
    let app = WebSocketExposure::build_router(host).expect("WebSocket served armed");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Handshake with extra headers; `Ok(status)` of the upgrade or the refusal status.
async fn handshake(addr: std::net::SocketAddr, path: &str, headers: &[(&str, &str)]) -> u16 {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{addr}{path}").into_client_request().unwrap();
    for (k, v) in headers {
        req.headers_mut().append(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(v).unwrap(),
        );
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((_, resp)) => resp.status().as_u16(),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => resp.status().as_u16(),
        Err(e) => panic!("handshake: {e}"),
    }
}

/// Cross-site WebSocket hijacking: an identity that comes from the AMBIENT cookie
/// `this_token` is accepted only from a listed `Origin` (default: refused, also when
/// `Origin` is absent). Explicit tokens (query, header) are not concerned.
#[tokio::test]
async fn ws_cookie_identity_requires_an_allowed_origin() {
    let cookie = ("cookie", "this_token=reader");
    // Default config: no listed origin → the cookie identity is refused.
    let addr = serve_ws(host()).await;
    assert_eq!(handshake(addr, "/ws", &[cookie]).await, 403, "no Origin");
    assert_eq!(
        handshake(addr, "/ws", &[cookie, ("origin", "https://evil.example")]).await,
        403,
        "unlisted Origin (empty list)"
    );
    // An explicit token needs no Origin.
    assert_eq!(handshake(addr, "/ws?access_token=reader", &[]).await, 101);
    // An explicit Authorization header needs no Origin, even a foreign one.
    assert_eq!(
        handshake(
            addr,
            "/ws",
            &[("authorization", "Bearer reader"), ("origin", "https://evil.example")]
        )
        .await,
        101,
        "header token"
    );

    let listed = host_with_ws_origins(
        PolicyExpr::parse("any(role:reader, role:admin)").unwrap(),
        vec!["https://app.example".into()],
    );
    let addr = serve_ws(listed).await;
    assert_eq!(
        handshake(addr, "/ws", &[cookie, ("origin", "https://evil.example")]).await,
        403,
        "unlisted Origin"
    );
    assert_eq!(
        handshake(addr, "/ws", &[cookie]).await,
        403,
        "absent Origin"
    );
    assert_eq!(
        handshake(addr, "/ws", &[cookie, ("origin", "https://app.example")]).await,
        101,
        "listed Origin"
    );
}

/// The handshake token is percent-decoded from the query and also read from
/// `Sec-WebSocket-Protocol`; it never reaches the logs.
#[tokio::test]
async fn ws_handshake_token_is_decoded_and_never_logged() {
    use std::io::Write;
    #[derive(Clone, Default)]
    struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
        type Writer = Buf;
        fn make_writer(&'a self) -> Buf {
            self.clone()
        }
    }
    let logs = Buf::default();
    // Global: the handshake runs in tasks spawned by the server (a scoped subscriber
    // would not follow them). This is the only test of the file that sets one.
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish(),
    )
    .expect("no other global subscriber in this test binary");
    let app = WebSocketExposure::build_router(host()).expect("WebSocket served armed");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // `%72eader` is `reader` once decoded; a raw (undecoded) token would be refused.
    assert_eq!(handshake(addr, "/ws?access_token=%72eader", &[]).await, 101);
    // Browser style: the token travels in the subprotocol list.
    assert_eq!(
        handshake(
            addr,
            "/ws",
            &[("sec-websocket-protocol", "access_token, reader")]
        )
        .await,
        101
    );
    // Forged tokens are refused and must not appear in any log line.
    assert_eq!(
        handshake(addr, "/ws?access_token=s3cr3t-forged%2Btoken", &[]).await,
        401
    );
    assert_eq!(
        handshake(
            addr,
            "/ws",
            &[("sec-websocket-protocol", "access_token, s3cr3t-forged-sub")]
        )
        .await,
        401
    );
    let text = String::from_utf8_lossy(&logs.0.lock().unwrap()).to_string();
    assert!(
        text.contains("authentication failed"),
        "the capture works (refusals are logged): {text}"
    );
    assert!(!text.contains("s3cr3t"), "token leaked into logs: {text}");
}

async fn serve_graphql_ws(host: Arc<ServerHost>) -> std::net::SocketAddr {
    let app = GraphQLExposure::build_router(host).expect("GraphQL served armed");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Cross-site WebSocket hijacking on graphql-ws (`graphql-transport-ws`): an identity
/// that comes from the AMBIENT cookie `this_token` is accepted only from a listed
/// `Origin` (default: refused, also when `Origin` is absent). An explicit
/// `Authorization` header is not concerned.
#[tokio::test]
async fn graphql_ws_cookie_identity_requires_an_allowed_origin() {
    let cookie = ("cookie", "this_token=reader");
    // Default config: no listed origin → the cookie identity is refused.
    let addr = serve_graphql_ws(host()).await;
    assert_eq!(
        handshake(addr, "/graphql/ws", &[cookie]).await,
        403,
        "no Origin"
    );
    assert_eq!(
        handshake(
            addr,
            "/graphql/ws",
            &[cookie, ("origin", "https://evil.example")]
        )
        .await,
        403,
        "unlisted Origin (empty list)"
    );
    // An explicit Authorization header needs no Origin, even a foreign one.
    assert_eq!(
        handshake(
            addr,
            "/graphql/ws",
            &[("authorization", "Bearer reader"), ("origin", "https://evil.example")]
        )
        .await,
        101,
        "header token"
    );

    let listed = host_with_ws_origins(
        PolicyExpr::parse("any(role:reader, role:admin)").unwrap(),
        vec!["https://app.example".into()],
    );
    let addr = serve_graphql_ws(listed).await;
    assert_eq!(
        handshake(
            addr,
            "/graphql/ws",
            &[cookie, ("origin", "https://evil.example")]
        )
        .await,
        403,
        "unlisted Origin"
    );
    assert_eq!(
        handshake(addr, "/graphql/ws", &[cookie]).await,
        403,
        "absent Origin"
    );
    assert_eq!(
        handshake(
            addr,
            "/graphql/ws",
            &[cookie, ("origin", "https://app.example")]
        )
        .await,
        101,
        "listed Origin"
    );
}
