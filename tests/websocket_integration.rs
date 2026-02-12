//! Integration tests for the WebSocket exposure
//!
//! These tests spin up a real HTTP+WebSocket server and verify the full
//! event flow: connect → subscribe → REST mutation → receive event via WS.

#![cfg(feature = "websocket")]

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use this::core::events::EventBus;
use this::server::exposure::rest::RestExposure;
use this::server::exposure::websocket::WebSocketExposure;
use this::server::host::ServerHost;
use this::storage::InMemoryLinkService;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Helper: build a minimal ServerHost with EventBus enabled
fn build_test_host() -> Arc<ServerHost> {
    use std::collections::HashMap;
    use this::config::LinksConfig;
    use this::server::entity_registry::EntityRegistry;

    let host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        LinksConfig::default_config(),
        EntityRegistry::new(),
        HashMap::new(),
        HashMap::new(),
    )
    .unwrap()
    .with_event_bus(EventBus::new(256));

    Arc::new(host)
}

/// Helper: start a test server and return (address, host)
async fn start_test_server() -> (SocketAddr, Arc<ServerHost>) {
    let host = build_test_host();

    let rest_router = RestExposure::build_router(host.clone(), vec![]).unwrap();
    let ws_router = WebSocketExposure::build_router(host.clone()).unwrap();
    let app = rest_router.merge(ws_router);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Small delay to let the server start
    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, host)
}

/// Helper: connect to WS and return the welcome message + stream
async fn ws_connect(
    addr: SocketAddr,
) -> (
    Value,
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let url = format!("ws://{}/ws", addr);
    let (ws_stream, _) = connect_async(&url).await.expect("Failed to connect");
    let (write, mut read) = ws_stream.split();

    // Read welcome message
    let welcome_msg = timeout(Duration::from_secs(2), read.next())
        .await
        .expect("Timeout waiting for welcome")
        .expect("Stream ended")
        .expect("WS error");

    let welcome: Value = match welcome_msg {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("Expected text message, got {:?}", other),
    };

    assert_eq!(welcome["type"], "welcome");
    assert!(welcome["connection_id"].is_string());

    (welcome, write, read)
}

/// Helper: send a JSON message over WS
async fn ws_send(
    write: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    msg: &Value,
) {
    let text = serde_json::to_string(msg).unwrap();
    write.send(Message::Text(text.into())).await.unwrap();
}

/// Helper: receive next JSON message from WS (with timeout)
async fn ws_recv(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Value {
    let msg = timeout(Duration::from_secs(2), read.next())
        .await
        .expect("Timeout waiting for WS message")
        .expect("Stream ended")
        .expect("WS error");

    match msg {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("Expected text message, got {:?}", other),
    }
}

// === Tests ===

#[tokio::test]
async fn test_ws_connect_and_welcome() {
    let (addr, _host) = start_test_server().await;
    let (welcome, _write, _read) = ws_connect(addr).await;

    assert_eq!(welcome["type"], "welcome");
    let conn_id = welcome["connection_id"].as_str().unwrap();
    assert!(conn_id.starts_with("conn_"));
}

#[tokio::test]
async fn test_ws_ping_pong() {
    let (addr, _host) = start_test_server().await;
    let (_welcome, mut write, mut read) = ws_connect(addr).await;

    // Send ping
    ws_send(&mut write, &json!({"type": "ping"})).await;

    // Receive pong
    let pong = ws_recv(&mut read).await;
    assert_eq!(pong["type"], "pong");
}

#[tokio::test]
async fn test_ws_subscribe_and_receive_event() {
    let (addr, host) = start_test_server().await;
    let (_welcome, mut write, mut read) = ws_connect(addr).await;

    // Subscribe to all events
    ws_send(&mut write, &json!({"type": "subscribe", "filter": {}})).await;

    // Receive subscription confirmation
    let subscribed = ws_recv(&mut read).await;
    assert_eq!(subscribed["type"], "subscribed");
    let sub_id = subscribed["subscription_id"].as_str().unwrap().to_string();
    assert!(sub_id.starts_with("sub_"));

    // Publish an event directly through the EventBus
    let entity_id = uuid::Uuid::new_v4();
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id,
                data: json!({"amount": 42}),
            },
        ));

    // Receive the event via WebSocket
    let event = ws_recv(&mut read).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["subscription_id"], sub_id);
    assert!(event["data"]["event"].is_object());
}

#[tokio::test]
async fn test_ws_subscribe_with_filter() {
    let (addr, host) = start_test_server().await;
    let (_welcome, mut write, mut read) = ws_connect(addr).await;

    // Subscribe to order events only
    ws_send(
        &mut write,
        &json!({"type": "subscribe", "filter": {"entity_type": "order"}}),
    )
    .await;

    let subscribed = ws_recv(&mut read).await;
    assert_eq!(subscribed["type"], "subscribed");

    // Publish an invoice event (should NOT match)
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "invoice".to_string(),
                entity_id: uuid::Uuid::new_v4(),
                data: json!({}),
            },
        ));

    // Publish an order event (should match)
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: uuid::Uuid::new_v4(),
                data: json!({"amount": 99}),
            },
        ));

    // Should receive only the order event
    let event = ws_recv(&mut read).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["data"]["event"]["entity_type"], "order");
}

#[tokio::test]
async fn test_ws_unsubscribe() {
    let (addr, host) = start_test_server().await;
    let (_welcome, mut write, mut read) = ws_connect(addr).await;

    // Subscribe
    ws_send(&mut write, &json!({"type": "subscribe", "filter": {}})).await;

    let subscribed = ws_recv(&mut read).await;
    let sub_id = subscribed["subscription_id"].as_str().unwrap().to_string();

    // Unsubscribe
    ws_send(
        &mut write,
        &json!({"type": "unsubscribe", "subscription_id": sub_id}),
    )
    .await;

    let unsubscribed = ws_recv(&mut read).await;
    assert_eq!(unsubscribed["type"], "unsubscribed");
    assert_eq!(unsubscribed["subscription_id"], sub_id);

    // Publish an event — should NOT be received
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: uuid::Uuid::new_v4(),
                data: json!({}),
            },
        ));

    // Wait a bit and verify no message arrives
    let result = timeout(Duration::from_millis(200), read.next()).await;
    assert!(
        result.is_err(),
        "Should timeout — no event expected after unsubscribe"
    );
}

#[tokio::test]
async fn test_ws_multi_client_broadcast() {
    let (addr, host) = start_test_server().await;

    // Connect client 1
    let (_welcome1, mut write1, mut read1) = ws_connect(addr).await;
    ws_send(&mut write1, &json!({"type": "subscribe", "filter": {}})).await;
    let sub1 = ws_recv(&mut read1).await;
    assert_eq!(sub1["type"], "subscribed");

    // Connect client 2
    let (_welcome2, mut write2, mut read2) = ws_connect(addr).await;
    ws_send(&mut write2, &json!({"type": "subscribe", "filter": {}})).await;
    let sub2 = ws_recv(&mut read2).await;
    assert_eq!(sub2["type"], "subscribed");

    // Publish one event
    let entity_id = uuid::Uuid::new_v4();
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id,
                data: json!({"test": "broadcast"}),
            },
        ));

    // Both clients should receive it
    let event1 = ws_recv(&mut read1).await;
    let event2 = ws_recv(&mut read2).await;

    assert_eq!(event1["type"], "event");
    assert_eq!(event2["type"], "event");

    // Same event ID in the envelope
    assert_eq!(event1["data"]["id"], event2["data"]["id"]);
}

#[tokio::test]
async fn test_ws_invalid_message() {
    let (addr, _host) = start_test_server().await;
    let (_welcome, mut write, mut read) = ws_connect(addr).await;

    // Send invalid JSON message
    ws_send(&mut write, &json!({"type": "unknown_action"})).await;

    // Should receive an error
    let error = ws_recv(&mut read).await;
    assert_eq!(error["type"], "error");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("Invalid message")
    );
}

#[tokio::test]
async fn test_ws_multiple_subscriptions_same_client() {
    let (addr, host) = start_test_server().await;
    let (_welcome, mut write, mut read) = ws_connect(addr).await;

    // Subscribe to orders
    ws_send(
        &mut write,
        &json!({"type": "subscribe", "filter": {"entity_type": "order"}}),
    )
    .await;
    let sub1 = ws_recv(&mut read).await;
    assert_eq!(sub1["type"], "subscribed");

    // Subscribe to invoices
    ws_send(
        &mut write,
        &json!({"type": "subscribe", "filter": {"entity_type": "invoice"}}),
    )
    .await;
    let sub2 = ws_recv(&mut read).await;
    assert_eq!(sub2["type"], "subscribed");

    // Publish an order event
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: uuid::Uuid::new_v4(),
                data: json!({}),
            },
        ));

    // Should receive via first subscription only
    let event = ws_recv(&mut read).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["subscription_id"], sub1["subscription_id"]);

    // Publish an invoice event
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "invoice".to_string(),
                entity_id: uuid::Uuid::new_v4(),
                data: json!({}),
            },
        ));

    // Should receive via second subscription
    let event = ws_recv(&mut read).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["subscription_id"], sub2["subscription_id"]);
}
