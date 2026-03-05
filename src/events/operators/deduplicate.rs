//! Deduplicate operator — eliminates duplicate events within a sliding window
//!
//! Uses a bounded HashSet with time-based expiration. The deduplication key
//! is read from a context variable (e.g., `source_id`).
//!
//! ```yaml
//! - deduplicate:
//!     key: source_id
//!     window: 1h
//! ```

use crate::config::events::DeduplicateConfig;
use crate::events::context::FlowContext;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Compiled deduplicate operator
#[derive(Debug)]
pub struct DeduplicateOp {
    /// Field name in context to use as the dedup key
    key: String,

    /// Sliding window duration
    window: Duration,

    /// Set of seen keys with their insertion timestamp
    seen: Arc<RwLock<HashMap<String, Instant>>>,
}

impl DeduplicateOp {
    /// Create a DeduplicateOp from a DeduplicateConfig
    pub fn from_config(config: &DeduplicateConfig) -> Result<Self> {
        let window = parse_duration(&config.window)?;
        Ok(Self {
            key: config.key.clone(),
            window,
            seen: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create with a specific window (for testing)
    #[cfg(test)]
    fn with_window(key: &str, window: Duration) -> Self {
        Self {
            key: key.to_string(),
            window,
            seen: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl PipelineOperator for DeduplicateOp {
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult> {
        // Read the dedup key from context
        let key_value = ctx
            .get_var(&self.key)
            .ok_or_else(|| {
                anyhow!(
                    "deduplicate: variable '{}' not found in context",
                    self.key
                )
            })?
            .clone();

        let key_str = value_to_string(&key_value);
        let now = Instant::now();

        let mut seen = self.seen.write().await;

        // Clean expired entries
        seen.retain(|_, ts| now.duration_since(*ts) < self.window);

        // Check if already seen
        if seen.contains_key(&key_str) {
            return Ok(OpResult::Drop);
        }

        // Mark as seen
        seen.insert(key_str, now);
        Ok(OpResult::Continue)
    }

    fn name(&self) -> &str {
        "deduplicate"
    }
}

/// Convert a JSON value to a string key for deduplication
fn value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// Parse a duration string like "5m", "1h", "30s", "100ms"
pub(crate) fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();

    if let Some(ms) = s.strip_suffix("ms") {
        let n: u64 = ms
            .parse()
            .map_err(|_| anyhow!("invalid duration: '{}'", s))?;
        return Ok(Duration::from_millis(n));
    }

    if let Some(secs) = s.strip_suffix('s') {
        let n: u64 = secs
            .parse()
            .map_err(|_| anyhow!("invalid duration: '{}'", s))?;
        return Ok(Duration::from_secs(n));
    }

    if let Some(mins) = s.strip_suffix('m') {
        let n: u64 = mins
            .parse()
            .map_err(|_| anyhow!("invalid duration: '{}'", s))?;
        return Ok(Duration::from_secs(n * 60));
    }

    if let Some(hours) = s.strip_suffix('h') {
        let n: u64 = hours
            .parse()
            .map_err(|_| anyhow!("invalid duration: '{}'", s))?;
        return Ok(Duration::from_secs(n * 3600));
    }

    Err(anyhow!(
        "invalid duration '{}': expected format like '5m', '1h', '30s', '100ms'",
        s
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, FrameworkEvent};
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
            _link: crate::core::link::LinkEntity,
        ) -> Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn get(&self, _id: &Uuid) -> Result<Option<crate::core::link::LinkEntity>> {
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

    fn make_context(entity_id: Uuid) -> FlowContext {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id,
            data: json!({}),
        });
        FlowContext::new(
            event,
            Arc::new(MockLinkService) as Arc<dyn LinkService>,
            StdHashMap::new(),
        )
    }

    #[tokio::test]
    async fn test_dedup_first_event_passes() {
        let op = DeduplicateOp::with_window("entity_id", Duration::from_secs(60));
        let mut ctx = make_context(Uuid::new_v4());

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_dedup_same_key_in_window_drops() {
        let entity_id = Uuid::new_v4();
        let op = DeduplicateOp::with_window("entity_id", Duration::from_secs(60));

        let mut ctx1 = make_context(entity_id);
        let result1 = op.execute(&mut ctx1).await.unwrap();
        assert!(matches!(result1, OpResult::Continue));

        let mut ctx2 = make_context(entity_id);
        let result2 = op.execute(&mut ctx2).await.unwrap();
        assert!(matches!(result2, OpResult::Drop));
    }

    #[tokio::test]
    async fn test_dedup_different_key_passes() {
        let op = DeduplicateOp::with_window("entity_id", Duration::from_secs(60));

        let mut ctx1 = make_context(Uuid::new_v4());
        let result1 = op.execute(&mut ctx1).await.unwrap();
        assert!(matches!(result1, OpResult::Continue));

        let mut ctx2 = make_context(Uuid::new_v4());
        let result2 = op.execute(&mut ctx2).await.unwrap();
        assert!(matches!(result2, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_dedup_expired_window_passes_again() {
        let entity_id = Uuid::new_v4();
        // Use a very short window
        let op = DeduplicateOp::with_window("entity_id", Duration::from_millis(50));

        let mut ctx1 = make_context(entity_id);
        let result1 = op.execute(&mut ctx1).await.unwrap();
        assert!(matches!(result1, OpResult::Continue));

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(60)).await;

        let mut ctx2 = make_context(entity_id);
        let result2 = op.execute(&mut ctx2).await.unwrap();
        assert!(matches!(result2, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_dedup_missing_key_errors() {
        let op = DeduplicateOp::with_window("nonexistent", Duration::from_secs(60));
        let mut ctx = make_context(Uuid::new_v4());

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
    }

    // ── Duration parsing tests ───────────────────────────────────────

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_duration_milliseconds() {
        assert_eq!(parse_duration("100ms").unwrap(), Duration::from_millis(100));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("abc").is_err());
    }
}
