//! Batch operator — accumulates events by key within a time window
//!
//! Groups events by a key field (e.g., `target_id`) and holds them for
//! a configurable window duration. When the window expires, emits a single
//! batched event with a count and the list of accumulated source IDs.
//!
//! ```yaml
//! - batch:
//!     key: target_id
//!     window: 5m
//!     min_count: 1
//! ```
//!
//! The batch operator stores a `_batch` variable in the context:
//! ```json
//! {
//!   "count": 3,
//!   "key": "target_id_value",
//!   "items": ["source_1", "source_2", "source_3"]
//! }
//! ```

use crate::config::events::BatchConfig;
use crate::events::context::FlowContext;
use crate::events::operators::deduplicate::parse_duration;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// A bucket of accumulated events for a single key
#[derive(Debug, Clone)]
struct BatchBucket {
    /// Items accumulated (stored as string representations)
    items: Vec<String>,
    /// When this bucket was created
    started_at: Instant,
    /// The last FlowContext (used as template for the emitted batch event)
    last_ctx: Option<FlowContext>,
}

/// Compiled batch operator
#[derive(Debug)]
pub struct BatchOp {
    /// Field to group events by
    key: String,

    /// Time window duration
    window: Duration,

    /// Minimum number of events before emitting
    min_count: u32,

    /// Accumulated buckets: key_value -> BatchBucket
    buckets: Arc<RwLock<HashMap<String, BatchBucket>>>,
}

impl BatchOp {
    /// Create a BatchOp from a BatchConfig
    pub fn from_config(config: &BatchConfig) -> Result<Self> {
        let window = parse_duration(&config.window)?;
        Ok(Self {
            key: config.key.clone(),
            window,
            min_count: config.min_count,
            buckets: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create with specific parameters (for testing)
    #[cfg(test)]
    fn with_params(key: &str, window: Duration, min_count: u32) -> Self {
        Self {
            key: key.to_string(),
            window,
            min_count,
            buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl PipelineOperator for BatchOp {
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult> {
        // Read the grouping key from context
        let key_value = ctx
            .get_var(&self.key)
            .ok_or_else(|| anyhow!("batch: variable '{}' not found in context", self.key))?
            .clone();

        let key_str = value_to_string(&key_value);

        // Read a secondary value to store in items (e.g., source_id for "who did it")
        let item_value = ctx
            .get_var("source_id")
            .or_else(|| ctx.get_var("entity_id"))
            .map(|v| value_to_string(v))
            .unwrap_or_default();

        let now = Instant::now();
        let mut buckets = self.buckets.write().await;

        // Check if we have an existing bucket for this key
        let bucket = buckets.entry(key_str.clone()).or_insert_with(|| BatchBucket {
            items: Vec::new(),
            started_at: now,
            last_ctx: None,
        });

        // Check if the window has expired
        let window_expired = now.duration_since(bucket.started_at) >= self.window;

        if window_expired && bucket.items.len() as u32 >= self.min_count {
            // Window expired with enough items — flush the batch
            let count = bucket.items.len();
            let items = bucket.items.clone();

            // Reset the bucket with the current event
            bucket.items = vec![item_value];
            bucket.started_at = now;
            bucket.last_ctx = Some(ctx.clone());

            // Set batch variables in context
            ctx.set_var(
                "_batch",
                json!({
                    "count": count,
                    "key": key_str,
                    "items": items,
                }),
            );

            Ok(OpResult::Continue)
        } else if window_expired {
            // Window expired but not enough items — reset and accumulate
            bucket.items = vec![item_value];
            bucket.started_at = now;
            bucket.last_ctx = Some(ctx.clone());
            Ok(OpResult::Drop)
        } else {
            // Window still active — accumulate
            bucket.items.push(item_value);
            bucket.last_ctx = Some(ctx.clone());
            Ok(OpResult::Drop)
        }
    }

    fn name(&self) -> &str {
        "batch"
    }
}

/// Convert a JSON value to a string key
fn value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{FrameworkEvent, LinkEvent};
    use crate::core::service::LinkService;
    use serde_json::json;
    use std::collections::HashMap as StdHashMap;
    use std::sync::Arc;
    use uuid::Uuid;

    struct MockLinkService;

    #[async_trait]
    impl LinkService for MockLinkService {
        async fn create(
            &self,
            _: crate::core::link::LinkEntity,
        ) -> Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn get(&self, _: &Uuid) -> Result<Option<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn list(&self) -> Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn find_by_source(
            &self,
            _: &Uuid,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn find_by_target(
            &self,
            _: &Uuid,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn update(
            &self,
            _: &Uuid,
            _: crate::core::link::LinkEntity,
        ) -> Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn delete(&self, _: &Uuid) -> Result<()> {
            unimplemented!()
        }
        async fn delete_by_entity(&self, _: &Uuid) -> Result<()> {
            unimplemented!()
        }
    }

    fn make_link_context(source_id: Uuid, target_id: Uuid) -> FlowContext {
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "likes".to_string(),
            link_id: Uuid::new_v4(),
            source_id,
            target_id,
            metadata: None,
        });
        FlowContext::new(
            event,
            Arc::new(MockLinkService) as Arc<dyn LinkService>,
            StdHashMap::new(),
        )
    }

    #[tokio::test]
    async fn test_batch_accumulates_within_window() {
        let target_id = Uuid::new_v4();
        let op = BatchOp::with_params("target_id", Duration::from_secs(60), 1);

        // First event — accumulates (window just started)
        let mut ctx1 = make_link_context(Uuid::new_v4(), target_id);
        let result1 = op.execute(&mut ctx1).await.unwrap();
        assert!(matches!(result1, OpResult::Drop));

        // Second event — still accumulating
        let mut ctx2 = make_link_context(Uuid::new_v4(), target_id);
        let result2 = op.execute(&mut ctx2).await.unwrap();
        assert!(matches!(result2, OpResult::Drop));
    }

    #[tokio::test]
    async fn test_batch_flushes_after_window() {
        let target_id = Uuid::new_v4();
        let op = BatchOp::with_params("target_id", Duration::from_millis(50), 1);

        // Accumulate 3 events
        for _ in 0..3 {
            let mut ctx = make_link_context(Uuid::new_v4(), target_id);
            let _ = op.execute(&mut ctx).await.unwrap();
        }

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Next event should trigger flush
        let mut ctx = make_link_context(Uuid::new_v4(), target_id);
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        // Check the _batch variable
        let batch = ctx.get_var("_batch").unwrap();
        assert_eq!(batch["count"], 3);
        assert_eq!(batch["key"], target_id.to_string());
        assert_eq!(batch["items"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_batch_min_count_not_met() {
        let target_id = Uuid::new_v4();
        // Require min_count of 5, but only send 2
        let op = BatchOp::with_params("target_id", Duration::from_millis(50), 5);

        // Accumulate 2 events
        for _ in 0..2 {
            let mut ctx = make_link_context(Uuid::new_v4(), target_id);
            let _ = op.execute(&mut ctx).await.unwrap();
        }

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Next event — window expired but min_count not met → drop and reset
        let mut ctx = make_link_context(Uuid::new_v4(), target_id);
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    #[tokio::test]
    async fn test_batch_different_keys_independent() {
        let target_a = Uuid::new_v4();
        let target_b = Uuid::new_v4();
        let op = BatchOp::with_params("target_id", Duration::from_millis(50), 1);

        // Accumulate for key A
        let mut ctx_a = make_link_context(Uuid::new_v4(), target_a);
        let _ = op.execute(&mut ctx_a).await.unwrap();

        // Accumulate for key B
        let mut ctx_b = make_link_context(Uuid::new_v4(), target_b);
        let _ = op.execute(&mut ctx_b).await.unwrap();

        // Wait for window
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Flush key A
        let mut ctx_a2 = make_link_context(Uuid::new_v4(), target_a);
        let result_a = op.execute(&mut ctx_a2).await.unwrap();
        assert!(matches!(result_a, OpResult::Continue));
        assert_eq!(ctx_a2.get_var("_batch").unwrap()["count"], 1);

        // Flush key B
        let mut ctx_b2 = make_link_context(Uuid::new_v4(), target_b);
        let result_b = op.execute(&mut ctx_b2).await.unwrap();
        assert!(matches!(result_b, OpResult::Continue));
        assert_eq!(ctx_b2.get_var("_batch").unwrap()["count"], 1);
    }

    #[tokio::test]
    async fn test_batch_missing_key_errors() {
        let op = BatchOp::with_params("nonexistent", Duration::from_secs(60), 1);
        let mut ctx = make_link_context(Uuid::new_v4(), Uuid::new_v4());

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
    }
}
