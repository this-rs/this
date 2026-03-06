//! Rate limit operator — throttles event throughput via token bucket
//!
//! Uses a simple token bucket algorithm: tokens are consumed for each event,
//! and refilled at a constant rate. When tokens are exhausted, events are
//! either dropped or queued (based on strategy).
//!
//! ```yaml
//! - rate_limit:
//!     max: 100
//!     per: 1s
//!     strategy: drop
//! ```

use crate::config::events::RateLimitConfig;
use crate::events::context::FlowContext;
use crate::events::operators::deduplicate::parse_duration;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Token bucket state
#[derive(Debug)]
struct TokenBucket {
    /// Current available tokens
    tokens: f64,
    /// Maximum tokens (bucket capacity)
    max_tokens: f64,
    /// Refill rate (tokens per second)
    refill_rate: f64,
    /// Last refill timestamp
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_tokens: u32, period: Duration) -> Self {
        let max = max_tokens as f64;
        let refill_rate = max / period.as_secs_f64();
        Self {
            tokens: max,
            max_tokens: max,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume one token. Returns true if allowed.
    fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill);
        let new_tokens = elapsed.as_secs_f64() * self.refill_rate;
        self.tokens = (self.tokens + new_tokens).min(self.max_tokens);
        self.last_refill = now;
    }
}

/// Compiled rate limit operator
#[derive(Debug)]
pub struct RateLimitOp {
    /// Strategy when limit exceeded: "drop" or "queue"
    strategy: String,

    /// Token bucket
    bucket: Arc<Mutex<TokenBucket>>,
}

impl RateLimitOp {
    /// Create a RateLimitOp from a RateLimitConfig
    pub fn from_config(config: &RateLimitConfig) -> Result<Self> {
        let period = parse_duration(&config.per)?;
        Ok(Self {
            strategy: config.strategy.clone(),
            bucket: Arc::new(Mutex::new(TokenBucket::new(config.max, period))),
        })
    }

    /// Create with specific parameters (for testing)
    #[cfg(test)]
    fn with_params(max: u32, period: Duration) -> Self {
        Self {
            strategy: "drop".to_string(),
            bucket: Arc::new(Mutex::new(TokenBucket::new(max, period))),
        }
    }
}

#[async_trait]
impl PipelineOperator for RateLimitOp {
    async fn execute(&self, _ctx: &mut FlowContext) -> Result<OpResult> {
        let mut bucket = self.bucket.lock().await;
        if bucket.try_consume() {
            Ok(OpResult::Continue)
        } else {
            match self.strategy.as_str() {
                "queue" => {
                    // TODO: implement queuing (requires background drain)
                    // For now, treat as drop with a trace
                    tracing::debug!("rate_limit: event queued (falling back to drop)");
                    Ok(OpResult::Drop)
                }
                _ => {
                    // "drop" strategy (default)
                    Ok(OpResult::Drop)
                }
            }
        }
    }

    fn name(&self) -> &str {
        "rate_limit"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, FrameworkEvent};
    use crate::core::service::LinkService;
    use serde_json::json;
    use std::collections::HashMap;
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

    fn make_context() -> FlowContext {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });
        FlowContext::new(
            event,
            Arc::new(MockLinkService) as Arc<dyn LinkService>,
            HashMap::new(),
        )
    }

    #[tokio::test]
    async fn test_rate_limit_allows_within_limit() {
        let op = RateLimitOp::with_params(3, Duration::from_secs(1));

        for _ in 0..3 {
            let mut ctx = make_context();
            let result = op.execute(&mut ctx).await.unwrap();
            assert!(matches!(result, OpResult::Continue));
        }
    }

    #[tokio::test]
    async fn test_rate_limit_drops_over_limit() {
        let op = RateLimitOp::with_params(2, Duration::from_secs(1));

        // First 2 pass
        for _ in 0..2 {
            let mut ctx = make_context();
            let result = op.execute(&mut ctx).await.unwrap();
            assert!(matches!(result, OpResult::Continue));
        }

        // 3rd is dropped
        let mut ctx = make_context();
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    #[tokio::test]
    async fn test_rate_limit_refills_after_period() {
        let op = RateLimitOp::with_params(2, Duration::from_millis(50));

        // Consume all tokens
        for _ in 0..2 {
            let mut ctx = make_context();
            let _ = op.execute(&mut ctx).await.unwrap();
        }

        // Should be dropped
        let mut ctx = make_context();
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));

        // Wait for refill
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Should pass again
        let mut ctx = make_context();
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_rate_limit_partial_refill() {
        // 2 tokens per 100ms = 20 tokens/sec
        let op = RateLimitOp::with_params(2, Duration::from_millis(100));

        // Consume all tokens
        for _ in 0..2 {
            let mut ctx = make_context();
            let _ = op.execute(&mut ctx).await.unwrap();
        }

        // Wait for half the period — should get ~1 token back
        tokio::time::sleep(Duration::from_millis(55)).await;

        // Should pass (1 token refilled)
        let mut ctx = make_context();
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        // But second should drop (only ~1 token was refilled)
        let mut ctx = make_context();
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }
}
