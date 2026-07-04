//! Connection manager for WebSocket clients
//!
//! The `ConnectionManager` tracks all active WebSocket connections and their
//! subscriptions. When an event arrives from the `EventBus`, it fans out the
//! event to all connections whose subscriptions match the event.
//!
//! # Architecture
//!
//! ```text
//! EventBus ──recv──▶ ConnectionManager::run_dispatch_loop()
//!                          │
//!                    for each connection
//!                          │
//!                    for each subscription
//!                          │
//!                    filter.matches(event)?
//!                          │
//!                    ──yes──▶ send to client via mpsc channel
//! ```

use super::protocol::{ServerMessage, Subscription, SubscriptionFilter};
use crate::core::events::EventEnvelope;
use crate::server::host::ServerHost;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};
use uuid::Uuid;

/// Result type for custom message handlers.
///
/// - `true`: the message was handled (consumed)
/// - `false`: the message was not recognized, fall back to error
pub type CustomHandlerResult = Pin<Box<dyn Future<Output = bool> + Send>>;

/// Trait for handling custom (non-protocol) messages on the WebSocket connection.
///
/// When a client sends a message that doesn't parse as a `ClientMessage`
/// (subscribe/unsubscribe/ping), the `ConnectionManager` invokes the registered
/// custom handler before returning an error.
///
/// This allows host applications to multiplex additional protocols (e.g., chat)
/// over the same `/ws` connection without modifying the core `this-rs` protocol.
///
/// The handler receives:
/// - `connection_id`: unique identifier for the WebSocket connection
/// - `user_id`: optional authenticated user ID (if `associate_user()` was called)
/// - `raw_text`: the raw JSON text that failed to parse as `ClientMessage`
/// - `sender`: an `mpsc::UnboundedSender<ServerMessage>` to send responses back
///   to this specific connection
pub trait CustomMessageHandler: Send + Sync + 'static {
    fn handle(
        &self,
        connection_id: &str,
        user_id: Option<&str>,
        raw_text: &str,
        sender: mpsc::UnboundedSender<ServerMessage>,
    ) -> CustomHandlerResult;
}

/// A handle to a single WebSocket connection
///
/// Each connection has a unique ID and a sender channel to push messages
/// to the client's WebSocket write loop.
struct ConnectionHandle {
    /// Sender to push ServerMessage to the client's write loop
    tx: mpsc::UnboundedSender<ServerMessage>,
    /// Active subscriptions for this connection
    subscriptions: Vec<Subscription>,
    /// Optional user ID associated with this connection
    ///
    /// Set via `associate_user()` after authentication. Used by
    /// `send_to_user()` to dispatch sink notifications to the right connections.
    user_id: Option<String>,
    /// Optional tenant ID for multi-tenant event isolation
    ///
    /// When set, this connection only receives events whose `tenant_id` matches.
    /// Events without a `tenant_id` (broadcast events) are always delivered.
    /// Set at connection time from the `AuthContext` extracted during WS upgrade.
    tenant_id: Option<Uuid>,
}

/// Manages all active WebSocket connections and their subscriptions
///
/// Thread-safe via `RwLock` — reads (dispatch) are frequent, writes
/// (connect/disconnect/subscribe) are infrequent.
pub struct ConnectionManager {
    /// Reference to the server host (for future use, e.g. auth)
    _host: Arc<ServerHost>,
    /// All active connections indexed by connection ID
    connections: RwLock<HashMap<String, ConnectionHandle>>,
    /// Optional custom message handler for non-protocol messages.
    ///
    /// When set, messages that fail to parse as `ClientMessage` are forwarded
    /// to this handler before returning an error to the client.
    custom_handler: RwLock<Option<Arc<dyn CustomMessageHandler>>>,
}

impl std::fmt::Debug for ConnectionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionManager").finish_non_exhaustive()
    }
}

impl ConnectionManager {
    /// Create a new ConnectionManager
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self {
            _host: host,
            connections: RwLock::new(HashMap::new()),
            custom_handler: RwLock::new(None),
        }
    }

    /// Register a custom message handler for non-protocol messages.
    ///
    /// When a client sends a JSON message whose `type` field doesn't match any
    /// `ClientMessage` variant (subscribe/unsubscribe/ping), the handler is
    /// invoked with the raw text and connection metadata. This enables host
    /// applications to multiplex additional protocols (e.g., chat) over the
    /// same `/ws` connection.
    ///
    /// Only one handler can be registered at a time; calling this again replaces
    /// the previous handler.
    pub async fn set_custom_handler(&self, handler: Arc<dyn CustomMessageHandler>) {
        *self.custom_handler.write().await = Some(handler);
    }

    /// Try to handle a message via the custom handler.
    ///
    /// Returns `true` if the message was consumed by the custom handler,
    /// `false` if no handler is registered or the handler didn't recognize it.
    pub async fn try_custom_handler(&self, connection_id: &str, raw_text: &str) -> bool {
        // Read the handler under a short lock, then release before calling.
        let handler = {
            let guard = self.custom_handler.read().await;
            guard.clone()
        };

        let handler = match handler {
            Some(h) => h,
            None => return false,
        };

        // Get user_id and sender for this connection.
        let (user_id, sender) = {
            let connections = self.connections.read().await;
            match connections.get(connection_id) {
                Some(handle) => (handle.user_id.clone(), handle.tx.clone()),
                None => return false,
            }
        };

        handler
            .handle(connection_id, user_id.as_deref(), raw_text, sender)
            .await
    }

    /// Register a new WebSocket connection
    ///
    /// Returns a tuple of (connection_id, receiver) where the receiver
    /// will receive `ServerMessage`s to forward to the client.
    ///
    /// The optional `tenant_id` enables multi-tenant event isolation:
    /// when set, only events with a matching `tenant_id` (or no tenant_id) are dispatched.
    pub async fn connect(
        &self,
        tenant_id: Option<Uuid>,
    ) -> (String, mpsc::UnboundedReceiver<ServerMessage>) {
        let connection_id = format!("conn_{}", Uuid::new_v4().simple());
        let (tx, rx) = mpsc::unbounded_channel();

        let handle = ConnectionHandle {
            tx,
            subscriptions: Vec::new(),
            user_id: None,
            tenant_id,
        };

        self.connections
            .write()
            .await
            .insert(connection_id.clone(), handle);

        tracing::debug!(
            connection_id = %connection_id,
            tenant_id = ?tenant_id,
            "WebSocket client connected"
        );

        (connection_id, rx)
    }

    /// Remove a connection when the client disconnects
    pub async fn disconnect(&self, connection_id: &str) {
        self.connections.write().await.remove(connection_id);
        tracing::debug!(connection_id = %connection_id, "WebSocket client disconnected");
    }

    /// Add a subscription to a connection
    ///
    /// Returns the subscription ID on success, or an error message if the
    /// connection doesn't exist.
    pub async fn subscribe(
        &self,
        connection_id: &str,
        filter: SubscriptionFilter,
    ) -> Result<String, String> {
        let mut connections = self.connections.write().await;
        let conn = connections
            .get_mut(connection_id)
            .ok_or_else(|| format!("Connection {} not found", connection_id))?;

        let subscription = Subscription::new(filter);
        let sub_id = subscription.id.clone();
        conn.subscriptions.push(subscription);

        tracing::debug!(
            connection_id = %connection_id,
            subscription_id = %sub_id,
            "Subscription added"
        );

        Ok(sub_id)
    }

    /// Remove a subscription from a connection
    ///
    /// Returns `true` if the subscription was found and removed.
    pub async fn unsubscribe(
        &self,
        connection_id: &str,
        subscription_id: &str,
    ) -> Result<bool, String> {
        let mut connections = self.connections.write().await;
        let conn = connections
            .get_mut(connection_id)
            .ok_or_else(|| format!("Connection {} not found", connection_id))?;

        let before = conn.subscriptions.len();
        conn.subscriptions.retain(|s| s.id != subscription_id);
        let removed = conn.subscriptions.len() < before;

        if removed {
            tracing::debug!(
                connection_id = %connection_id,
                subscription_id = %subscription_id,
                "Subscription removed"
            );
        }

        Ok(removed)
    }

    /// Send a message to a specific connection
    pub async fn send_to(&self, connection_id: &str, message: ServerMessage) {
        let connections = self.connections.read().await;
        if let Some(conn) = connections.get(connection_id) {
            // If send fails, the receiver is dropped (client disconnected)
            let _ = conn.tx.send(message);
        }
    }

    /// Associate a user ID with a connection
    ///
    /// Call this after the client authenticates (e.g., via a JWT in the
    /// first message or via a query parameter on the WebSocket URL).
    /// Once associated, `send_to_user()` can dispatch notifications to
    /// all connections belonging to that user.
    #[allow(dead_code)] // Will be used when auth is implemented
    pub async fn associate_user(&self, connection_id: &str, user_id: String) -> Result<(), String> {
        let mut connections = self.connections.write().await;
        let conn = connections
            .get_mut(connection_id)
            .ok_or_else(|| format!("Connection {} not found", connection_id))?;

        tracing::debug!(
            connection_id = %connection_id,
            user_id = %user_id,
            "User associated with WebSocket connection"
        );

        conn.user_id = Some(user_id);
        Ok(())
    }

    /// Send a notification payload to all connections belonging to a user
    ///
    /// Returns the number of connections that received the message.
    /// Returns 0 if no connections are associated with the given user ID.
    pub async fn send_to_user(&self, user_id: &str, payload: serde_json::Value) -> usize {
        let connections = self.connections.read().await;
        let mut count = 0;

        for handle in connections.values() {
            if handle.user_id.as_deref() == Some(user_id) {
                let msg = ServerMessage::Notification {
                    data: payload.clone(),
                };
                if handle.tx.send(msg).is_ok() {
                    count += 1;
                }
            }
        }

        if count > 0 {
            tracing::debug!(
                user_id = %user_id,
                connections = count,
                "Dispatched notification to user connections"
            );
        }

        count
    }

    /// Broadcast a notification payload to ALL connected clients
    ///
    /// Returns the number of connections that received the message.
    pub async fn broadcast_payload(&self, payload: serde_json::Value) -> usize {
        let connections = self.connections.read().await;
        let mut count = 0;

        for handle in connections.values() {
            let msg = ServerMessage::Notification {
                data: payload.clone(),
            };
            if handle.tx.send(msg).is_ok() {
                count += 1;
            }
        }

        tracing::debug!(
            connections = count,
            "Broadcast notification to all connections"
        );

        count
    }

    /// Dispatch an event to all matching subscriptions across all connections
    ///
    /// For each connection, check every subscription filter against the event.
    /// If a subscription matches, send the event to that connection with the
    /// subscription ID attached.
    ///
    /// **Tenant isolation**: if a connection has a `tenant_id`, events with a
    /// *different* `tenant_id` are silently skipped. Events without a `tenant_id`
    /// (broadcast events) are always delivered. Anonymous connections (no tenant)
    /// receive all events.
    ///
    /// **User isolation for user-scoped events**: if the event payload contains a
    /// `user_id` field AND the connection has an associated `user_id`, the event
    /// is only delivered if both match. This prevents user A from receiving user B's
    /// private events (e.g., chat messages). Events without `user_id` in the payload
    /// or connections without an associated user are not filtered (backward compatible).
    async fn dispatch_event(&self, envelope: &EventEnvelope) {
        let connections = self.connections.read().await;

        // Pre-extract user_id from the event payload (if present) to avoid
        // re-parsing for every connection. This applies to entity events whose
        // `data` JSON contains a top-level `user_id` string field.
        let event_user_id = Self::extract_event_user_id(&envelope.event);

        for (connection_id, handle) in connections.iter() {
            // Tenant isolation check
            if let Some(conn_tenant) = handle.tenant_id
                && let Some(event_tenant) = envelope.tenant_id()
                && conn_tenant != event_tenant
            {
                continue; // Skip: event belongs to a different tenant
            }

            // User isolation check: if BOTH the event and connection have a user_id,
            // only deliver if they match. This prevents cross-user event leaks.
            if let (Some(event_uid), Some(conn_uid)) = (&event_user_id, &handle.user_id)
                && event_uid != conn_uid
            {
                continue; // Skip: event belongs to a different user
            }

            for subscription in &handle.subscriptions {
                if subscription.filter.matches(&envelope.event) {
                    let message = ServerMessage::Event {
                        subscription_id: subscription.id.clone(),
                        data: envelope.clone(),
                    };

                    if handle.tx.send(message).is_err() {
                        tracing::debug!(
                            connection_id = %connection_id,
                            "Failed to send event to connection (likely disconnected)"
                        );
                        break; // No need to check other subscriptions for this dead connection
                    }
                }
            }
        }
    }

    /// Extract a `user_id` string from the event payload, if present.
    ///
    /// For `EntityEvent::Created` and `EntityEvent::Updated`, checks the `data`
    /// JSON for a top-level `"user_id"` string field. Returns `None` for all
    /// other event types or if the field is absent.
    fn extract_event_user_id(event: &crate::core::events::FrameworkEvent) -> Option<String> {
        use crate::core::events::{EntityEvent, FrameworkEvent};
        match event {
            FrameworkEvent::Entity(
                EntityEvent::Created { data, .. } | EntityEvent::Updated { data, .. },
            ) => data
                .get("user_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            _ => None,
        }
    }

    /// Run the event dispatch loop
    ///
    /// This continuously receives events from the `EventBus` broadcast channel
    /// and dispatches them to matching subscriptions. Should be spawned as a
    /// background task.
    ///
    /// The loop will exit when all senders are dropped (EventBus is destroyed).
    pub async fn run_dispatch_loop(&self, mut rx: broadcast::Receiver<EventEnvelope>) {
        tracing::info!("WebSocket dispatch loop started");

        loop {
            match rx.recv().await {
                Ok(envelope) => {
                    self.dispatch_event(&envelope).await;
                }
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    tracing::warn!(
                        count = count,
                        "WebSocket dispatch loop lagged, {} events skipped",
                        count
                    );
                    // Continue receiving — lagged is not fatal
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("EventBus closed, stopping WebSocket dispatch loop");
                    break;
                }
            }
        }
    }

    /// Get the number of active connections (for monitoring)
    #[allow(dead_code)]
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, EventBus, FrameworkEvent};
    use serde_json::json;

    /// Helper to create a minimal ServerHost for testing
    fn test_host() -> Arc<ServerHost> {
        use crate::config::LinksConfig;
        use crate::server::entity_registry::EntityRegistry;
        use crate::storage::InMemoryLinkService;
        use std::collections::HashMap;

        let host = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .unwrap();

        Arc::new(host)
    }

    #[tokio::test]
    async fn test_connect_and_disconnect() {
        let cm = ConnectionManager::new(test_host());

        let (conn_id, _rx) = cm.connect(None).await;
        assert!(conn_id.starts_with("conn_"));
        assert_eq!(cm.connection_count().await, 1);

        cm.disconnect(&conn_id).await;
        assert_eq!(cm.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_subscribe_and_unsubscribe() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, _rx) = cm.connect(None).await;

        // Subscribe
        let filter = SubscriptionFilter {
            entity_type: Some("order".to_string()),
            ..Default::default()
        };
        let sub_id = cm.subscribe(&conn_id, filter).await.unwrap();
        assert!(sub_id.starts_with("sub_"));

        // Unsubscribe
        let removed = cm.unsubscribe(&conn_id, &sub_id).await.unwrap();
        assert!(removed);

        // Unsubscribe again — should not find it
        let removed = cm.unsubscribe(&conn_id, &sub_id).await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_subscribe_nonexistent_connection() {
        let cm = ConnectionManager::new(test_host());
        let result = cm
            .subscribe("nonexistent", SubscriptionFilter::default())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dispatch_event_matches() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, mut rx) = cm.connect(None).await;

        // Subscribe to order events
        let filter = SubscriptionFilter {
            entity_type: Some("order".to_string()),
            ..Default::default()
        };
        let sub_id = cm.subscribe(&conn_id, filter).await.unwrap();

        // Dispatch a matching event
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"amount": 100}),
        }));

        cm.dispatch_event(&envelope).await;

        // Should receive the event
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Event {
                subscription_id,
                data,
            } => {
                assert_eq!(subscription_id, sub_id);
                assert_eq!(data.id, envelope.id);
            }
            _ => panic!("Expected Event message"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_event_no_match() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, mut rx) = cm.connect(None).await;

        // Subscribe to order events only
        let filter = SubscriptionFilter {
            entity_type: Some("order".to_string()),
            ..Default::default()
        };
        cm.subscribe(&conn_id, filter).await.unwrap();

        // Dispatch an invoice event (should not match)
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "invoice".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        cm.dispatch_event(&envelope).await;

        // Should NOT receive anything
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_dispatch_with_event_bus() {
        let cm = Arc::new(ConnectionManager::new(test_host()));
        let (conn_id, mut rx) = cm.connect(None).await;

        // Subscribe to everything
        cm.subscribe(&conn_id, SubscriptionFilter::default())
            .await
            .unwrap();

        // Create an EventBus and spawn the dispatch loop
        let event_bus = EventBus::new(16);
        let bus_rx = event_bus.subscribe();

        let cm_clone = cm.clone();
        let handle = tokio::spawn(async move {
            cm_clone.run_dispatch_loop(bus_rx).await;
        });

        // Publish an event
        let entity_id = Uuid::new_v4();
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id,
            data: json!({"test": true}),
        }));

        // Wait for the event to be dispatched
        let msg = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout waiting for event")
            .expect("Channel closed");

        match msg {
            ServerMessage::Event { data, .. } => {
                assert_eq!(data.event.entity_id(), Some(entity_id));
            }
            _ => panic!("Expected Event message"),
        }

        // Cleanup
        drop(event_bus);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn test_multiple_subscriptions_same_connection() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, mut rx) = cm.connect(None).await;

        // Subscribe to orders
        cm.subscribe(
            &conn_id,
            SubscriptionFilter {
                entity_type: Some("order".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Subscribe to invoices
        cm.subscribe(
            &conn_id,
            SubscriptionFilter {
                entity_type: Some("invoice".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Dispatch order event — should match first sub
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        cm.dispatch_event(&envelope).await;
        assert!(rx.try_recv().is_ok());

        // Dispatch invoice event — should match second sub
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "invoice".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        cm.dispatch_event(&envelope).await;
        assert!(rx.try_recv().is_ok());

        // Dispatch user event — should match neither
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        cm.dispatch_event(&envelope).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_concurrent_subscriptions_same_event_different_connections() {
        let cm = ConnectionManager::new(test_host());

        let (conn1_id, mut rx1) = cm.connect(None).await;
        let (conn2_id, mut rx2) = cm.connect(None).await;

        // Both connections subscribe to "order" created events
        let filter = SubscriptionFilter {
            entity_type: Some("order".to_string()),
            event_type: Some("created".to_string()),
            ..Default::default()
        };
        cm.subscribe(&conn1_id, filter.clone())
            .await
            .expect("conn1 subscribe should succeed");
        cm.subscribe(&conn2_id, filter)
            .await
            .expect("conn2 subscribe should succeed");

        // Dispatch an order created event
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"total": 50}),
        }));
        cm.dispatch_event(&envelope).await;

        // Both connections should receive the event
        let msg1 = rx1.try_recv().expect("conn1 should receive event");
        let msg2 = rx2.try_recv().expect("conn2 should receive event");

        match (&msg1, &msg2) {
            (ServerMessage::Event { data: d1, .. }, ServerMessage::Event { data: d2, .. }) => {
                assert_eq!(d1.id, envelope.id);
                assert_eq!(d2.id, envelope.id);
            }
            _ => panic!("Expected Event messages for both connections"),
        }
    }

    #[tokio::test]
    async fn test_send_to_nonexistent_connection() {
        let cm = ConnectionManager::new(test_host());

        // Sending to a nonexistent connection should not panic
        cm.send_to("conn_does_not_exist", ServerMessage::Pong).await;

        // Verify manager is still functional
        assert_eq!(cm.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_dead_connection_handling() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, rx) = cm.connect(None).await;

        // Subscribe to all events
        cm.subscribe(&conn_id, SubscriptionFilter::default())
            .await
            .expect("subscribe should succeed");

        // Drop the receiver to simulate a dead connection
        drop(rx);

        // Dispatching should not panic even though the receiver is dropped
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        cm.dispatch_event(&envelope).await;

        // Connection is still registered (cleanup happens on disconnect)
        assert_eq!(cm.connection_count().await, 1);
    }

    #[tokio::test]
    async fn test_dispatch_event_with_multiple_matching_subscriptions() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, mut rx) = cm.connect(None).await;

        // Two subscriptions that both match the same event:
        // 1. Subscribe to all "order" events
        cm.subscribe(
            &conn_id,
            SubscriptionFilter {
                entity_type: Some("order".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("first subscribe should succeed");

        // 2. Subscribe to all "created" events (regardless of entity type)
        cm.subscribe(
            &conn_id,
            SubscriptionFilter {
                event_type: Some("created".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("second subscribe should succeed");

        // Dispatch an order created event — should match BOTH subscriptions
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        cm.dispatch_event(&envelope).await;

        // Should receive two messages (one per matching subscription)
        let msg1 = rx.try_recv().expect("should receive first matching event");
        let msg2 = rx.try_recv().expect("should receive second matching event");

        // Both should be Event messages with the same envelope ID
        match (&msg1, &msg2) {
            (
                ServerMessage::Event {
                    subscription_id: sub1,
                    data: d1,
                },
                ServerMessage::Event {
                    subscription_id: sub2,
                    data: d2,
                },
            ) => {
                assert_ne!(sub1, sub2, "subscription IDs should differ");
                assert_eq!(d1.id, d2.id, "both should carry the same event envelope");
            }
            _ => panic!("Expected two Event messages"),
        }
    }

    #[tokio::test]
    async fn test_associate_user() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, _rx) = cm.connect(None).await;

        // Associate user
        cm.associate_user(&conn_id, "user-A".to_string())
            .await
            .expect("associate should succeed");

        // Non-existent connection should fail
        let result = cm
            .associate_user("conn_nonexistent", "user-B".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_to_user() {
        let cm = ConnectionManager::new(test_host());

        // Create two connections for the same user
        let (conn1_id, mut rx1) = cm.connect(None).await;
        let (conn2_id, mut rx2) = cm.connect(None).await;
        // Create one connection for a different user
        let (conn3_id, mut rx3) = cm.connect(None).await;

        cm.associate_user(&conn1_id, "user-A".to_string())
            .await
            .unwrap();
        cm.associate_user(&conn2_id, "user-A".to_string())
            .await
            .unwrap();
        cm.associate_user(&conn3_id, "user-B".to_string())
            .await
            .unwrap();

        // Send to user-A
        let payload = json!({"title": "Hello user-A"});
        let count = cm.send_to_user("user-A", payload.clone()).await;
        assert_eq!(count, 2);

        // Both user-A connections should receive it
        let msg1 = rx1.try_recv().expect("conn1 should receive");
        let msg2 = rx2.try_recv().expect("conn2 should receive");
        assert!(matches!(msg1, ServerMessage::Notification { .. }));
        assert!(matches!(msg2, ServerMessage::Notification { .. }));

        // user-B should NOT receive it
        assert!(rx3.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_send_to_user_no_match() {
        let cm = ConnectionManager::new(test_host());
        let (_conn_id, _rx) = cm.connect(None).await;
        // Connection has no user_id — send_to_user should return 0
        let count = cm.send_to_user("user-X", json!({})).await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_broadcast_payload() {
        let cm = ConnectionManager::new(test_host());
        let (_conn1_id, mut rx1) = cm.connect(None).await;
        let (_conn2_id, mut rx2) = cm.connect(None).await;

        let payload = json!({"type": "system_announcement", "message": "Server restarting"});
        let count = cm.broadcast_payload(payload.clone()).await;
        assert_eq!(count, 2);

        let msg1 = rx1.try_recv().expect("conn1 should receive broadcast");
        let msg2 = rx2.try_recv().expect("conn2 should receive broadcast");

        match (&msg1, &msg2) {
            (
                ServerMessage::Notification { data: d1 },
                ServerMessage::Notification { data: d2 },
            ) => {
                assert_eq!(d1["message"], "Server restarting");
                assert_eq!(d2["message"], "Server restarting");
            }
            _ => panic!("Expected Notification messages"),
        }
    }

    #[tokio::test]
    async fn test_broadcast_payload_empty() {
        let cm = ConnectionManager::new(test_host());
        // No connections — should return 0
        let count = cm.broadcast_payload(json!({})).await;
        assert_eq!(count, 0);
    }

    // --- Tenant isolation tests ---

    #[tokio::test]
    async fn test_tenant_isolation_same_tenant_receives_event() {
        let cm = ConnectionManager::new(test_host());
        let tenant_a = Uuid::new_v4();

        let (conn_id, mut rx) = cm.connect(Some(tenant_a)).await;
        cm.subscribe(&conn_id, SubscriptionFilter::default())
            .await
            .unwrap();

        // Event for tenant A — should be received
        let envelope = EventEnvelope::new_with_tenant(
            FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({}),
            }),
            tenant_a,
        );
        cm.dispatch_event(&envelope).await;

        assert!(rx.try_recv().is_ok(), "same tenant should receive event");
    }

    #[tokio::test]
    async fn test_tenant_isolation_different_tenant_blocked() {
        let cm = ConnectionManager::new(test_host());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();

        let (conn_id, mut rx) = cm.connect(Some(tenant_a)).await;
        cm.subscribe(&conn_id, SubscriptionFilter::default())
            .await
            .unwrap();

        // Event for tenant B — should NOT be received by tenant A
        let envelope = EventEnvelope::new_with_tenant(
            FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({}),
            }),
            tenant_b,
        );
        cm.dispatch_event(&envelope).await;

        assert!(
            rx.try_recv().is_err(),
            "different tenant should NOT receive event"
        );
    }

    #[tokio::test]
    async fn test_tenant_isolation_broadcast_event_reaches_all() {
        let cm = ConnectionManager::new(test_host());
        let tenant_a = Uuid::new_v4();

        let (conn_id, mut rx) = cm.connect(Some(tenant_a)).await;
        cm.subscribe(&conn_id, SubscriptionFilter::default())
            .await
            .unwrap();

        // Event without tenant_id (broadcast) — should be received by everyone
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "system".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        cm.dispatch_event(&envelope).await;

        assert!(
            rx.try_recv().is_ok(),
            "broadcast event should reach all tenants"
        );
    }

    #[tokio::test]
    async fn test_tenant_isolation_anonymous_receives_all() {
        let cm = ConnectionManager::new(test_host());
        let tenant_a = Uuid::new_v4();

        // Anonymous connection (no tenant)
        let (conn_id, mut rx) = cm.connect(None).await;
        cm.subscribe(&conn_id, SubscriptionFilter::default())
            .await
            .unwrap();

        // Tenant-scoped event — anonymous should still receive it
        let envelope = EventEnvelope::new_with_tenant(
            FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({}),
            }),
            tenant_a,
        );
        cm.dispatch_event(&envelope).await;

        assert!(
            rx.try_recv().is_ok(),
            "anonymous connection should receive all events"
        );
    }

    #[tokio::test]
    async fn test_tenant_isolation_multi_tenant_dispatch() {
        let cm = ConnectionManager::new(test_host());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();

        let (conn_a, mut rx_a) = cm.connect(Some(tenant_a)).await;
        let (conn_b, mut rx_b) = cm.connect(Some(tenant_b)).await;

        cm.subscribe(&conn_a, SubscriptionFilter::default())
            .await
            .unwrap();
        cm.subscribe(&conn_b, SubscriptionFilter::default())
            .await
            .unwrap();

        // Publish event for tenant A
        let envelope_a = EventEnvelope::new_with_tenant(
            FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({}),
            }),
            tenant_a,
        );
        cm.dispatch_event(&envelope_a).await;

        assert!(rx_a.try_recv().is_ok(), "tenant A should receive its event");
        assert!(
            rx_b.try_recv().is_err(),
            "tenant B should NOT receive tenant A's event"
        );
    }

    // -----------------------------------------------------------------------
    // Custom message handler tests
    // -----------------------------------------------------------------------

    /// Messages recorded by [`RecordingHandler`]: (user_id, raw_text).
    type SeenMessages = Arc<RwLock<Vec<(Option<String>, String)>>>;

    /// Test handler that records received messages and optionally consumes them.
    struct RecordingHandler {
        seen: SeenMessages,
        consume: bool,
    }

    impl CustomMessageHandler for RecordingHandler {
        fn handle(
            &self,
            _connection_id: &str,
            user_id: Option<&str>,
            raw_text: &str,
            sender: mpsc::UnboundedSender<ServerMessage>,
        ) -> CustomHandlerResult {
            let seen = self.seen.clone();
            let consume = self.consume;
            let user = user_id.map(|u| u.to_string());
            let text = raw_text.to_string();
            Box::pin(async move {
                seen.write().await.push((user, text));
                if consume {
                    let _ = sender.send(ServerMessage::Error {
                        message: "ack".to_string(),
                    });
                }
                consume
            })
        }
    }

    #[tokio::test]
    async fn test_custom_handler_not_registered_returns_false() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, _rx) = cm.connect(None).await;

        let handled = cm.try_custom_handler(&conn_id, r#"{"type":"chat"}"#).await;
        assert!(!handled, "no handler registered → message not consumed");
    }

    #[tokio::test]
    async fn test_custom_handler_consumes_message_and_replies() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, mut rx) = cm.connect(None).await;
        cm.associate_user(&conn_id, "user_42".to_string())
            .await
            .unwrap();

        let seen = Arc::new(RwLock::new(Vec::new()));
        cm.set_custom_handler(Arc::new(RecordingHandler {
            seen: seen.clone(),
            consume: true,
        }))
        .await;

        let raw = r#"{"type":"chat","body":"hello"}"#;
        let handled = cm.try_custom_handler(&conn_id, raw).await;
        assert!(handled, "handler should consume the message");

        // The handler saw the raw text and the associated user
        let recorded = seen.read().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0.as_deref(), Some("user_42"));
        assert_eq!(recorded[0].1, raw);
        drop(recorded);

        // The handler's reply reached this connection's channel
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Error { message } => assert_eq!(message, "ack"),
            other => panic!("Expected handler ack, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_custom_handler_declining_returns_false() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, _rx) = cm.connect(None).await;

        cm.set_custom_handler(Arc::new(RecordingHandler {
            seen: Arc::new(RwLock::new(Vec::new())),
            consume: false,
        }))
        .await;

        let handled = cm.try_custom_handler(&conn_id, "not-json").await;
        assert!(
            !handled,
            "handler declined → caller falls back to error path"
        );
    }

    #[tokio::test]
    async fn test_custom_handler_unknown_connection_returns_false() {
        let cm = ConnectionManager::new(test_host());

        cm.set_custom_handler(Arc::new(RecordingHandler {
            seen: Arc::new(RwLock::new(Vec::new())),
            consume: true,
        }))
        .await;

        let handled = cm
            .try_custom_handler("conn_nonexistent", r#"{"type":"chat"}"#)
            .await;
        assert!(!handled, "unknown connection → message not consumed");
    }
}
