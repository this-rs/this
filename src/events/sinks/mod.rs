//! Sink system — destinations for processed events
//!
//! Sinks are the final step in a pipeline: the `deliver` operator dispatches
//! the `_payload` to one or more registered sinks by name.
//!
//! # Architecture
//!
//! ```text
//! FlowRuntime → Pipeline → DeliverOp → SinkRegistry → Sink::deliver()
//! ```
//!
//! # Sink types
//!
//! - `InApp` — In-app notification store (list, mark_as_read, unread_count)
//! - `Push` — Push notifications (Expo/APNs/FCM) [Plan 3, T3.2]
//! - `WebSocket` — Real-time dispatch to connected clients [Plan 3, T3.3]
//! - `Webhook` — HTTP POST to external URLs [Plan 3, T3.3]
//! - `Counter` — Counter update on entity fields [Plan 3, T3.3]

pub mod in_app;
pub mod preferences;

pub use in_app::InAppNotificationSink;
pub use preferences::{NotificationPreferencesStore, UserPreferences};

use crate::config::sinks::SinkType;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Trait for event sinks — destinations that receive processed events
///
/// Each sink is registered in the `SinkRegistry` by name (matching the
/// YAML `sinks[].name` field). The `deliver` operator looks up sinks
/// by name and calls `deliver()` with the payload.
///
/// # Object Safety
///
/// This trait is object-safe: no generics, all methods take `&self`.
/// It can be used as `Arc<dyn Sink>`.
#[async_trait]
pub trait Sink: Send + Sync + std::fmt::Debug {
    /// Deliver a payload to this sink
    ///
    /// - `payload`: The JSON payload built by the `map` operator
    /// - `recipient_id`: Optional recipient (e.g., user ID for notifications)
    /// - `context_vars`: Additional context variables from the pipeline
    async fn deliver(
        &self,
        payload: Value,
        recipient_id: Option<&str>,
        context_vars: &HashMap<String, Value>,
    ) -> Result<()>;

    /// Human-readable name for this sink instance
    fn name(&self) -> &str;

    /// The sink type (matches SinkConfig.sink_type)
    fn sink_type(&self) -> SinkType;
}

/// Registry of named sinks
///
/// The SinkRegistry maps sink names (from YAML config) to sink
/// implementations. The `deliver` operator uses this to dispatch
/// payloads to the correct sinks.
///
/// # Thread Safety
///
/// The registry is immutable after construction — sinks are registered
/// during startup and never removed. No locking needed for reads.
#[derive(Debug)]
pub struct SinkRegistry {
    sinks: HashMap<String, Arc<dyn Sink>>,
}

impl SinkRegistry {
    /// Create an empty sink registry
    pub fn new() -> Self {
        Self {
            sinks: HashMap::new(),
        }
    }

    /// Register a sink by name
    ///
    /// If a sink with the same name already exists, it is replaced.
    pub fn register(&mut self, name: impl Into<String>, sink: Arc<dyn Sink>) {
        self.sinks.insert(name.into(), sink);
    }

    /// Look up a sink by name
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Sink>> {
        self.sinks.get(name)
    }

    /// Get all registered sink names
    pub fn names(&self) -> Vec<&str> {
        self.sinks.keys().map(|s| s.as_str()).collect()
    }

    /// Deliver a payload to a named sink
    ///
    /// Returns an error if the sink is not found.
    pub async fn deliver(
        &self,
        sink_name: &str,
        payload: Value,
        recipient_id: Option<&str>,
        context_vars: &HashMap<String, Value>,
    ) -> Result<()> {
        let sink = self
            .sinks
            .get(sink_name)
            .ok_or_else(|| anyhow::anyhow!("sink '{}' not found in registry", sink_name))?;

        sink.deliver(payload, recipient_id, context_vars).await
    }

    /// Number of registered sinks
    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    /// Whether the registry is empty
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl Default for SinkRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A simple test sink that records deliveries
    #[derive(Debug)]
    struct TestSink {
        sink_name: String,
        deliveries: Arc<tokio::sync::Mutex<Vec<(Value, Option<String>)>>>,
    }

    impl TestSink {
        fn new(name: &str) -> Self {
            Self {
                sink_name: name.to_string(),
                deliveries: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl Sink for TestSink {
        async fn deliver(
            &self,
            payload: Value,
            recipient_id: Option<&str>,
            _context_vars: &HashMap<String, Value>,
        ) -> Result<()> {
            self.deliveries
                .lock()
                .await
                .push((payload, recipient_id.map(|s| s.to_string())));
            Ok(())
        }

        fn name(&self) -> &str {
            &self.sink_name
        }

        fn sink_type(&self) -> SinkType {
            SinkType::Custom
        }
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = SinkRegistry::new();
        let sink = Arc::new(TestSink::new("test-sink"));
        registry.register("test-sink", sink);

        assert_eq!(registry.len(), 1);
        assert!(registry.get("test-sink").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_names() {
        let mut registry = SinkRegistry::new();
        registry.register("a", Arc::new(TestSink::new("a")));
        registry.register("b", Arc::new(TestSink::new("b")));

        let mut names = registry.names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_registry_deliver() {
        let mut registry = SinkRegistry::new();
        let sink = Arc::new(TestSink::new("test-sink"));
        let deliveries = sink.deliveries.clone();
        registry.register("test-sink", sink);

        let payload = json!({"title": "Hello", "body": "World"});
        registry
            .deliver("test-sink", payload.clone(), Some("user-1"), &HashMap::new())
            .await
            .unwrap();

        let recorded = deliveries.lock().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, payload);
        assert_eq!(recorded[0].1.as_deref(), Some("user-1"));
    }

    #[tokio::test]
    async fn test_registry_deliver_unknown_sink() {
        let registry = SinkRegistry::new();

        let result = registry
            .deliver("nonexistent", json!({}), None, &HashMap::new())
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nonexistent"));
    }

    #[test]
    fn test_registry_replace_sink() {
        let mut registry = SinkRegistry::new();
        registry.register("s", Arc::new(TestSink::new("s-v1")));
        registry.register("s", Arc::new(TestSink::new("s-v2")));

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get("s").unwrap().name(), "s-v2");
    }

    #[test]
    fn test_registry_default_is_empty() {
        let registry = SinkRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}
