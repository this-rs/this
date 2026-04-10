//! Deduplication filter to prevent event loops between inbound and outbound bridges
//!
//! When the inbound bridge translates a `MutationEvent` into a `FrameworkEvent`
//! and publishes it on the `EventBus`, the outbound bridge would pick it up and
//! try to translate it back — creating an infinite loop.
//!
//! The `DedupFilter` breaks this cycle by maintaining a TTL-based set of event IDs
//! that were recently injected by the inbound bridge. The outbound bridge checks
//! this set and skips any event that originated from inbound translation.

use crate::core::events::EventEnvelope;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// TTL-based deduplication filter for the event bridge
pub struct DedupFilter {
    /// Map of event_id → when it was marked (for TTL expiry)
    seen: Mutex<HashMap<Uuid, Instant>>,
    /// How long to remember events before expiring them
    ttl: Duration,
}

impl DedupFilter {
    /// Create a new dedup filter with the given TTL
    ///
    /// Events older than `ttl` are automatically cleaned up.
    /// A reasonable default is 5-10 seconds.
    pub fn new(ttl: Duration) -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Mark an event as seen (called by the inbound bridge when it publishes to EventBus)
    pub fn mark(&self, event_id: Uuid) {
        let mut seen = self.seen.lock().expect("DedupFilter lock poisoned");
        seen.insert(event_id, Instant::now());
    }

    /// Check if an event was recently processed by the inbound bridge
    ///
    /// Returns `true` if the event should be skipped by the outbound bridge.
    pub fn is_duplicate(&self, envelope: &EventEnvelope) -> bool {
        let seen = self.seen.lock().expect("DedupFilter lock poisoned");
        if let Some(timestamp) = seen.get(&envelope.id) {
            timestamp.elapsed() < self.ttl
        } else {
            false
        }
    }

    /// Remove expired entries from the filter
    ///
    /// Should be called periodically (e.g. every 30s) to prevent unbounded growth.
    pub fn cleanup(&self) {
        let mut seen = self.seen.lock().expect("DedupFilter lock poisoned");
        seen.retain(|_, timestamp| timestamp.elapsed() < self.ttl);
    }

    /// Return the number of entries currently tracked (mainly for testing/metrics)
    pub fn len(&self) -> usize {
        self.seen.lock().expect("DedupFilter lock poisoned").len()
    }

    /// Return whether the filter is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, EventEnvelope, FrameworkEvent};
    use serde_json::json;
    use std::time::Duration;

    fn make_envelope() -> EventEnvelope {
        EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }))
    }

    #[test]
    fn test_unmarked_event_is_not_duplicate() {
        let filter = DedupFilter::new(Duration::from_secs(5));
        let envelope = make_envelope();
        assert!(!filter.is_duplicate(&envelope));
    }

    #[test]
    fn test_marked_event_is_duplicate() {
        let filter = DedupFilter::new(Duration::from_secs(5));
        let envelope = make_envelope();
        filter.mark(envelope.id);
        assert!(filter.is_duplicate(&envelope));
    }

    #[test]
    fn test_expired_event_is_not_duplicate() {
        let filter = DedupFilter::new(Duration::from_millis(1));
        let envelope = make_envelope();
        filter.mark(envelope.id);

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(5));
        assert!(!filter.is_duplicate(&envelope));
    }

    #[test]
    fn test_cleanup_removes_expired() {
        let filter = DedupFilter::new(Duration::from_millis(1));
        filter.mark(Uuid::new_v4());
        filter.mark(Uuid::new_v4());
        assert_eq!(filter.len(), 2);

        std::thread::sleep(Duration::from_millis(5));
        filter.cleanup();
        assert!(filter.is_empty());
    }

    #[test]
    fn test_cleanup_keeps_fresh_entries() {
        let filter = DedupFilter::new(Duration::from_secs(60));
        let id = Uuid::new_v4();
        filter.mark(id);
        filter.cleanup();
        assert_eq!(filter.len(), 1);
    }

    #[test]
    fn test_different_events_not_confused() {
        let filter = DedupFilter::new(Duration::from_secs(5));
        let e1 = make_envelope();
        let e2 = make_envelope();
        filter.mark(e1.id);

        assert!(filter.is_duplicate(&e1));
        assert!(!filter.is_duplicate(&e2));
    }
}
