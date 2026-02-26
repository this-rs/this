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
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};
use uuid::Uuid;

/// A handle to a single WebSocket connection
///
/// Each connection has a unique ID and a sender channel to push messages
/// to the client's WebSocket write loop.
struct ConnectionHandle {
    /// Sender to push ServerMessage to the client's write loop
    tx: mpsc::UnboundedSender<ServerMessage>,
    /// Active subscriptions for this connection
    subscriptions: Vec<Subscription>,
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
}

impl ConnectionManager {
    /// Create a new ConnectionManager
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self {
            _host: host,
            connections: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new WebSocket connection
    ///
    /// Returns a tuple of (connection_id, receiver) where the receiver
    /// will receive `ServerMessage`s to forward to the client.
    pub async fn connect(&self) -> (String, mpsc::UnboundedReceiver<ServerMessage>) {
        let connection_id = format!("conn_{}", Uuid::new_v4().simple());
        let (tx, rx) = mpsc::unbounded_channel();

        let handle = ConnectionHandle {
            tx,
            subscriptions: Vec::new(),
        };

        self.connections
            .write()
            .await
            .insert(connection_id.clone(), handle);

        tracing::debug!(connection_id = %connection_id, "WebSocket client connected");

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

    /// Dispatch an event to all matching subscriptions across all connections
    ///
    /// For each connection, check every subscription filter against the event.
    /// If a subscription matches, send the event to that connection with the
    /// subscription ID attached.
    async fn dispatch_event(&self, envelope: &EventEnvelope) {
        let connections = self.connections.read().await;

        for (connection_id, handle) in connections.iter() {
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

        let (conn_id, _rx) = cm.connect().await;
        assert!(conn_id.starts_with("conn_"));
        assert_eq!(cm.connection_count().await, 1);

        cm.disconnect(&conn_id).await;
        assert_eq!(cm.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_subscribe_and_unsubscribe() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, _rx) = cm.connect().await;

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
        let (conn_id, mut rx) = cm.connect().await;

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
        let (conn_id, mut rx) = cm.connect().await;

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
        let (conn_id, mut rx) = cm.connect().await;

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
        let (conn_id, mut rx) = cm.connect().await;

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

        let (conn1_id, mut rx1) = cm.connect().await;
        let (conn2_id, mut rx2) = cm.connect().await;

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
            (
                ServerMessage::Event { data: d1, .. },
                ServerMessage::Event { data: d2, .. },
            ) => {
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
        cm.send_to(
            "conn_does_not_exist",
            ServerMessage::Pong,
        )
        .await;

        // Verify manager is still functional
        assert_eq!(cm.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_dead_connection_handling() {
        let cm = ConnectionManager::new(test_host());
        let (conn_id, rx) = cm.connect().await;

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
        let (conn_id, mut rx) = cm.connect().await;

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
}
