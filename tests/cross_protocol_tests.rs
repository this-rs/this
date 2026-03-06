//! Cross-protocol integration tests
//!
//! These tests verify that the same event/notification is visible across
//! multiple protocol exposures simultaneously:
//! - WebSocket + gRPC EventService receive the same EventBus events
//! - NotificationStore is readable from gRPC NotificationService
//! - Both streaming channels (EventBus + NotificationStore) work concurrently
//!
//! Requires: `--features "grpc,websocket"`

#![cfg(all(feature = "grpc", feature = "websocket"))]

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use this::core::events::EventBus;
use this::core::{EntityCreator, EntityFetcher};
use this::events::sinks::in_app::{NotificationStore, StoredNotification};
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::grpc::GrpcExposure;
use this::server::exposure::websocket::WebSocketExposure;
use this::server::host::ServerHost;
use this::storage::InMemoryLinkService;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

// ============================================================================
// Test infrastructure
// ============================================================================

#[derive(Clone)]
struct TestEntityStore {
    entity_type: String,
    entities: Arc<tokio::sync::RwLock<HashMap<Uuid, Value>>>,
}

impl TestEntityStore {
    fn new(entity_type: &str) -> Self {
        Self {
            entity_type: entity_type.to_string(),
            entities: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait::async_trait]
impl EntityFetcher for TestEntityStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> anyhow::Result<Value> {
        let entities = self.entities.read().await;
        entities
            .get(entity_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{} not found: {}", self.entity_type, entity_id))
    }
    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> anyhow::Result<Vec<Value>> {
        let entities = self.entities.read().await;
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(50) as usize;
        Ok(entities.values().skip(offset).take(limit).cloned().collect())
    }
}

#[async_trait::async_trait]
impl EntityCreator for TestEntityStore {
    async fn create_from_json(&self, entity_data: Value) -> anyhow::Result<Value> {
        let id = Uuid::new_v4();
        let mut data = entity_data.as_object().cloned().unwrap_or_default();
        data.insert("id".to_string(), json!(id.to_string()));
        data.insert("type".to_string(), json!(self.entity_type));
        let value = Value::Object(data);
        self.entities.write().await.insert(id, value.clone());
        Ok(value)
    }
    async fn update_from_json(&self, entity_id: &Uuid, entity_data: Value) -> anyhow::Result<Value> {
        let mut entities = self.entities.write().await;
        let existing = entities
            .get_mut(entity_id)
            .ok_or_else(|| anyhow::anyhow!("not found: {}", entity_id))?;
        if let (Some(obj), Some(update)) = (existing.as_object_mut(), entity_data.as_object()) {
            for (k, v) in update {
                obj.insert(k.clone(), v.clone());
            }
        }
        Ok(existing.clone())
    }
    async fn delete(&self, entity_id: &Uuid) -> anyhow::Result<()> {
        self.entities.write().await.remove(entity_id);
        Ok(())
    }
}

struct TestEntityDescriptor {
    entity_type: String,
    plural: String,
}

impl EntityDescriptor for TestEntityDescriptor {
    fn entity_type(&self) -> &str {
        &self.entity_type
    }
    fn plural(&self) -> &str {
        &self.plural
    }
    fn build_routes(&self) -> axum::Router {
        axum::Router::new()
    }
}

/// Build a ServerHost with EventBus + NotificationStore
fn build_test_host() -> (Arc<ServerHost>, Arc<NotificationStore>) {
    use this::config::LinksConfig;

    let order_store = TestEntityStore::new("order");

    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert("order".to_string(), Arc::new(order_store.clone()));

    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert("order".to_string(), Arc::new(order_store));

    let mut registry = EntityRegistry::new();
    registry.register(Box::new(TestEntityDescriptor {
        entity_type: "order".to_string(),
        plural: "orders".to_string(),
    }));

    let notification_store = Arc::new(NotificationStore::new());

    let host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        LinksConfig::default_config(),
        registry,
        fetchers,
        creators,
    )
    .unwrap()
    .with_event_bus(EventBus::new(256))
    .with_notification_store(notification_store.clone());

    (Arc::new(host), notification_store)
}

/// Start a combined WS + gRPC server
async fn start_combined_server() -> (SocketAddr, Arc<ServerHost>, Arc<NotificationStore>) {
    let (host, store) = build_test_host();

    // Build both WS and gRPC routers sharing the same host
    let ws_router = WebSocketExposure::build_router(host.clone()).unwrap();
    let grpc_router = GrpcExposure::build_router_no_fallback(host.clone()).unwrap();

    let app = ws_router.merge(grpc_router);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, host, store)
}

/// Helper: connect to WebSocket and return (welcome, write, read)
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
    (welcome, write, read)
}

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

/// Create a tonic NotificationService client
async fn notification_client(
    addr: SocketAddr,
) -> this::server::exposure::grpc::proto::notification_service_client::NotificationServiceClient<
    tonic::transport::Channel,
> {
    use this::server::exposure::grpc::proto::notification_service_client::NotificationServiceClient;
    let url = format!("http://{}", addr);
    NotificationServiceClient::connect(url).await.unwrap()
}

/// Create a tonic EventService client
async fn event_client(
    addr: SocketAddr,
) -> this::server::exposure::grpc::proto::event_service_client::EventServiceClient<
    tonic::transport::Channel,
> {
    use this::server::exposure::grpc::proto::event_service_client::EventServiceClient;
    let url = format!("http://{}", addr);
    EventServiceClient::connect(url).await.unwrap()
}

/// Insert a test notification
async fn insert_test_notification(
    store: &NotificationStore,
    user_id: &str,
    title: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    store
        .insert(StoredNotification {
            id,
            recipient_id: user_id.to_string(),
            notification_type: "test".to_string(),
            title: title.to_string(),
            body: format!("Body for {}", title),
            data: json!({"source": "cross_protocol_test"}),
            read: false,
            created_at: Utc::now(),
        })
        .await;
    id
}

// ============================================================================
// Cross-protocol: EventBus event → WS + gRPC EventService simultaneously
// ============================================================================

#[tokio::test]
async fn test_cross_protocol_event_ws_and_grpc() {
    use this::server::exposure::grpc::proto::*;

    let (addr, host, _store) = start_combined_server().await;

    // 1. Connect WebSocket client and subscribe
    let (_welcome, mut ws_write, mut ws_read) = ws_connect(addr).await;
    ws_send(&mut ws_write, &json!({"type": "subscribe", "filter": {}})).await;
    let subscribed = ws_recv(&mut ws_read).await;
    assert_eq!(subscribed["type"], "subscribed");

    // 2. Connect gRPC EventService client and subscribe
    let mut grpc_event = event_client(addr).await;
    let grpc_response = grpc_event
        .subscribe(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        })
        .await
        .unwrap();
    let mut grpc_stream = grpc_response.into_inner();

    // 3. Publish ONE event on the EventBus
    let entity_id = Uuid::new_v4();
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id,
                data: json!({"cross_protocol": true}),
            },
        ));

    // 4. Both protocols should receive the SAME event
    // WebSocket
    let ws_event = ws_recv(&mut ws_read).await;
    assert_eq!(ws_event["type"], "event");
    assert_eq!(ws_event["data"]["event"]["entity_type"], "order");

    // gRPC
    let grpc_event = timeout(Duration::from_millis(200), grpc_stream.next())
        .await
        .expect("gRPC timed out")
        .expect("gRPC stream ended")
        .expect("gRPC error");
    assert_eq!(grpc_event.event_kind, "entity");
    assert_eq!(grpc_event.event_type, "created");
    assert_eq!(grpc_event.entity_type, "order");
    assert_eq!(grpc_event.entity_id, entity_id.to_string());
}

// ============================================================================
// Cross-protocol: Notification insert → gRPC List + gRPC Stream
// ============================================================================

#[tokio::test]
async fn test_cross_protocol_notification_grpc_list_and_stream() {
    use this::server::exposure::grpc::proto::*;

    let (addr, _host, store) = start_combined_server().await;

    // Subscribe to notification stream
    let mut stream_client = notification_client(addr).await;
    let response = stream_client
        .subscribe_notifications(SubscribeNotificationsRequest {
            user_id: Some("cross-user".to_string()),
        })
        .await
        .unwrap();
    let mut stream = response.into_inner();

    // Insert notification
    let notif_id = insert_test_notification(&store, "cross-user", "Cross-proto notif").await;

    // Verify via streaming
    let streamed = timeout(Duration::from_millis(200), stream.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("error");
    assert_eq!(streamed.id, notif_id.to_string());

    // Verify via gRPC List
    let mut list_client = notification_client(addr).await;
    let list_resp = list_client
        .list_notifications(ListNotificationsRequest {
            user_id: "cross-user".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list_resp.notifications.len(), 1);
    assert_eq!(list_resp.notifications[0].id, notif_id.to_string());
}

// ============================================================================
// Cross-protocol: WS event subscription + gRPC notification — concurrent
// ============================================================================

#[tokio::test]
async fn test_cross_protocol_concurrent_event_and_notification() {
    use this::server::exposure::grpc::proto::*;

    let (addr, host, store) = start_combined_server().await;

    // 1. WS subscribe to events
    let (_welcome, mut ws_write, mut ws_read) = ws_connect(addr).await;
    ws_send(
        &mut ws_write,
        &json!({"type": "subscribe", "filter": {"entity_type": "order"}}),
    )
    .await;
    let subscribed = ws_recv(&mut ws_read).await;
    assert_eq!(subscribed["type"], "subscribed");

    // 2. gRPC subscribe to notifications
    let mut notif_client = notification_client(addr).await;
    let notif_resp = notif_client
        .subscribe_notifications(SubscribeNotificationsRequest {
            user_id: Some("user-concurrent".to_string()),
        })
        .await
        .unwrap();
    let mut notif_stream = notif_resp.into_inner();

    // 3. Publish event AND insert notification concurrently
    let entity_id = Uuid::new_v4();
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id,
                data: json!({"status": "pending"}),
            },
        ));

    insert_test_notification(&store, "user-concurrent", "Concurrent notif").await;

    // 4. WS should receive the event
    let ws_event = ws_recv(&mut ws_read).await;
    assert_eq!(ws_event["type"], "event");
    assert_eq!(ws_event["data"]["event"]["entity_type"], "order");

    // 5. gRPC should receive the notification
    let notif_msg = timeout(Duration::from_millis(200), notif_stream.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("error");
    assert_eq!(notif_msg.title, "Concurrent notif");
    assert_eq!(notif_msg.recipient_id, "user-concurrent");
}

// ============================================================================
// Cross-protocol: gRPC CRUD, then verify notification count unaffected
// ============================================================================

#[tokio::test]
async fn test_cross_protocol_grpc_crud_independent_of_notifications() {
    use this::server::exposure::grpc::proto::*;

    let (addr, _host, store) = start_combined_server().await;

    // Insert some notifications
    insert_test_notification(&store, "user-Z", "Notif 1").await;
    insert_test_notification(&store, "user-Z", "Notif 2").await;

    // Verify notifications via gRPC
    let mut notif_client = notification_client(addr).await;
    let list_resp = notif_client
        .list_notifications(ListNotificationsRequest {
            user_id: "user-Z".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list_resp.total, 2);

    // Mark all as read via gRPC
    notif_client
        .mark_all_as_read(MarkAllAsReadRequest {
            user_id: "user-Z".to_string(),
        })
        .await
        .unwrap();

    // Verify unread count is 0
    let unread = notif_client
        .get_unread_count(GetUnreadCountRequest {
            user_id: "user-Z".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(unread.count, 0);
}
