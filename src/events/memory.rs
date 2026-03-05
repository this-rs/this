//! In-memory EventLog implementation
//!
//! Vec-backed event log suitable for development and single-instance deployments.
//! Events are stored in memory and lost on restart.

use crate::core::events::{EventEnvelope, FrameworkEvent};
use crate::events::log::EventLog;
use crate::events::types::{SeekPosition, SeqNo};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};
use tokio_stream::Stream;

/// In-memory implementation of the EventLog trait
///
/// Uses a Vec for storage and a Notify for waking subscribers
/// when new events are appended. Thread-safe via Arc<RwLock>.
///
/// # Performance
///
/// - Append: O(1) amortized
/// - Subscribe replay: O(n) from start position
/// - Ack/Seek: O(1)
///
/// # Limitations
///
/// - Events are lost on restart (no persistence)
/// - Memory grows unbounded (no retention policy yet)
/// - Single-instance only (no cross-process sharing)
#[derive(Debug, Clone)]
pub struct InMemoryEventLog {
    inner: Arc<InMemoryEventLogInner>,
}

#[derive(Debug)]
struct InMemoryEventLogInner {
    /// Ordered list of events (index = seq_no - 1)
    events: RwLock<Vec<EventEnvelope>>,
    /// Consumer positions: consumer_name -> last acked seq_no
    positions: RwLock<HashMap<String, SeqNo>>,
    /// Notification channel for new events
    notify: Notify,
}

impl InMemoryEventLog {
    /// Create a new empty in-memory event log
    pub fn new() -> Self {
        Self {
            inner: Arc::new(InMemoryEventLogInner {
                events: RwLock::new(Vec::new()),
                positions: RwLock::new(HashMap::new()),
                notify: Notify::new(),
            }),
        }
    }
}

impl Default for InMemoryEventLog {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventLog for InMemoryEventLog {
    async fn append(&self, event: FrameworkEvent) -> Result<SeqNo> {
        let envelope = EventEnvelope::new(event);
        let seq_no;
        {
            let mut events = self.inner.events.write().await;
            events.push(envelope);
            seq_no = events.len() as SeqNo;
        }
        // Wake all waiting subscribers
        self.inner.notify.notify_waiters();
        Ok(seq_no)
    }

    async fn subscribe(
        &self,
        consumer: &str,
        position: SeekPosition,
    ) -> Result<Pin<Box<dyn Stream<Item = EventEnvelope> + Send>>> {
        let start_seq = match position {
            SeekPosition::Beginning => 0,
            SeekPosition::Latest => {
                let events = self.inner.events.read().await;
                events.len() as SeqNo
            }
            SeekPosition::Sequence(seq) => seq.saturating_sub(1), // seq_no is 1-based, index is 0-based
            SeekPosition::LastAcknowledged => {
                let positions = self.inner.positions.read().await;
                positions.get(consumer).copied().unwrap_or(0)
            }
        };

        let inner = self.inner.clone();
        let stream = EventLogStream {
            inner,
            cursor: start_seq,
        };

        Ok(Box::pin(stream))
    }

    async fn ack(&self, consumer: &str, seq_no: SeqNo) -> Result<()> {
        let mut positions = self.inner.positions.write().await;
        positions.insert(consumer.to_string(), seq_no);
        Ok(())
    }

    async fn seek(&self, consumer: &str, position: SeekPosition) -> Result<()> {
        let seq_no = match position {
            SeekPosition::Beginning => 0,
            SeekPosition::Latest => {
                let events = self.inner.events.read().await;
                events.len() as SeqNo
            }
            SeekPosition::Sequence(seq) => seq,
            SeekPosition::LastAcknowledged => {
                // No-op: already at LastAcknowledged
                return Ok(());
            }
        };
        let mut positions = self.inner.positions.write().await;
        positions.insert(consumer.to_string(), seq_no);
        Ok(())
    }

    async fn last_seq_no(&self) -> Option<SeqNo> {
        let events = self.inner.events.read().await;
        if events.is_empty() {
            None
        } else {
            Some(events.len() as SeqNo)
        }
    }
}

/// Async stream over events in the log
///
/// Yields stored events first (replay), then waits for new events
/// using the Notify channel.
struct EventLogStream {
    inner: Arc<InMemoryEventLogInner>,
    cursor: SeqNo,
}

impl Stream for EventLogStream {
    type Item = EventEnvelope;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let cursor = self.cursor as usize;

        // Try to read the next event synchronously
        let maybe_event = {
            let events_guard = self.inner.events.try_read();
            match events_guard {
                Ok(events) => {
                    if cursor < events.len() {
                        Some(events[cursor].clone())
                    } else {
                        None
                    }
                }
                Err(_) => {
                    // Lock is held by writer, wake ourselves to retry
                    cx.waker().wake_by_ref();
                    return std::task::Poll::Pending;
                }
            }
        }; // Guard dropped here

        if let Some(event) = maybe_event {
            self.cursor += 1;
            return std::task::Poll::Ready(Some(event));
        }

        // No event available, register for notification
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        match notified.poll(cx) {
            std::task::Poll::Ready(()) => {
                // Woken up, re-check for events
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, LinkEvent};
    use serde_json::json;
    use tokio_stream::StreamExt;
    use uuid::Uuid;

    fn make_entity_event(entity_type: &str) -> FrameworkEvent {
        FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: entity_type.to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "test"}),
        })
    }

    fn make_link_event(link_type: &str) -> FrameworkEvent {
        FrameworkEvent::Link(LinkEvent::Created {
            link_type: link_type.to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        })
    }

    #[tokio::test]
    async fn test_append_returns_sequential_ids() {
        let log = InMemoryEventLog::new();

        let seq1 = log.append(make_entity_event("user")).await.unwrap();
        let seq2 = log.append(make_entity_event("order")).await.unwrap();
        let seq3 = log.append(make_link_event("follows")).await.unwrap();

        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(seq3, 3);
    }

    #[tokio::test]
    async fn test_last_seq_no_empty() {
        let log = InMemoryEventLog::new();
        assert_eq!(log.last_seq_no().await, None);
    }

    #[tokio::test]
    async fn test_last_seq_no_after_appends() {
        let log = InMemoryEventLog::new();
        log.append(make_entity_event("user")).await.unwrap();
        log.append(make_entity_event("order")).await.unwrap();
        assert_eq!(log.last_seq_no().await, Some(2));
    }

    #[tokio::test]
    async fn test_subscribe_from_beginning() {
        let log = InMemoryEventLog::new();

        // Append 5 events
        for i in 0..5 {
            log.append(make_entity_event(&format!("type_{i}")))
                .await
                .unwrap();
        }

        // Subscribe from beginning
        let stream = log
            .subscribe("test-consumer", SeekPosition::Beginning)
            .await
            .unwrap();

        // Take exactly 5 events (the stored ones)
        let events: Vec<_> = stream.take(5).collect().await;
        assert_eq!(events.len(), 5);

        // Verify order
        assert_eq!(events[0].event.entity_type(), Some("type_0"));
        assert_eq!(events[4].event.entity_type(), Some("type_4"));
    }

    #[tokio::test]
    async fn test_subscribe_from_latest_only_gets_new() {
        let log = InMemoryEventLog::new();

        // Append some events before subscribing
        log.append(make_entity_event("old_event")).await.unwrap();
        log.append(make_entity_event("old_event_2")).await.unwrap();

        // Subscribe from latest
        let mut stream = log
            .subscribe("test-consumer", SeekPosition::Latest)
            .await
            .unwrap();

        // Append a new event
        let log_clone = log.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            log_clone
                .append(make_entity_event("new_event"))
                .await
                .unwrap();
        });

        // Should receive only the new event
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.event.entity_type(), Some("new_event"));
    }

    #[tokio::test]
    async fn test_subscribe_from_sequence() {
        let log = InMemoryEventLog::new();

        // Append 5 events
        for i in 0..5 {
            log.append(make_entity_event(&format!("type_{i}")))
                .await
                .unwrap();
        }

        // Subscribe from sequence 3 (0-based internally, so we get events 3, 4, 5)
        let stream = log
            .subscribe("test-consumer", SeekPosition::Sequence(3))
            .await
            .unwrap();

        let events: Vec<_> = stream.take(3).collect().await;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event.entity_type(), Some("type_2")); // seq 3 = index 2
    }

    #[tokio::test]
    async fn test_ack_advances_position() {
        let log = InMemoryEventLog::new();

        // Append 5 events
        for i in 0..5 {
            log.append(make_entity_event(&format!("type_{i}")))
                .await
                .unwrap();
        }

        // Ack up to seq 3
        log.ack("consumer-a", 3).await.unwrap();

        // Subscribe from LastAcknowledged
        let stream = log
            .subscribe("consumer-a", SeekPosition::LastAcknowledged)
            .await
            .unwrap();

        let events: Vec<_> = stream.take(2).collect().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.entity_type(), Some("type_3")); // After ack(3), next is index 3 = type_3
    }

    #[tokio::test]
    async fn test_seek_repositions_consumer() {
        let log = InMemoryEventLog::new();

        // Append 5 events
        for i in 0..5 {
            log.append(make_entity_event(&format!("type_{i}")))
                .await
                .unwrap();
        }

        // Ack up to 5 (all events)
        log.ack("consumer-b", 5).await.unwrap();

        // Seek back to beginning
        log.seek("consumer-b", SeekPosition::Beginning)
            .await
            .unwrap();

        // Subscribe from LastAcknowledged should now give all events
        let stream = log
            .subscribe("consumer-b", SeekPosition::LastAcknowledged)
            .await
            .unwrap();

        let events: Vec<_> = stream.take(5).collect().await;
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].event.entity_type(), Some("type_0"));
    }

    #[tokio::test]
    async fn test_multiple_consumers_independent_positions() {
        let log = InMemoryEventLog::new();

        // Append 5 events
        for i in 0..5 {
            log.append(make_entity_event(&format!("type_{i}")))
                .await
                .unwrap();
        }

        // Consumer A acks up to 2
        log.ack("consumer-a", 2).await.unwrap();
        // Consumer B acks up to 4
        log.ack("consumer-b", 4).await.unwrap();

        // Consumer A from LastAcknowledged
        let stream_a = log
            .subscribe("consumer-a", SeekPosition::LastAcknowledged)
            .await
            .unwrap();
        let events_a: Vec<_> = stream_a.take(3).collect().await;
        assert_eq!(events_a.len(), 3); // Events 3, 4, 5 (indices 2, 3, 4)

        // Consumer B from LastAcknowledged
        let stream_b = log
            .subscribe("consumer-b", SeekPosition::LastAcknowledged)
            .await
            .unwrap();
        let events_b: Vec<_> = stream_b.take(1).collect().await;
        assert_eq!(events_b.len(), 1); // Only event 5 (index 4)
    }

    #[tokio::test]
    async fn test_live_subscription_receives_new_events() {
        let log = InMemoryEventLog::new();

        let mut stream = log
            .subscribe("live-consumer", SeekPosition::Latest)
            .await
            .unwrap();

        // Spawn a producer
        let log_clone = log.clone();
        tokio::spawn(async move {
            for i in 0..3 {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                log_clone
                    .append(make_entity_event(&format!("live_{i}")))
                    .await
                    .unwrap();
            }
        });

        // Consume 3 live events
        for i in 0..3 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                event.event.entity_type(),
                Some(format!("live_{i}").as_str())
            );
        }
    }

    #[tokio::test]
    async fn test_replay_then_live() {
        let log = InMemoryEventLog::new();

        // Pre-populate with 3 events
        for i in 0..3 {
            log.append(make_entity_event(&format!("old_{i}")))
                .await
                .unwrap();
        }

        // Subscribe from beginning (will replay first, then go live)
        let mut stream = log
            .subscribe("replay-consumer", SeekPosition::Beginning)
            .await
            .unwrap();

        // Read the 3 replayed events
        for i in 0..3 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                event.event.entity_type(),
                Some(format!("old_{i}").as_str())
            );
        }

        // Now append a live event
        let log_clone = log.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            log_clone
                .append(make_entity_event("live_new"))
                .await
                .unwrap();
        });

        // Should receive the live event
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.event.entity_type(), Some("live_new"));
    }

    #[tokio::test]
    async fn test_unacked_consumer_starts_from_zero() {
        let log = InMemoryEventLog::new();

        // Append events
        log.append(make_entity_event("first")).await.unwrap();
        log.append(make_entity_event("second")).await.unwrap();

        // New consumer (never acked) subscribing from LastAcknowledged
        let stream = log
            .subscribe("new-consumer", SeekPosition::LastAcknowledged)
            .await
            .unwrap();

        let events: Vec<_> = stream.take(2).collect().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.entity_type(), Some("first"));
    }

    #[tokio::test]
    async fn test_clone_shares_state() {
        let log1 = InMemoryEventLog::new();
        let log2 = log1.clone();

        log1.append(make_entity_event("from_log1")).await.unwrap();
        log2.append(make_entity_event("from_log2")).await.unwrap();

        assert_eq!(log1.last_seq_no().await, Some(2));
        assert_eq!(log2.last_seq_no().await, Some(2));
    }
}
