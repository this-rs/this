//! Dead letters: where a delivery that exhausted its retries is kept.
//!
//! # The contract this exists for
//!
//! A consumer has two bad options and one good one:
//!
//! - acknowledging a failed delivery loses it silently (the original defect);
//! - never acknowledging keeps the cursor behind forever, and one poison event
//!   stalls the flow for good;
//! - **or** the failure is written down somewhere durable, and only then the
//!   event is acknowledged. That is a dead letter.
//!
//! So the runtime's rule is: a delivery is acknowledged when it succeeded, **or**
//! when its failure is recorded here. If recording fails, the event is not
//! acknowledged — an event that is neither delivered nor written down must stay
//! replayable.
//!
//! # Why a separate trait and not `EventLog`
//!
//! `EventLog` is the ordered source of truth; a dead-letter store is a different
//! concern with different access patterns (bounded, queryable, drained on
//! replay). Keeping them apart also means no implementor of `EventLog` —
//! `InMemoryEventLog`, `log_obrain` behind the feature — has to grow new methods.

use crate::core::events::FrameworkEvent;
use crate::events::types::SeqNo;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

/// One delivery that could not be made, with why and how many times it was tried.
///
/// Everything needed to understand the failure without re-running it: which
/// flow, which event, which attempt count, the last error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetter {
    /// The consumer whose cursor this letter keeps from advancing.
    pub consumer: String,
    /// The flow whose pipeline failed.
    pub flow: String,
    /// Position of the event in the log, if it had one.
    pub seq_no: Option<SeqNo>,
    /// The event that could not be delivered — kept whole, so a replay does not
    /// need to find it in the log.
    pub event: FrameworkEvent,
    /// How many times the pipeline was executed (1 = no retry was configured).
    pub attempts: u32,
    /// The last error rendered, chain included.
    pub last_error: String,
    /// When the letter was written.
    pub at: DateTime<Utc>,
}

/// Where exhausted deliveries go.
#[async_trait]
pub trait DeadLetterSink: Send + Sync + std::fmt::Debug {
    /// Write one letter. An error here is meaningful: the caller must not
    /// acknowledge the event if this fails.
    async fn record(&self, letter: DeadLetter) -> Result<()>;

    /// Every letter currently held, oldest first.
    async fn list(&self) -> Result<Vec<DeadLetter>>;

    /// Remove and return the letters of `flow`, for a replay.
    ///
    /// Taking them is deliberate: a replay that leaves the originals in place
    /// would re-deliver them on the next read.
    async fn take(&self, flow: &str) -> Result<Vec<DeadLetter>>;

    /// How many letters are held. Used by health surfaces and tests.
    async fn len(&self) -> Result<usize>;

    /// Whether the store is empty.
    async fn is_empty(&self) -> Result<bool> {
        Ok(self.len().await? == 0)
    }
}

/// A bounded, in-process dead-letter store.
///
/// Bounded on purpose: an unbounded queue in a long-running process is a slow
/// leak, and the oldest letters are the least actionable. When it is full the
/// oldest letter is dropped and a warning is logged — dropping the *newest*
/// would hide the failure that is happening now.
#[derive(Debug)]
pub struct InMemoryDeadLetters {
    capacity: usize,
    inner: Mutex<VecDeque<DeadLetter>>,
}

/// Default bound: enough to hold a burst, small enough to stay in memory.
pub const DEFAULT_DEAD_LETTER_CAPACITY: usize = 1000;

impl Default for InMemoryDeadLetters {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryDeadLetters {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_DEAD_LETTER_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: Mutex::new(VecDeque::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<DeadLetter>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[async_trait]
impl DeadLetterSink for InMemoryDeadLetters {
    async fn record(&self, letter: DeadLetter) -> Result<()> {
        let mut queue = self.lock();
        if queue.len() == self.capacity {
            let dropped = queue.pop_front();
            tracing::warn!(
                dropped_flow = dropped.as_ref().map(|d| d.flow.as_str()).unwrap_or("?"),
                capacity = self.capacity,
                "dead-letter store full: the oldest letter was dropped"
            );
        }
        queue.push_back(letter);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<DeadLetter>> {
        Ok(self.lock().iter().cloned().collect())
    }

    async fn take(&self, flow: &str) -> Result<Vec<DeadLetter>> {
        let mut queue = self.lock();
        let taken: Vec<DeadLetter> = queue.iter().filter(|l| l.flow == flow).cloned().collect();
        queue.retain(|l| l.flow != flow);
        Ok(taken)
    }

    async fn len(&self) -> Result<usize> {
        Ok(self.lock().len())
    }
}

/// How many times, and how far apart, a failing pipeline is retried.
///
/// Defaults are deliberately modest: retrying a deterministic failure (a
/// malformed template, a missing entity) gains nothing, and the dead letter is
/// what makes the failure visible. The retries exist for transient causes — a
/// database blip, a sink briefly unreachable.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts, the first one included. `1` disables retrying.
    pub max_attempts: u32,
    /// Delay before the second attempt.
    pub initial_backoff: Duration,
    /// Ceiling for the exponential growth.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// No retry: one attempt, then the dead letter.
    pub fn once() -> Self {
        Self {
            max_attempts: 1,
            ..Self::default()
        }
    }

    /// Delay before attempt number `attempt` (1-based: attempt 2 waits
    /// `initial_backoff`, attempt 3 waits twice that, and so on).
    ///
    /// The shift is clamped before it is applied: `2u32.pow(40)` would overflow,
    /// and a policy with many attempts must not panic while handling a failure.
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(16);
        self.initial_backoff
            .saturating_mul(1u32 << doublings)
            .min(self.max_backoff)
    }

    /// A policy with zero attempts would deliver nothing and record nothing.
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.max_attempts >= 1,
            "retry policy: max_attempts must be at least 1 (got {})",
            self.max_attempts
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, FrameworkEvent};
    use uuid::Uuid;

    fn letter(flow: &str) -> DeadLetter {
        DeadLetter {
            consumer: format!("flow-runtime:{flow}"),
            flow: flow.to_string(),
            seq_no: Some(1),
            event: FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: "order".to_string(),
                entity_id: Uuid::new_v4(),
                data: serde_json::json!({}),
            }),
            attempts: 3,
            last_error: "boom".to_string(),
            at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn letters_are_kept_oldest_first() {
        let store = InMemoryDeadLetters::new();
        store.record(letter("a")).await.unwrap();
        store.record(letter("b")).await.unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].flow, "a");
        assert_eq!(all[1].flow, "b");
    }

    #[tokio::test]
    async fn take_removes_only_that_flow() {
        let store = InMemoryDeadLetters::new();
        store.record(letter("a")).await.unwrap();
        store.record(letter("b")).await.unwrap();
        store.record(letter("a")).await.unwrap();

        let taken = store.take("a").await.unwrap();
        assert_eq!(taken.len(), 2, "both letters of flow `a`");
        assert!(taken.iter().all(|l| l.flow == "a"));

        let left = store.list().await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].flow, "b", "another flow's letter is untouched");
    }

    #[tokio::test]
    async fn the_store_is_bounded_and_drops_the_oldest() {
        let store = InMemoryDeadLetters::with_capacity(2);
        store.record(letter("first")).await.unwrap();
        store.record(letter("second")).await.unwrap();
        store.record(letter("third")).await.unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 2, "the bound holds");
        assert_eq!(
            all[0].flow, "second",
            "the oldest went, not the newest: the newest is the actionable one"
        );
        assert_eq!(all[1].flow, "third");
    }

    #[tokio::test]
    async fn a_letter_survives_a_json_round_trip() {
        let store = InMemoryDeadLetters::new();
        store.record(letter("a")).await.unwrap();

        let all = store.list().await.unwrap();
        let json = serde_json::to_string(&all[0]).unwrap();
        let back: DeadLetter = serde_json::from_str(&json).unwrap();

        assert_eq!(back.flow, "a");
        assert_eq!(back.attempts, 3);
        assert_eq!(back.last_error, "boom");
        assert_eq!(back.seq_no, Some(1));
    }

    #[test]
    fn backoff_grows_then_stops_at_the_ceiling() {
        let policy = RetryPolicy {
            max_attempts: 8,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(300),
        };

        assert_eq!(policy.backoff_for(1), Duration::from_millis(50));
        assert_eq!(policy.backoff_for(2), Duration::from_millis(100));
        assert_eq!(policy.backoff_for(3), Duration::from_millis(200));
        assert_eq!(policy.backoff_for(4), Duration::from_millis(300), "capped");
        assert_eq!(
            policy.backoff_for(60),
            Duration::from_millis(300),
            "no overflow"
        );
    }

    #[test]
    fn a_policy_with_zero_attempts_is_refused() {
        let bad = RetryPolicy {
            max_attempts: 0,
            ..RetryPolicy::default()
        };
        assert!(bad.validate().is_err());
        assert!(RetryPolicy::default().validate().is_ok());
        assert!(RetryPolicy::once().validate().is_ok());
    }
}
