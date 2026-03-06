//! WebSocket sink — dispatches events to connected clients in real-time
//!
//! This sink publishes processed events to connected WebSocket clients.
//! It uses the `WebSocketDispatcher` trait to abstract the actual
//! connection management (which lives in the server layer).
//!
//! # Filtering
//!
//! Events are dispatched to the recipient's connections only. The
//! dispatcher implementation (in the server layer) is responsible for
//! matching connections to recipients via subscription filters.
//!
//! ```yaml
//! - deliver:
//!     sink: live-updates
//! ```

use crate::config::sinks::SinkType;
use crate::events::sinks::Sink;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Trait for dispatching events to WebSocket connections
///
/// This trait abstracts the server-layer `ConnectionManager` so the
/// sink can be used without depending on the server module.
///
/// The server layer provides the concrete implementation that maps
/// recipient IDs to WebSocket connections.
#[async_trait]
pub trait WebSocketDispatcher: Send + Sync + std::fmt::Debug {
    /// Dispatch a payload to a specific recipient's connections
    ///
    /// The implementation should find all WebSocket connections belonging
    /// to the recipient and send the payload to each of them.
    ///
    /// Returns the number of connections that received the message.
    async fn dispatch_to_recipient(
        &self,
        recipient_id: &str,
        payload: Value,
    ) -> Result<usize>;

    /// Broadcast a payload to ALL connected clients
    ///
    /// Used when no recipient_id is specified.
    async fn broadcast(&self, payload: Value) -> Result<usize>;
}

/// WebSocket notification sink
///
/// Receives payloads from the `deliver` operator and dispatches them
/// to connected WebSocket clients via the `WebSocketDispatcher`.
#[derive(Debug)]
pub struct WebSocketSink {
    /// Dispatcher for WebSocket connections
    dispatcher: Arc<dyn WebSocketDispatcher>,
}

impl WebSocketSink {
    /// Create a new WebSocketSink with a dispatcher
    pub fn new(dispatcher: Arc<dyn WebSocketDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Sink for WebSocketSink {
    async fn deliver(
        &self,
        payload: Value,
        recipient_id: Option<&str>,
        context_vars: &HashMap<String, Value>,
    ) -> Result<()> {
        // Determine recipient (optional — if None, broadcast to all)
        let recipient = super::resolve_recipient(recipient_id, &payload, context_vars);

        let count = match &recipient {
            Some(rid) => {
                tracing::debug!(
                    recipient = %rid,
                    "websocket sink: dispatching to recipient connections"
                );
                self.dispatcher.dispatch_to_recipient(rid, payload).await?
            }
            None => {
                tracing::debug!("websocket sink: broadcasting to all connections");
                self.dispatcher.broadcast(payload).await?
            }
        };

        tracing::debug!(
            connections = count,
            "websocket sink: dispatched to connections"
        );

        Ok(())
    }

    fn name(&self) -> &str {
        "websocket"
    }

    fn sink_type(&self) -> SinkType {
        SinkType::WebSocket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    /// Mock dispatcher that records calls
    #[derive(Debug)]
    struct MockDispatcher {
        dispatched: Mutex<Vec<(Option<String>, Value)>>,
        dispatch_count: AtomicUsize,
    }

    impl MockDispatcher {
        fn new() -> Self {
            Self {
                dispatched: Mutex::new(Vec::new()),
                dispatch_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl WebSocketDispatcher for MockDispatcher {
        async fn dispatch_to_recipient(
            &self,
            recipient_id: &str,
            payload: Value,
        ) -> Result<usize> {
            self.dispatched
                .lock()
                .await
                .push((Some(recipient_id.to_string()), payload));
            let count = self.dispatch_count.load(Ordering::SeqCst);
            Ok(if count > 0 { count } else { 1 })
        }

        async fn broadcast(&self, payload: Value) -> Result<usize> {
            self.dispatched.lock().await.push((None, payload));
            let count = self.dispatch_count.load(Ordering::SeqCst);
            Ok(if count > 0 { count } else { 1 })
        }
    }

    #[tokio::test]
    async fn test_ws_deliver_to_recipient() {
        let dispatcher = Arc::new(MockDispatcher::new());
        let sink = WebSocketSink::new(dispatcher.clone());

        let payload = json!({
            "title": "New follower",
            "body": "Alice followed you",
            "recipient_id": "user-A"
        });

        sink.deliver(payload.clone(), None, &HashMap::new())
            .await
            .unwrap();

        let dispatched = dispatcher.dispatched.lock().await;
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].0.as_deref(), Some("user-A"));
        assert_eq!(dispatched[0].1["title"], "New follower");
    }

    #[tokio::test]
    async fn test_ws_deliver_explicit_recipient() {
        let dispatcher = Arc::new(MockDispatcher::new());
        let sink = WebSocketSink::new(dispatcher.clone());

        let payload = json!({"title": "Test"});

        sink.deliver(payload, Some("user-B"), &HashMap::new())
            .await
            .unwrap();

        let dispatched = dispatcher.dispatched.lock().await;
        assert_eq!(dispatched[0].0.as_deref(), Some("user-B"));
    }

    #[tokio::test]
    async fn test_ws_broadcast_when_no_recipient() {
        let dispatcher = Arc::new(MockDispatcher::new());
        let sink = WebSocketSink::new(dispatcher.clone());

        // No recipient_id anywhere → broadcast
        let payload = json!({"title": "System announcement"});

        sink.deliver(payload, None, &HashMap::new())
            .await
            .unwrap();

        let dispatched = dispatcher.dispatched.lock().await;
        assert_eq!(dispatched.len(), 1);
        assert!(dispatched[0].0.is_none()); // No recipient = broadcast
    }

    #[tokio::test]
    async fn test_ws_recipient_from_context() {
        let dispatcher = Arc::new(MockDispatcher::new());
        let sink = WebSocketSink::new(dispatcher.clone());

        let payload = json!({"title": "Test"});
        let mut vars = HashMap::new();
        vars.insert(
            "recipient_id".to_string(),
            Value::String("user-C".to_string()),
        );

        sink.deliver(payload, None, &vars).await.unwrap();

        let dispatched = dispatcher.dispatched.lock().await;
        assert_eq!(dispatched[0].0.as_deref(), Some("user-C"));
    }

    #[test]
    fn test_ws_sink_name_and_type() {
        let dispatcher = Arc::new(MockDispatcher::new());
        let sink = WebSocketSink::new(dispatcher);
        assert_eq!(sink.name(), "websocket");
        assert_eq!(sink.sink_type(), SinkType::WebSocket);
    }
}
