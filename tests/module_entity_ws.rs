//! A hot-loaded module's entity, on the WebSocket surface.
//!
//! The event of a declarative entity is proven on the `EventBus` elsewhere
//! (`tests/json_entity_no_double_publish.rs`). What was never proven is the WIRE: a client
//! connected to `/ws`, subscribed to the entity type, receives the event that a REST write on
//! a module's entity publishes. That is what this pins — the fourth surface of a module's
//! entity, with no code of its own.
//!
//! Run: `cargo test -p this-rs --features websocket --test module_entity_ws`.

#![cfg(feature = "websocket")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

use this::core::auth::{AuthContext, AuthProvider};
use this::descriptor::{EntityDesc, FieldDesc, FieldType, ModuleDescriptor, Operation, PolicyExpr};
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::exposure::websocket::WebSocketExposure;
use this::server::host::ServerHost;
use this::server::module_runtime::{ModuleRuntime, ModuleSource};
use this::storage::in_memory::InMemoryLinkService;

/// `gateway { ticket }`: one entity whose every operation requires an authenticated caller
/// (a manifest-sourced module must declare its policies explicitly), and a proxy that is
/// authenticated by construction — the point of the test is the wire, not the identity.
fn gateway() -> ModuleDescriptor {
    let mut entity = EntityDesc::new("ticket");
    entity
        .fields
        .push(FieldDesc::new("subject", FieldType::String));
    for op in Operation::CRUD {
        entity.permissions.ops.insert(op, PolicyExpr::Authenticated);
    }
    let mut module = ModuleDescriptor::new("gateway", "1.0.0");
    module.entities.push(entity);
    module.normalize();
    module
}

/// Every caller is an authenticated user: policies are armed (the production shape) and the
/// point of the test is the wire, not the identity.
struct Token;

#[async_trait::async_trait]
impl AuthProvider for Token {
    async fn extract_context(
        &self,
        _headers: &axum::http::HeaderMap,
    ) -> anyhow::Result<AuthContext> {
        Ok(AuthContext::User {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            roles: vec!["user".into()],
            scopes: vec![],
        })
    }
    async fn is_owner(
        &self,
        _user_id: &uuid::Uuid,
        _resource_id: &uuid::Uuid,
        _resource_type: &str,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _user_id: &uuid::Uuid, _role: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// A live host: runtime with hot loading, event bus, REST + WebSocket on one listener.
async fn start() -> (SocketAddr, Arc<ServerHost>) {
    let host = ServerBuilder::new()
        .with_module_runtime()
        .with_hot_load_enabled(true)
        .with_link_service(InMemoryLinkService::new())
        .with_event_bus(256)
        .build_host()
        .expect("host")
        .with_auth_provider(Arc::new(Token));
    let host = Arc::new(host);

    let rest = RestExposure::build_router(host.clone(), vec![]).expect("REST");
    let ws = WebSocketExposure::build_router_unenforced(host.clone()).expect("WebSocket");
    let app = rest.merge(ws);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, host)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(
    addr: SocketAddr,
) -> (
    futures_util::stream::SplitSink<Ws, Message>,
    futures_util::stream::SplitStream<Ws>,
) {
    let (stream, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("connect");
    stream.split()
}

async fn recv(read: &mut futures_util::stream::SplitStream<Ws>) -> Value {
    let message = timeout(Duration::from_secs(2), read.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("ws error");
    match message {
        Message::Text(text) => serde_json::from_str(&text).expect("json"),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn a_hot_loaded_module_entity_publishes_its_events_on_the_websocket() {
    let (addr, host) = start().await;
    let (mut write, mut read) = connect(addr).await;
    // Welcome.
    assert_eq!(recv(&mut read).await["type"], "welcome");

    let runtime = host.module_runtime.clone().expect("runtime");
    runtime
        .load(ModuleSource::Manifest(gateway().to_json_pretty()))
        .await
        .expect("hot load");

    // Subscribe to the module's entity type only.
    write
        .send(Message::Text(
            json!({"type": "subscribe",
                   "filter": {"entity_type": "ticket"}})
            .to_string()
            .into(),
        ))
        .await
        .expect("send");
    let subscribed = recv(&mut read).await;
    assert_eq!(subscribed["type"], "subscribed", "{subscribed}");
    let subscription_id = subscribed["subscription_id"]
        .as_str()
        .expect("subscription id")
        .to_string();

    // A REST write on the module's entity (the route only exists once the module is loaded).
    let res = RestExposure::build_router(host.clone(), vec![])
        .expect("REST")
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/tickets")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({ "name": "t", "subject": "hello" }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(res.status(), axum::http::StatusCode::CREATED);

    // The event reaches the socket, with the module's entity type.
    let event = recv(&mut read).await;
    assert_eq!(event["type"], "event", "{event}");
    assert_eq!(event["subscription_id"], json!(subscription_id));
    let envelope = event["data"].to_string();
    assert!(
        envelope.contains("\"ticket\""),
        "the event names the module's entity: {envelope}"
    );
    assert!(
        envelope.contains("\"created\""),
        "the event is a creation: {envelope}"
    );
}

/// The REST surface of the SAME host: what the module declares is served, and what it does
/// not is refused — the dispatch layer runs before the link wildcards.
#[tokio::test]
async fn the_module_entity_routes_exist_only_while_the_module_is_loaded() {
    let (_addr, host) = start().await;
    let app = RestExposure::build_router(host.clone(), vec![]).expect("REST");
    assert!(
        app.clone()
            .oneshot(
                axum::http::Request::get("/tickets")
                    .body(axum::body::Body::empty())
                    .expect("request")
            )
            .await
            .expect("response")
            .status()
            .is_client_error()
    );

    let runtime: &Arc<ModuleRuntime> = host.module_runtime.as_ref().expect("runtime");
    runtime
        .load(ModuleSource::Manifest(gateway().to_json_pretty()))
        .await
        .expect("hot load");
    let res = app
        .oneshot(
            axum::http::Request::get("/tickets")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(res.status(), axum::http::StatusCode::OK);
}
