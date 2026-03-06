//! EventLog trait — persistent, ordered, replayable event storage
//!
//! The EventLog is the source of truth for the event flow system.
//! Unlike the EventBus (fire-and-forget broadcast), the EventLog persists
//! events and supports replay from any position.

use crate::core::events::EventEnvelope;
use crate::events::types::{SeekPosition, SeqNo};
use anyhow::Result;
use async_trait::async_trait;
use std::pin::Pin;
use tokio_stream::Stream;

/// Trait for persistent event storage backends
///
/// Implementations must be Send + Sync to allow sharing across tasks.
///
/// # Backends
///
/// - `InMemoryEventLog` — Vec-backed, suitable for dev/single-instance
/// - Future: NATS JetStream, Kafka, Redis Streams
///
/// # Consumer Groups
///
/// Each consumer has an independent position in the log. The `ack` method
/// advances the consumer's position, and `seek` allows repositioning.
/// Consumer groups enable:
/// - **Replay**: Start from `Beginning` to reprocess all events
/// - **Resume**: Use `LastAcknowledged` to pick up where you left off
/// - **Live**: Use `Latest` to only see new events
#[async_trait]
pub trait EventLog: Send + Sync {
    /// Append an event envelope to the log
    ///
    /// Returns the sequence number assigned to the event.
    /// Events are assigned monotonically increasing sequence numbers.
    /// The envelope's `seq_no` field is set by the implementation.
    async fn append(&self, envelope: EventEnvelope) -> Result<SeqNo>;

    /// Subscribe to events from a given position
    ///
    /// Returns a stream of `EventEnvelope` starting from the specified position.
    /// The stream is infinite — it will yield stored events first, then wait
    /// for new events as they are appended.
    ///
    /// # Arguments
    ///
    /// * `consumer` - Consumer group name (for tracking position)
    /// * `position` - Where to start reading from
    async fn subscribe(
        &self,
        consumer: &str,
        position: SeekPosition,
    ) -> Result<Pin<Box<dyn Stream<Item = EventEnvelope> + Send>>>;

    /// Acknowledge that a consumer has processed up to a sequence number
    ///
    /// This advances the consumer's `LastAcknowledged` position.
    async fn ack(&self, consumer: &str, seq_no: SeqNo) -> Result<()>;

    /// Seek a consumer to a new position
    ///
    /// This changes the consumer's position without acknowledging.
    /// The next `subscribe` with `LastAcknowledged` will use this position.
    async fn seek(&self, consumer: &str, position: SeekPosition) -> Result<()>;

    /// Get the current last sequence number in the log
    ///
    /// Returns `None` if the log is empty.
    async fn last_seq_no(&self) -> Option<SeqNo>;
}
