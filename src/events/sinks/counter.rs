//! Counter sink — updates numeric fields on entities
//!
//! Increments, decrements, or sets a numeric field on an entity.
//! Useful for maintaining derived counters like `follower_count`,
//! `like_count`, etc. in response to events.
//!
//! ```yaml
//! sinks:
//!   - name: like-counter
//!     type: counter
//!     config:
//!       field: like_count
//!       operation: increment
//! ```
//!
//! # Payload format
//!
//! The payload (from `map` operator) must include:
//! - `entity_type`: Type of the entity to update
//! - `entity_id`: ID of the entity to update
//!
//! The field name and operation come from the sink configuration
//! or can be overridden in the payload:
//! - `field`: Name of the numeric field (default from config)
//! - `operation`: "increment", "decrement", or "set" (default from config)
//! - `value`: Amount to increment/decrement or value to set (default: 1)

use crate::config::sinks::SinkType;
use crate::events::sinks::Sink;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Counter operations
#[derive(Debug, Clone, PartialEq)]
pub enum CounterOperation {
    /// Add a value to the current count
    Increment,
    /// Subtract a value from the current count
    Decrement,
    /// Set the counter to an absolute value
    Set,
}

impl CounterOperation {
    /// Parse from a string
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "increment" | "inc" | "add" => Ok(Self::Increment),
            "decrement" | "dec" | "sub" | "subtract" => Ok(Self::Decrement),
            "set" => Ok(Self::Set),
            _ => Err(anyhow!(
                "invalid counter operation '{}': expected 'increment', 'decrement', or 'set'",
                s
            )),
        }
    }

    /// Apply the operation to a current value
    pub fn apply(&self, current: f64, amount: f64) -> f64 {
        match self {
            Self::Increment => current + amount,
            Self::Decrement => (current - amount).max(0.0), // Never go negative
            Self::Set => amount,
        }
    }
}

/// Trait for reading and updating entity fields
///
/// Abstracts the entity storage so the counter sink can work
/// without depending on the server layer.
#[async_trait]
pub trait EntityFieldUpdater: Send + Sync + std::fmt::Debug {
    /// Read a numeric field from an entity
    ///
    /// Returns the current field value, or 0.0 if the field doesn't exist.
    async fn read_field(&self, entity_type: &str, entity_id: &str, field: &str) -> Result<f64>;

    /// Write a numeric field to an entity
    async fn write_field(
        &self,
        entity_type: &str,
        entity_id: &str,
        field: &str,
        value: f64,
    ) -> Result<()>;
}

/// Counter sink configuration
#[derive(Debug, Clone)]
pub struct CounterConfig {
    /// Default field name to update
    pub field: String,

    /// Default operation
    pub operation: CounterOperation,
}

/// Counter notification sink
///
/// Updates numeric fields on entities. Used for maintaining derived
/// counters (follower_count, like_count, etc.) in response to events.
///
/// Uses per-key locks to ensure atomic read-modify-write operations,
/// preventing TOCTOU race conditions under concurrent access.
#[derive(Debug)]
pub struct CounterSink {
    /// Default counter configuration
    config: CounterConfig,

    /// Entity field updater
    updater: Arc<dyn EntityFieldUpdater>,

    /// Per-key locks for atomic read-modify-write
    /// Key format: "entity_type:entity_id:field"
    key_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
}

impl CounterSink {
    /// Create a new CounterSink
    pub fn new(updater: Arc<dyn EntityFieldUpdater>, config: CounterConfig) -> Self {
        Self {
            config,
            updater,
            key_locks: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a lock for the given key
    async fn get_lock(&self, key: &str) -> Arc<Mutex<()>> {
        // Fast path: check if lock already exists (read lock)
        {
            let locks = self.key_locks.read().await;
            if let Some(lock) = locks.get(key) {
                return lock.clone();
            }
        }

        // Slow path: create the lock (write lock)
        let mut locks = self.key_locks.write().await;
        // Double-check after acquiring write lock
        locks
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[async_trait]
impl Sink for CounterSink {
    async fn deliver(
        &self,
        payload: Value,
        _recipient_id: Option<&str>,
        context_vars: &HashMap<String, Value>,
    ) -> Result<()> {
        // Extract entity_type and entity_id from payload or context
        let entity_type = payload
            .get("entity_type")
            .and_then(|v| v.as_str())
            .or_else(|| context_vars.get("entity_type").and_then(|v| v.as_str()))
            .ok_or_else(|| anyhow!("counter sink: entity_type not found in payload or context"))?
            .to_string();

        let entity_id = payload
            .get("entity_id")
            .and_then(|v| v.as_str())
            .or_else(|| context_vars.get("entity_id").and_then(|v| v.as_str()))
            .ok_or_else(|| anyhow!("counter sink: entity_id not found in payload or context"))?
            .to_string();

        // Field name: payload overrides config default
        let field = payload
            .get("field")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.config.field)
            .to_string();

        // Operation: payload overrides config default
        let operation = if let Some(op_str) = payload.get("operation").and_then(|v| v.as_str()) {
            CounterOperation::parse(op_str)?
        } else {
            self.config.operation.clone()
        };

        // Value: default 1
        let amount = payload.get("value").and_then(|v| v.as_f64()).unwrap_or(1.0);

        // Acquire per-key lock for atomic read-modify-write
        let lock_key = format!("{}:{}:{}", entity_type, entity_id, field);
        let lock = self.get_lock(&lock_key).await;
        let _guard = lock.lock().await;

        // Read current value
        let current = self
            .updater
            .read_field(&entity_type, &entity_id, &field)
            .await?;

        // Apply operation
        let new_value = operation.apply(current, amount);

        tracing::debug!(
            entity_type = %entity_type,
            entity_id = %entity_id,
            field = %field,
            current = current,
            operation = ?operation,
            amount = amount,
            new_value = new_value,
            "counter sink: updating field"
        );

        // Write new value
        self.updater
            .write_field(&entity_type, &entity_id, &field, new_value)
            .await?;

        Ok(())
    }

    fn name(&self) -> &str {
        "counter"
    }

    fn sink_type(&self) -> SinkType {
        SinkType::Counter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::RwLock;

    /// Mock entity storage
    #[derive(Debug)]
    struct MockEntityStore {
        /// Fields keyed by "entity_type:entity_id:field"
        fields: RwLock<HashMap<String, f64>>,
    }

    impl MockEntityStore {
        fn new() -> Self {
            Self {
                fields: RwLock::new(HashMap::new()),
            }
        }

        fn key(entity_type: &str, entity_id: &str, field: &str) -> String {
            format!("{}:{}:{}", entity_type, entity_id, field)
        }

        async fn set(&self, entity_type: &str, entity_id: &str, field: &str, value: f64) {
            self.fields
                .write()
                .await
                .insert(Self::key(entity_type, entity_id, field), value);
        }
    }

    #[async_trait]
    impl EntityFieldUpdater for MockEntityStore {
        async fn read_field(&self, entity_type: &str, entity_id: &str, field: &str) -> Result<f64> {
            let store = self.fields.read().await;
            Ok(*store
                .get(&Self::key(entity_type, entity_id, field))
                .unwrap_or(&0.0))
        }

        async fn write_field(
            &self,
            entity_type: &str,
            entity_id: &str,
            field: &str,
            value: f64,
        ) -> Result<()> {
            self.fields
                .write()
                .await
                .insert(Self::key(entity_type, entity_id, field), value);
            Ok(())
        }
    }

    fn increment_config(field: &str) -> CounterConfig {
        CounterConfig {
            field: field.to_string(),
            operation: CounterOperation::Increment,
        }
    }

    #[tokio::test]
    async fn test_counter_increment() {
        let store = Arc::new(MockEntityStore::new());
        store.set("capture", "cap-1", "like_count", 5.0).await;

        let sink = CounterSink::new(store.clone(), increment_config("like_count"));

        let payload = json!({
            "entity_type": "capture",
            "entity_id": "cap-1"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let value = store
            .read_field("capture", "cap-1", "like_count")
            .await
            .unwrap();
        assert_eq!(value, 6.0);
    }

    #[tokio::test]
    async fn test_counter_increment_from_zero() {
        let store = Arc::new(MockEntityStore::new());
        let sink = CounterSink::new(store.clone(), increment_config("like_count"));

        let payload = json!({
            "entity_type": "capture",
            "entity_id": "cap-1"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let value = store
            .read_field("capture", "cap-1", "like_count")
            .await
            .unwrap();
        assert_eq!(value, 1.0);
    }

    #[tokio::test]
    async fn test_counter_decrement() {
        let store = Arc::new(MockEntityStore::new());
        store.set("capture", "cap-1", "like_count", 5.0).await;

        let sink = CounterSink::new(
            store.clone(),
            CounterConfig {
                field: "like_count".to_string(),
                operation: CounterOperation::Decrement,
            },
        );

        let payload = json!({
            "entity_type": "capture",
            "entity_id": "cap-1"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let value = store
            .read_field("capture", "cap-1", "like_count")
            .await
            .unwrap();
        assert_eq!(value, 4.0);
    }

    #[tokio::test]
    async fn test_counter_decrement_floor_at_zero() {
        let store = Arc::new(MockEntityStore::new());
        store.set("capture", "cap-1", "like_count", 0.0).await;

        let sink = CounterSink::new(
            store.clone(),
            CounterConfig {
                field: "like_count".to_string(),
                operation: CounterOperation::Decrement,
            },
        );

        let payload = json!({
            "entity_type": "capture",
            "entity_id": "cap-1"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let value = store
            .read_field("capture", "cap-1", "like_count")
            .await
            .unwrap();
        assert_eq!(value, 0.0); // Never goes negative
    }

    #[tokio::test]
    async fn test_counter_set() {
        let store = Arc::new(MockEntityStore::new());
        store.set("capture", "cap-1", "like_count", 5.0).await;

        let sink = CounterSink::new(
            store.clone(),
            CounterConfig {
                field: "like_count".to_string(),
                operation: CounterOperation::Set,
            },
        );

        let payload = json!({
            "entity_type": "capture",
            "entity_id": "cap-1",
            "value": 42
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let value = store
            .read_field("capture", "cap-1", "like_count")
            .await
            .unwrap();
        assert_eq!(value, 42.0);
    }

    #[tokio::test]
    async fn test_counter_custom_amount() {
        let store = Arc::new(MockEntityStore::new());
        store.set("user", "u-1", "follower_count", 10.0).await;

        let sink = CounterSink::new(store.clone(), increment_config("follower_count"));

        let payload = json!({
            "entity_type": "user",
            "entity_id": "u-1",
            "value": 5
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let value = store
            .read_field("user", "u-1", "follower_count")
            .await
            .unwrap();
        assert_eq!(value, 15.0);
    }

    #[tokio::test]
    async fn test_counter_override_field_and_operation() {
        let store = Arc::new(MockEntityStore::new());
        store.set("user", "u-1", "comment_count", 3.0).await;

        // Config says increment like_count, but payload overrides both
        let sink = CounterSink::new(store.clone(), increment_config("like_count"));

        let payload = json!({
            "entity_type": "user",
            "entity_id": "u-1",
            "field": "comment_count",
            "operation": "decrement"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let value = store
            .read_field("user", "u-1", "comment_count")
            .await
            .unwrap();
        assert_eq!(value, 2.0);
    }

    #[tokio::test]
    async fn test_counter_entity_from_context() {
        let store = Arc::new(MockEntityStore::new());
        store.set("capture", "cap-1", "like_count", 0.0).await;

        let sink = CounterSink::new(store.clone(), increment_config("like_count"));

        let payload = json!({}); // No entity info in payload

        let mut vars = HashMap::new();
        vars.insert(
            "entity_type".to_string(),
            Value::String("capture".to_string()),
        );
        vars.insert("entity_id".to_string(), Value::String("cap-1".to_string()));

        sink.deliver(payload, None, &vars).await.unwrap();

        let value = store
            .read_field("capture", "cap-1", "like_count")
            .await
            .unwrap();
        assert_eq!(value, 1.0);
    }

    #[tokio::test]
    async fn test_counter_missing_entity_type_error() {
        let store = Arc::new(MockEntityStore::new());
        let sink = CounterSink::new(store, increment_config("like_count"));

        let payload = json!({"entity_id": "cap-1"});
        let result = sink.deliver(payload, None, &HashMap::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("entity_type"));
    }

    #[tokio::test]
    async fn test_counter_missing_entity_id_error() {
        let store = Arc::new(MockEntityStore::new());
        let sink = CounterSink::new(store, increment_config("like_count"));

        let payload = json!({"entity_type": "capture"});
        let result = sink.deliver(payload, None, &HashMap::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("entity_id"));
    }

    #[test]
    fn test_counter_operation_parse() {
        assert_eq!(
            CounterOperation::parse("increment").unwrap(),
            CounterOperation::Increment
        );
        assert_eq!(
            CounterOperation::parse("inc").unwrap(),
            CounterOperation::Increment
        );
        assert_eq!(
            CounterOperation::parse("decrement").unwrap(),
            CounterOperation::Decrement
        );
        assert_eq!(
            CounterOperation::parse("dec").unwrap(),
            CounterOperation::Decrement
        );
        assert_eq!(
            CounterOperation::parse("set").unwrap(),
            CounterOperation::Set
        );
        assert!(CounterOperation::parse("invalid").is_err());
    }

    #[test]
    fn test_counter_sink_name_and_type() {
        let store = Arc::new(MockEntityStore::new());
        let sink = CounterSink::new(store, increment_config("like_count"));
        assert_eq!(sink.name(), "counter");
        assert_eq!(sink.sink_type(), SinkType::Counter);
    }

    #[tokio::test]
    async fn test_counter_concurrent_increments() {
        let store = Arc::new(MockEntityStore::new());
        store.set("capture", "cap-1", "like_count", 0.0).await;

        let sink = Arc::new(CounterSink::new(
            store.clone(),
            increment_config("like_count"),
        ));

        // Spawn 50 concurrent increment tasks
        let mut handles = Vec::new();
        for _ in 0..50 {
            let sink = sink.clone();
            handles.push(tokio::spawn(async move {
                let payload = json!({
                    "entity_type": "capture",
                    "entity_id": "cap-1"
                });
                sink.deliver(payload, None, &HashMap::new()).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // Without per-key locks, this would be less than 50 due to TOCTOU
        let value = store
            .read_field("capture", "cap-1", "like_count")
            .await
            .unwrap();
        assert_eq!(
            value, 50.0,
            "All 50 increments should be applied atomically"
        );
    }
}
