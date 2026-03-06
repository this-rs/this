//! Bridge between the server-layer ConnectionManager and the event-layer WebSocketSink
//!
//! The `ConnectionManagerDispatcher` wraps an `Arc<ConnectionManager>` and implements
//! the `WebSocketDispatcher` trait (defined in `events::sinks::websocket`). This allows
//! the `WebSocketSink` to dispatch payloads to connected clients without depending
//! directly on the server module.
//!
//! # Architecture
//!
//! ```text
//! FlowRuntime
//!   └─ DeliverOp → SinkRegistry
//!        └─ WebSocketSink (events layer)
//!             └─ WebSocketDispatcher trait
//!                  └─ ConnectionManagerDispatcher (this module)
//!                       └─ ConnectionManager (server layer)
//!                            └─ send_to_user() / broadcast_payload()
//! ```

use super::manager::ConnectionManager;
use crate::events::sinks::websocket::WebSocketDispatcher;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// Adapter that bridges `ConnectionManager` to the `WebSocketDispatcher` trait
///
/// This struct is created by `WebSocketExposure::build_router()` and passed
/// to the `WebSocketSink` so the event pipeline can dispatch notifications
/// to connected WebSocket clients.
#[derive(Debug)]
pub struct ConnectionManagerDispatcher {
    manager: Arc<ConnectionManager>,
}

impl ConnectionManagerDispatcher {
    /// Create a new dispatcher wrapping a `ConnectionManager`
    pub fn new(manager: Arc<ConnectionManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl WebSocketDispatcher for ConnectionManagerDispatcher {
    /// Dispatch a payload to all connections belonging to a specific user
    ///
    /// Maps `recipient_id` → `ConnectionManager::send_to_user()`.
    /// Returns the number of connections that received the message.
    async fn dispatch_to_recipient(&self, recipient_id: &str, payload: Value) -> Result<usize> {
        Ok(self.manager.send_to_user(recipient_id, payload).await)
    }

    /// Broadcast a payload to ALL connected clients
    ///
    /// Maps to `ConnectionManager::broadcast_payload()`.
    /// Returns the number of connections that received the message.
    async fn broadcast(&self, payload: Value) -> Result<usize> {
        Ok(self.manager.broadcast_payload(payload).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::server::entity_registry::EntityRegistry;
    use crate::server::host::ServerHost;
    use crate::storage::InMemoryLinkService;
    use serde_json::json;
    use std::collections::HashMap;

    fn test_host() -> Arc<ServerHost> {
        let host = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .expect("should build host");
        Arc::new(host)
    }

    #[tokio::test]
    async fn test_dispatcher_dispatch_to_recipient() {
        let cm = Arc::new(ConnectionManager::new(test_host()));
        let dispatcher = ConnectionManagerDispatcher::new(cm.clone());

        // Create a connection and associate a user
        let (conn_id, mut rx) = cm.connect().await;
        cm.associate_user(&conn_id, "user-42".to_string())
            .await
            .unwrap();

        // Dispatch via the trait
        let payload = json!({"title": "You have a new message"});
        let count = dispatcher
            .dispatch_to_recipient("user-42", payload.clone())
            .await
            .unwrap();

        assert_eq!(count, 1);
        let msg = rx.try_recv().expect("should receive notification");
        match msg {
            crate::server::exposure::websocket::protocol::ServerMessage::Notification { data } => {
                assert_eq!(data["title"], "You have a new message");
            }
            other => panic!("Expected Notification, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_dispatch_to_unknown_recipient() {
        let cm = Arc::new(ConnectionManager::new(test_host()));
        let dispatcher = ConnectionManagerDispatcher::new(cm);

        let count = dispatcher
            .dispatch_to_recipient("unknown-user", json!({}))
            .await
            .unwrap();

        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_dispatcher_broadcast() {
        let cm = Arc::new(ConnectionManager::new(test_host()));
        let dispatcher = ConnectionManagerDispatcher::new(cm.clone());

        let (_conn1, mut rx1) = cm.connect().await;
        let (_conn2, mut rx2) = cm.connect().await;

        let payload = json!({"message": "System update"});
        let count = dispatcher.broadcast(payload).await.unwrap();

        assert_eq!(count, 2);
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[tokio::test]
    async fn test_dispatcher_broadcast_empty() {
        let cm = Arc::new(ConnectionManager::new(test_host()));
        let dispatcher = ConnectionManagerDispatcher::new(cm);

        let count = dispatcher.broadcast(json!({})).await.unwrap();
        assert_eq!(count, 0);
    }
}
