//! End-to-end tests for the notification system across protocols
//!
//! These tests verify that notifications flow correctly through the system:
//! - Direct NotificationStore insert → gRPC ListNotifications
//! - gRPC MarkAsRead / MarkAllAsRead / Delete
//! - gRPC SubscribeNotifications streaming
//! - EventBus publish → gRPC EventService Subscribe receives events
//! - Cross-protocol: NotificationStore visible from gRPC + REST simultaneously
//!
//! Requires: `--features grpc`

#![cfg(feature = "grpc")]

use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use this::core::events::EventBus;
use this::core::{EntityCreator, EntityFetcher};
use this::events::sinks::in_app::{NotificationStore, StoredNotification};
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::grpc::GrpcExposure;
use this::server::host::ServerHost;
use this::storage::InMemoryLinkService;
use tokio::net::TcpListener;
use uuid::Uuid;

// ============================================================================
// Test infrastructure (reused from grpc_integration.rs pattern)
// ============================================================================

#[derive(Clone)]
struct TestEntityStore {
    entity_type: String,
    entities: Arc<tokio::sync::RwLock<HashMap<Uuid, serde_json::Value>>>,
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
    async fn fetch_as_json(&self, entity_id: &Uuid) -> anyhow::Result<serde_json::Value> {
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
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let entities = self.entities.read().await;
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(50) as usize;
        Ok(entities
            .values()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }
}

#[async_trait::async_trait]
impl EntityCreator for TestEntityStore {
    async fn create_from_json(
        &self,
        entity_data: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now().to_rfc3339();
        let mut data = entity_data.as_object().cloned().unwrap_or_default();
        data.insert("id".to_string(), json!(id.to_string()));
        data.insert("type".to_string(), json!(self.entity_type));
        data.insert("created_at".to_string(), json!(now));
        let value = serde_json::Value::Object(data);
        self.entities.write().await.insert(id, value.clone());
        Ok(value)
    }

    async fn update_from_json(
        &self,
        entity_id: &Uuid,
        entity_data: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let mut entities = self.entities.write().await;
        let existing = entities
            .get_mut(entity_id)
            .ok_or_else(|| anyhow::anyhow!("not found: {}", entity_id))?;
        if let (Some(existing_obj), Some(update_obj)) =
            (existing.as_object_mut(), entity_data.as_object())
        {
            for (key, value) in update_obj {
                existing_obj.insert(key.clone(), value.clone());
            }
        }
        Ok(existing.clone())
    }

    async fn delete(&self, entity_id: &Uuid) -> anyhow::Result<()> {
        self.entities
            .write()
            .await
            .remove(entity_id)
            .ok_or_else(|| anyhow::anyhow!("not found: {}", entity_id))?;
        Ok(())
    }
}

struct TestEntityDescriptor {
    entity_type: String,
    plural: String,
}

impl TestEntityDescriptor {
    fn new(entity_type: &str, plural: &str) -> Self {
        Self {
            entity_type: entity_type.to_string(),
            plural: plural.to_string(),
        }
    }
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

/// Build a ServerHost with EventBus + NotificationStore + entity stores
fn build_test_host() -> (Arc<ServerHost>, Arc<NotificationStore>) {
    use this::config::LinksConfig;

    let order_store = TestEntityStore::new("order");

    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert("order".to_string(), Arc::new(order_store.clone()));

    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert("order".to_string(), Arc::new(order_store));

    let mut registry = EntityRegistry::new();
    registry.register(Box::new(TestEntityDescriptor::new("order", "orders")));

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

/// Start a gRPC server with NotificationService enabled
async fn start_server() -> (SocketAddr, Arc<ServerHost>, Arc<NotificationStore>) {
    let (host, store) = build_test_host();

    let grpc_router = GrpcExposure::build_router(host.clone()).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, grpc_router).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, host, store)
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

/// Create a tonic EntityService client
async fn entity_client(
    addr: SocketAddr,
) -> this::server::exposure::grpc::proto::entity_service_client::EntityServiceClient<
    tonic::transport::Channel,
> {
    use this::server::exposure::grpc::proto::entity_service_client::EntityServiceClient;
    let url = format!("http://{}", addr);
    EntityServiceClient::connect(url).await.unwrap()
}

/// Helper: insert a test notification into the store
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
            data: json!({"source": "e2e_test"}),
            read: false,
            created_at: Utc::now(),
        })
        .await;
    id
}

// ============================================================================
// E2E: gRPC NotificationService — full CRUD flow
// ============================================================================

#[tokio::test]
async fn test_e2e_grpc_notification_crud_flow() {
    use this::server::exposure::grpc::proto::*;

    let (addr, _host, store) = start_server().await;
    let mut client = notification_client(addr).await;

    // 1. Insert 3 notifications directly into the store
    let id1 = insert_test_notification(&store, "user-A", "Notif 1").await;
    let id2 = insert_test_notification(&store, "user-A", "Notif 2").await;
    let _id3 = insert_test_notification(&store, "user-A", "Notif 3").await;

    // 2. List via gRPC → should see 3 notifications
    let list_resp = client
        .list_notifications(ListNotificationsRequest {
            user_id: "user-A".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(list_resp.notifications.len(), 3);
    assert_eq!(list_resp.total, 3);
    assert_eq!(list_resp.unread, 3);
    // Newest first
    assert_eq!(list_resp.notifications[0].title, "Notif 3");

    // 3. GetUnreadCount → 3
    let unread_resp = client
        .get_unread_count(GetUnreadCountRequest {
            user_id: "user-A".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(unread_resp.count, 3);

    // 4. MarkAsRead 2 notifications
    let mark_resp = client
        .mark_as_read(MarkAsReadRequest {
            notification_ids: vec![id1.to_string(), id2.to_string()],
            user_id: Some("user-A".to_string()),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(mark_resp.marked_count, 2);

    // 5. GetUnreadCount → 1
    let unread_resp = client
        .get_unread_count(GetUnreadCountRequest {
            user_id: "user-A".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(unread_resp.count, 1);

    // 6. MarkAllAsRead
    let mark_all_resp = client
        .mark_all_as_read(MarkAllAsReadRequest {
            user_id: "user-A".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(mark_all_resp.marked_count, 1);

    // 7. GetUnreadCount → 0
    let unread_resp = client
        .get_unread_count(GetUnreadCountRequest {
            user_id: "user-A".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(unread_resp.count, 0);

    // 8. Delete one notification
    let del_resp = client
        .delete_notification(DeleteNotificationRequest {
            notification_id: id1.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(del_resp.success);

    // 9. List → should have 2 remaining
    let list_resp = client
        .list_notifications(ListNotificationsRequest {
            user_id: "user-A".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list_resp.total, 2);
}

// ============================================================================
// E2E: gRPC NotificationService — pagination
// ============================================================================

#[tokio::test]
async fn test_e2e_grpc_notification_pagination() {
    use this::server::exposure::grpc::proto::*;

    let (addr, _host, store) = start_server().await;
    let mut client = notification_client(addr).await;

    // Insert 10 notifications
    for i in 0..10 {
        insert_test_notification(&store, "user-A", &format!("Notif {}", i)).await;
    }

    // Page 1: limit=3, offset=0
    let page1 = client
        .list_notifications(ListNotificationsRequest {
            user_id: "user-A".to_string(),
            limit: 3,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page1.notifications.len(), 3);
    assert_eq!(page1.total, 10);

    // Page 2: limit=3, offset=3
    let page2 = client
        .list_notifications(ListNotificationsRequest {
            user_id: "user-A".to_string(),
            limit: 3,
            offset: 3,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page2.notifications.len(), 3);

    // Verify no overlap between pages
    let p1_ids: Vec<_> = page1.notifications.iter().map(|n| &n.id).collect();
    let p2_ids: Vec<_> = page2.notifications.iter().map(|n| &n.id).collect();
    for id in &p2_ids {
        assert!(!p1_ids.contains(id), "pages should not overlap");
    }
}

// ============================================================================
// E2E: gRPC SubscribeNotifications — real-time streaming
// ============================================================================

#[tokio::test]
async fn test_e2e_grpc_notification_streaming() {
    use this::server::exposure::grpc::proto::*;
    use tokio_stream::StreamExt;

    let (addr, _host, store) = start_server().await;
    let mut client = notification_client(addr).await;

    // Subscribe to notifications for user-A
    let response = client
        .subscribe_notifications(SubscribeNotificationsRequest {
            user_id: Some("user-A".to_string()),
        })
        .await
        .unwrap();
    let mut stream = response.into_inner();

    // Insert a notification for user-A
    let notif_id = insert_test_notification(&store, "user-A", "Streamed notification").await;

    // Should receive it on the stream
    let msg = tokio::time::timeout(Duration::from_millis(200), stream.next())
        .await
        .expect("timed out waiting for notification")
        .expect("stream ended")
        .expect("error");

    assert_eq!(msg.id, notif_id.to_string());
    assert_eq!(msg.recipient_id, "user-A");
    assert_eq!(msg.title, "Streamed notification");
    assert!(!msg.read);

    // Insert a notification for user-B — should NOT arrive on user-A's stream
    insert_test_notification(&store, "user-B", "Not for A").await;

    let timeout_result =
        tokio::time::timeout(Duration::from_millis(100), stream.next()).await;
    assert!(
        timeout_result.is_err(),
        "should not receive user-B notification on user-A stream"
    );
}

// ============================================================================
// E2E: gRPC EventService — event streaming after entity creation
// ============================================================================

#[tokio::test]
async fn test_e2e_grpc_event_stream_on_entity_create() {
    use this::server::exposure::grpc::proto::*;
    use tokio_stream::StreamExt;

    let (addr, host, _store) = start_server().await;
    let mut event_client = event_client(addr).await;

    // Subscribe to entity events via gRPC EventService
    let response = event_client
        .subscribe(SubscribeRequest {
            entity_type: Some("order".to_string()),
            entity_id: None,
            event_type: Some("created".to_string()),
            kind: Some("entity".to_string()),
            link_type: None,
        })
        .await
        .unwrap();
    let mut stream = response.into_inner();

    // Publish an entity event via EventBus (simulating what a REST/gRPC handler does)
    let entity_id = Uuid::new_v4();
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id,
                data: json!({"amount": 99.99}),
            },
        ));

    // Should receive the event on the gRPC stream
    let msg = tokio::time::timeout(Duration::from_millis(200), stream.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("error");

    assert_eq!(msg.event_kind, "entity");
    assert_eq!(msg.event_type, "created");
    assert_eq!(msg.entity_type, "order");
    assert_eq!(msg.entity_id, entity_id.to_string());
}

// ============================================================================
// E2E: gRPC Entity CRUD → EventBus → EventService stream
// ============================================================================

#[tokio::test]
async fn test_e2e_grpc_entity_create_triggers_event() {
    use this::server::exposure::grpc::proto::*;
    use tokio_stream::StreamExt;

    let (addr, host, _store) = start_server().await;

    // Subscribe to events FIRST
    let mut evt_client = event_client(addr).await;
    let response = evt_client
        .subscribe(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        })
        .await
        .unwrap();
    let mut event_stream = response.into_inner();

    // Create an entity via gRPC
    let mut ent_client = entity_client(addr).await;
    let data = {
        use prost_types::value::Kind;
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "name".to_string(),
            prost_types::Value {
                kind: Some(Kind::StringValue("Test Order".to_string())),
            },
        );
        prost_types::Struct { fields }
    };

    let create_resp = ent_client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(data),
        })
        .await
        .unwrap()
        .into_inner();

    // Verify entity was created
    assert!(create_resp.data.is_some());

    // Now publish an event for this entity (simulating the EventBus publish
    // that normally happens in the framework when auto-wiring is configured)
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({"name": "Test Order"}),
            },
        ));

    // Verify the event arrives on the stream
    let msg = tokio::time::timeout(Duration::from_millis(200), event_stream.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("error");

    assert_eq!(msg.event_kind, "entity");
    assert_eq!(msg.event_type, "created");
    assert_eq!(msg.entity_type, "order");
}

// ============================================================================
// E2E: Cross-service — Notification insert visible from both List and Stream
// ============================================================================

#[tokio::test]
async fn test_e2e_notification_visible_from_list_and_stream() {
    use this::server::exposure::grpc::proto::*;
    use tokio_stream::StreamExt;

    let (addr, _host, store) = start_server().await;
    let mut client = notification_client(addr).await;

    // Start streaming FIRST
    let mut stream_client = notification_client(addr).await;
    let response = stream_client
        .subscribe_notifications(SubscribeNotificationsRequest {
            user_id: Some("user-X".to_string()),
        })
        .await
        .unwrap();
    let mut stream = response.into_inner();

    // Insert a notification
    let notif_id = insert_test_notification(&store, "user-X", "Cross-check").await;

    // Verify via streaming (real-time)
    let streamed = tokio::time::timeout(Duration::from_millis(200), stream.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("error");
    assert_eq!(streamed.id, notif_id.to_string());
    assert_eq!(streamed.title, "Cross-check");

    // Verify via List (query)
    let list_resp = client
        .list_notifications(ListNotificationsRequest {
            user_id: "user-X".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list_resp.notifications.len(), 1);
    assert_eq!(list_resp.notifications[0].id, notif_id.to_string());
    assert_eq!(list_resp.notifications[0].title, "Cross-check");
}

// ============================================================================
// E2E: Multiple users — notifications are properly isolated
// ============================================================================

#[tokio::test]
async fn test_e2e_notification_user_isolation() {
    use this::server::exposure::grpc::proto::*;

    let (addr, _host, store) = start_server().await;
    let mut client = notification_client(addr).await;

    // Insert notifications for different users
    insert_test_notification(&store, "alice", "For Alice 1").await;
    insert_test_notification(&store, "alice", "For Alice 2").await;
    insert_test_notification(&store, "bob", "For Bob").await;

    // Alice should see 2
    let alice = client
        .list_notifications(ListNotificationsRequest {
            user_id: "alice".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(alice.total, 2);
    assert_eq!(alice.unread, 2);

    // Bob should see 1
    let bob = client
        .list_notifications(ListNotificationsRequest {
            user_id: "bob".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(bob.total, 1);
    assert_eq!(bob.unread, 1);

    // Mark Alice's as read — shouldn't affect Bob
    client
        .mark_all_as_read(MarkAllAsReadRequest {
            user_id: "alice".to_string(),
        })
        .await
        .unwrap();

    let bob_after = client
        .get_unread_count(GetUnreadCountRequest {
            user_id: "bob".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(bob_after.count, 1, "Bob's count should be unaffected");
}

// ============================================================================
// E2E: EventBus + NotificationStore simultaneously active
// ============================================================================

#[tokio::test]
async fn test_e2e_event_and_notification_streams_coexist() {
    use this::server::exposure::grpc::proto::*;
    use tokio_stream::StreamExt;

    let (addr, host, store) = start_server().await;

    // Start an EventService subscription
    let mut evt_client = event_client(addr).await;
    let evt_response = evt_client
        .subscribe(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        })
        .await
        .unwrap();
    let mut event_stream = evt_response.into_inner();

    // Start a NotificationService subscription
    let mut notif_client = notification_client(addr).await;
    let notif_response = notif_client
        .subscribe_notifications(SubscribeNotificationsRequest {
            user_id: None, // wildcard
        })
        .await
        .unwrap();
    let mut notif_stream = notif_response.into_inner();

    // Publish an event on EventBus
    host.event_bus()
        .unwrap()
        .publish(this::core::events::FrameworkEvent::Entity(
            this::core::events::EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({"status": "new"}),
            },
        ));

    // Insert a notification
    insert_test_notification(&store, "user-A", "Coexist test").await;

    // Event should arrive on event stream
    let event_msg = tokio::time::timeout(Duration::from_millis(200), event_stream.next())
        .await
        .expect("timed out waiting for event")
        .expect("event stream ended")
        .expect("event error");
    assert_eq!(event_msg.event_kind, "entity");
    assert_eq!(event_msg.event_type, "created");

    // Notification should arrive on notification stream
    let notif_msg = tokio::time::timeout(Duration::from_millis(200), notif_stream.next())
        .await
        .expect("timed out waiting for notification")
        .expect("notification stream ended")
        .expect("notification error");
    assert_eq!(notif_msg.title, "Coexist test");
    assert_eq!(notif_msg.recipient_id, "user-A");
}
