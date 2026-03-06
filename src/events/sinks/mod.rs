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

pub mod counter;
pub mod device_tokens;
pub mod in_app;
pub mod preferences;
pub mod push;
pub mod webhook;
pub mod websocket;

pub use counter::{CounterConfig, CounterOperation, CounterSink, EntityFieldUpdater};
pub use device_tokens::{DeviceToken, DeviceTokenStore, Platform};
pub use in_app::{InAppNotificationSink, NotificationStore};
pub use preferences::{NotificationPreferencesStore, UserPreferences};
#[cfg(feature = "push")]
pub use push::ExpoPushProvider;
pub use push::{PushNotificationSink, PushProvider};
pub use webhook::{HttpSender, WebhookConfig, WebhookSink};
pub use websocket::{WebSocketDispatcher, WebSocketSink};

use crate::config::sinks::SinkType;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Resolve the recipient ID from multiple sources
///
/// Priority: explicit parameter > payload field > context variable.
/// Returns `None` if no recipient ID is found in any source.
///
/// Shared by all sinks that need a recipient (in_app, push, websocket).
pub fn resolve_recipient(
    explicit: Option<&str>,
    payload: &Value,
    context_vars: &HashMap<String, Value>,
) -> Option<String> {
    explicit
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("recipient_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            context_vars
                .get("recipient_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

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
/// Uses interior mutability (`RwLock`) so that sinks can be registered
/// after initial construction (e.g., the WebSocket sink is wired when
/// `WebSocketExposure::build_router()` is called, after the host is
/// already wrapped in `Arc`).
#[derive(Debug)]
pub struct SinkRegistry {
    sinks: RwLock<HashMap<String, Arc<dyn Sink>>>,
}

impl SinkRegistry {
    /// Create an empty sink registry
    pub fn new() -> Self {
        Self {
            sinks: RwLock::new(HashMap::new()),
        }
    }

    /// Register a sink by name
    ///
    /// If a sink with the same name already exists, it is replaced.
    /// This method uses interior mutability so it can be called through
    /// `&self` (even behind `Arc`).
    pub fn register(&self, name: impl Into<String>, sink: Arc<dyn Sink>) {
        self.sinks.write().unwrap().insert(name.into(), sink);
    }

    /// Look up a sink by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Sink>> {
        self.sinks.read().unwrap().get(name).cloned()
    }

    /// Get all registered sink names
    pub fn names(&self) -> Vec<String> {
        self.sinks.read().unwrap().keys().cloned().collect()
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
            .get(sink_name)
            .ok_or_else(|| anyhow::anyhow!("sink '{}' not found in registry", sink_name))?;

        sink.deliver(payload, recipient_id, context_vars).await
    }

    /// Number of registered sinks
    pub fn len(&self) -> usize {
        self.sinks.read().unwrap().len()
    }

    /// Whether the registry is empty
    pub fn is_empty(&self) -> bool {
        self.sinks.read().unwrap().is_empty()
    }
}

impl Default for SinkRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Factory for creating sinks from YAML configuration
///
/// Builds sink instances from `SinkConfig` entries. Some sinks can be
/// auto-created (InApp), while others require external dependencies
/// provided by the user (Push needs PushProvider, Counter needs EntityFieldUpdater,
/// WebSocket needs WebSocketDispatcher, Webhook needs HttpSender).
///
/// Sinks that cannot be auto-created are logged as warnings and skipped.
pub struct SinkFactory {
    /// Shared notification store (created once, reused by all InApp sinks)
    notification_store: Arc<NotificationStore>,

    /// Shared preferences store
    preferences_store: Arc<NotificationPreferencesStore>,

    /// Shared device token store
    device_token_store: Arc<DeviceTokenStore>,
}

impl SinkFactory {
    /// Create a new SinkFactory with fresh stores
    pub fn new() -> Self {
        Self {
            notification_store: Arc::new(NotificationStore::new()),
            preferences_store: Arc::new(NotificationPreferencesStore::new()),
            device_token_store: Arc::new(DeviceTokenStore::new()),
        }
    }

    /// Create a SinkFactory with pre-existing stores
    pub fn with_stores(
        notification_store: Arc<NotificationStore>,
        preferences_store: Arc<NotificationPreferencesStore>,
        device_token_store: Arc<DeviceTokenStore>,
    ) -> Self {
        Self {
            notification_store,
            preferences_store,
            device_token_store,
        }
    }

    /// Get the notification store (for sharing with ServerHost)
    pub fn notification_store(&self) -> &Arc<NotificationStore> {
        &self.notification_store
    }

    /// Get the preferences store (for sharing with ServerHost)
    pub fn preferences_store(&self) -> &Arc<NotificationPreferencesStore> {
        &self.preferences_store
    }

    /// Get the device token store (for sharing with ServerHost)
    pub fn device_token_store(&self) -> &Arc<DeviceTokenStore> {
        &self.device_token_store
    }

    /// Build a SinkRegistry from a list of SinkConfigs
    ///
    /// Auto-creates sinks that don't need external dependencies (InApp).
    /// Logs warnings for sinks that need manual wiring (Push, WebSocket,
    /// Counter, Webhook).
    pub fn build_registry(
        &self,
        sink_configs: &[crate::config::sinks::SinkConfig],
    ) -> SinkRegistry {
        let registry = SinkRegistry::new();

        for config in sink_configs {
            match config.sink_type {
                SinkType::InApp => {
                    let sink = InAppNotificationSink::with_preferences(
                        self.notification_store.clone(),
                        self.preferences_store.clone(),
                    );
                    registry.register(&config.name, Arc::new(sink));
                    tracing::info!(
                        sink = %config.name,
                        "auto-wired InApp notification sink"
                    );
                }
                SinkType::Push => {
                    tracing::warn!(
                        sink = %config.name,
                        "Push sink requires a PushProvider — use ServerBuilder::with_push_provider() to wire it"
                    );
                }
                SinkType::WebSocket => {
                    tracing::warn!(
                        sink = %config.name,
                        "WebSocket sink will be wired automatically when WebSocketExposure is built"
                    );
                }
                SinkType::Webhook => {
                    tracing::warn!(
                        sink = %config.name,
                        "Webhook sink requires an HttpSender implementation — skipping auto-wire"
                    );
                }
                SinkType::Counter => {
                    tracing::warn!(
                        sink = %config.name,
                        "Counter sink requires an EntityFieldUpdater — use ServerBuilder::with_counter_updater() to wire it"
                    );
                }
                SinkType::Feed => {
                    tracing::warn!(
                        sink = %config.name,
                        "Feed sink is not yet implemented — skipping"
                    );
                }
                SinkType::Custom => {
                    tracing::warn!(
                        sink = %config.name,
                        "Custom sink requires manual registration — skipping auto-wire"
                    );
                }
            }
        }

        registry
    }
}

impl Default for SinkFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type DeliveryLog = Vec<(Value, Option<String>)>;

    /// A simple test sink that records deliveries
    #[derive(Debug)]
    struct TestSink {
        sink_name: String,
        deliveries: Arc<tokio::sync::Mutex<DeliveryLog>>,
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
        let registry = SinkRegistry::new();
        let sink = Arc::new(TestSink::new("test-sink"));
        registry.register("test-sink", sink);

        assert_eq!(registry.len(), 1);
        assert!(registry.get("test-sink").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_names() {
        let registry = SinkRegistry::new();
        registry.register("a", Arc::new(TestSink::new("a")));
        registry.register("b", Arc::new(TestSink::new("b")));

        let mut names = registry.names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_registry_deliver() {
        let registry = SinkRegistry::new();
        let sink = Arc::new(TestSink::new("test-sink"));
        let deliveries = sink.deliveries.clone();
        registry.register("test-sink", sink);

        let payload = json!({"title": "Hello", "body": "World"});
        registry
            .deliver(
                "test-sink",
                payload.clone(),
                Some("user-1"),
                &HashMap::new(),
            )
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
        let registry = SinkRegistry::new();
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
