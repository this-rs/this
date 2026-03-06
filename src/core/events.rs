//! Internal event system for real-time notifications
//!
//! The EventBus is the core of the real-time system. It uses `tokio::sync::broadcast`
//! to decouple mutations (REST, GraphQL handlers) from notifications (WebSocket, SSE).
//!
//! # Architecture
//!
//! ```text
//! REST Handler ──┐
//!                ├──▶ EventBus::publish() ──▶ broadcast channel ──▶ WebSocket subscribers
//! GraphQL Handler┘                                                ──▶ SSE subscribers
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! let event_bus = EventBus::new(1024);
//!
//! // Subscribe to events
//! let mut rx = event_bus.subscribe();
//!
//! // Publish an event (non-blocking, fire-and-forget)
//! event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
//!     entity_type: "order".to_string(),
//!     entity_id: Uuid::new_v4(),
//!     data: json!({"name": "Order #1"}),
//! }));
//!
//! // Receive events
//! if let Ok(event) = rx.recv().await {
//!     println!("Received: {:?}", event);
//! }
//! ```

use crate::events::log::EventLog;
use crate::events::types::SeqNo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Events related to entity mutations (create, update, delete)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum EntityEvent {
    /// An entity was created
    Created {
        entity_type: String,
        entity_id: Uuid,
        data: serde_json::Value,
    },
    /// An entity was updated
    Updated {
        entity_type: String,
        entity_id: Uuid,
        data: serde_json::Value,
    },
    /// An entity was deleted
    Deleted {
        entity_type: String,
        entity_id: Uuid,
    },
}

/// Events related to link mutations (create, delete)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum LinkEvent {
    /// A link was created between two entities
    Created {
        link_type: String,
        link_id: Uuid,
        source_id: Uuid,
        target_id: Uuid,
        metadata: Option<serde_json::Value>,
    },
    /// A link was deleted
    Deleted {
        link_type: String,
        link_id: Uuid,
        source_id: Uuid,
        target_id: Uuid,
    },
}

/// Top-level framework event that wraps entity and link events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameworkEvent {
    /// An entity event
    Entity(EntityEvent),
    /// A link event
    Link(LinkEvent),
}

impl FrameworkEvent {
    /// Get the timestamp of the event (generated at creation time)
    /// Note: timestamp is added by EventEnvelope, not by the event itself
    pub fn event_kind(&self) -> &str {
        match self {
            FrameworkEvent::Entity(_) => "entity",
            FrameworkEvent::Link(_) => "link",
        }
    }

    /// Get the entity type this event relates to
    pub fn entity_type(&self) -> Option<&str> {
        match self {
            FrameworkEvent::Entity(e) => match e {
                EntityEvent::Created { entity_type, .. }
                | EntityEvent::Updated { entity_type, .. }
                | EntityEvent::Deleted { entity_type, .. } => Some(entity_type),
            },
            FrameworkEvent::Link(_) => None,
        }
    }

    /// Get the entity ID this event relates to (if applicable)
    pub fn entity_id(&self) -> Option<Uuid> {
        match self {
            FrameworkEvent::Entity(e) => match e {
                EntityEvent::Created { entity_id, .. }
                | EntityEvent::Updated { entity_id, .. }
                | EntityEvent::Deleted { entity_id, .. } => Some(*entity_id),
            },
            FrameworkEvent::Link(l) => match l {
                LinkEvent::Created { link_id, .. } | LinkEvent::Deleted { link_id, .. } => {
                    Some(*link_id)
                }
            },
        }
    }

    /// Get the action name (created, updated, deleted)
    pub fn action(&self) -> &str {
        match self {
            FrameworkEvent::Entity(e) => match e {
                EntityEvent::Created { .. } => "created",
                EntityEvent::Updated { .. } => "updated",
                EntityEvent::Deleted { .. } => "deleted",
            },
            FrameworkEvent::Link(l) => match l {
                LinkEvent::Created { .. } => "created",
                LinkEvent::Deleted { .. } => "deleted",
            },
        }
    }
}

/// Envelope wrapping a framework event with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Unique event ID
    pub id: Uuid,
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// The actual event
    pub event: FrameworkEvent,
    /// Sequence number assigned by the EventLog (None if not yet persisted)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seq_no: Option<SeqNo>,
}

impl EventEnvelope {
    /// Create a new event envelope
    pub fn new(event: FrameworkEvent) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event,
            seq_no: None,
        }
    }
}

/// Broadcast-based event bus for the framework
///
/// Uses `tokio::sync::broadcast` which allows multiple receivers and is
/// designed for exactly this kind of pub/sub pattern.
///
/// The bus is cheap to clone (Arc internally) and can be shared across threads.
///
/// # EventLog Bridge
///
/// When an `EventLog` is attached via `with_event_log()`, every published event
/// is also appended to the persistent log. The EventLog becomes the source of
/// truth, while the broadcast channel remains the real-time notification path.
///
/// ```text
/// publish(event) ──┬──▶ broadcast channel (real-time, fire-and-forget)
///                  └──▶ EventLog.append() (persistent, replayable)
/// ```
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<EventEnvelope>,
    /// Optional persistent event log (bridge)
    event_log: Option<Arc<dyn EventLog>>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("sender", &self.sender)
            .field("has_event_log", &self.event_log.is_some())
            .finish()
    }
}

impl EventBus {
    /// Create a new EventBus with the given channel capacity
    ///
    /// The capacity determines how many events can be buffered before
    /// slow receivers start losing events (lagged).
    ///
    /// # Arguments
    ///
    /// * `capacity` - Buffer size for the broadcast channel (recommended: 1024)
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            event_log: None,
        }
    }

    /// Attach a persistent EventLog to this bus
    ///
    /// When set, every `publish()` call also appends the event to the log.
    /// The append is done via `tokio::spawn` to avoid blocking the publisher.
    ///
    /// This enables the event flow system to consume events from the durable
    /// log instead of the ephemeral broadcast channel.
    pub fn with_event_log(mut self, event_log: Arc<dyn EventLog>) -> Self {
        self.event_log = Some(event_log);
        self
    }

    /// Get a reference to the attached EventLog, if any
    pub fn event_log(&self) -> Option<&Arc<dyn EventLog>> {
        self.event_log.as_ref()
    }

    /// Publish an event to all subscribers
    ///
    /// This is non-blocking and will never fail. If there are no subscribers,
    /// the event is simply dropped. If subscribers are lagging, they will
    /// receive a `Lagged` error on their next recv().
    ///
    /// If an EventLog is attached, the event is also appended to the log
    /// asynchronously (fire-and-forget via tokio::spawn).
    ///
    /// Returns the number of broadcast receivers that will receive the event.
    pub fn publish(&self, event: FrameworkEvent) -> usize {
        // Create a single envelope shared between broadcast and EventLog
        let envelope = EventEnvelope::new(event);

        // If an EventLog is attached, append a clone to it (non-blocking)
        if let Some(event_log) = &self.event_log {
            let log = event_log.clone();
            let envelope_clone = envelope.clone();
            tokio::spawn(async move {
                if let Err(e) = log.append(envelope_clone).await {
                    tracing::warn!("Failed to append event to EventLog: {}", e);
                }
            });
        }

        // send() returns Err only if there are no receivers, which is fine
        self.sender.send(envelope).unwrap_or(0)
    }

    /// Subscribe to events
    ///
    /// Returns a receiver that will get all future events published to the bus.
    /// Events published before this call are not received.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.sender.subscribe()
    }

    /// Get the current number of active subscribers
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_entity_event_created() {
        let event = EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "Order #1"}),
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["action"], "created");
        assert_eq!(json["entity_type"], "order");
    }

    #[test]
    fn test_link_event_created() {
        let event = LinkEvent::Created {
            link_type: "has_invoice".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: Some(json!({"priority": "high"})),
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["action"], "created");
        assert_eq!(json["link_type"], "has_invoice");
    }

    #[test]
    fn test_framework_event_entity_type() {
        let event = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "invoice".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"status": "paid"}),
        });

        assert_eq!(event.entity_type(), Some("invoice"));
        assert_eq!(event.action(), "updated");
        assert_eq!(event.event_kind(), "entity");
    }

    #[test]
    fn test_framework_event_link() {
        let event = FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: "has_invoice".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
        });

        assert_eq!(event.entity_type(), None);
        assert_eq!(event.action(), "deleted");
        assert_eq!(event.event_kind(), "link");
    }

    #[test]
    fn test_event_envelope_has_metadata() {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let envelope = EventEnvelope::new(event);
        assert!(!envelope.id.is_nil());
        assert!(envelope.timestamp <= Utc::now());
    }

    #[test]
    fn test_event_envelope_serialization_roundtrip() {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"amount": 42.0}),
        });

        let envelope = EventEnvelope::new(event);
        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: EventEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(envelope.id, deserialized.id);
        assert_eq!(envelope.event.event_kind(), deserialized.event.event_kind());
    }

    #[tokio::test]
    async fn test_event_bus_publish_subscribe() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();

        let entity_id = Uuid::new_v4();
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id,
            data: json!({"name": "Test Order"}),
        });

        let receivers = bus.publish(event);
        assert_eq!(receivers, 1);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.event.entity_id(), Some(entity_id));
        assert_eq!(received.event.action(), "created");
    }

    #[tokio::test]
    async fn test_event_bus_multiple_subscribers() {
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        assert_eq!(bus.receiver_count(), 2);

        let event = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
        });

        let receivers = bus.publish(event);
        assert_eq!(receivers, 2);

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();

        assert_eq!(e1.id, e2.id); // Same event envelope
    }

    #[test]
    fn test_event_bus_publish_without_subscribers() {
        let bus = EventBus::new(16);

        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        // Should not panic even with no subscribers
        let receivers = bus.publish(event);
        assert_eq!(receivers, 0);
    }

    #[test]
    fn test_event_bus_default() {
        let bus = EventBus::default();
        assert_eq!(bus.receiver_count(), 0);
    }

    #[test]
    fn test_event_bus_clone() {
        let bus = EventBus::new(16);
        let _rx = bus.subscribe();

        let bus2 = bus.clone();
        assert_eq!(bus2.receiver_count(), 1);

        let _rx2 = bus2.subscribe();
        assert_eq!(bus.receiver_count(), 2);
    }

    #[test]
    fn test_entity_event_deleted_serialization() {
        let entity_id = Uuid::new_v4();
        let event = EntityEvent::Deleted {
            entity_type: "invoice".to_string(),
            entity_id,
        };

        let json = serde_json::to_value(&event).expect("EntityEvent::Deleted should serialize");
        assert_eq!(json["action"], "deleted");
        assert_eq!(json["entity_type"], "invoice");
        assert_eq!(json["entity_id"], entity_id.to_string());
        // Deleted variant should NOT have a "data" field
        assert!(json.get("data").is_none());
    }

    #[test]
    fn test_link_event_deleted_serialization() {
        let link_id = Uuid::new_v4();
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let event = LinkEvent::Deleted {
            link_type: "ownership".to_string(),
            link_id,
            source_id,
            target_id,
        };

        let json = serde_json::to_value(&event).expect("LinkEvent::Deleted should serialize");
        assert_eq!(json["action"], "deleted");
        assert_eq!(json["link_type"], "ownership");
        assert_eq!(json["link_id"], link_id.to_string());
        assert_eq!(json["source_id"], source_id.to_string());
        assert_eq!(json["target_id"], target_id.to_string());
        // Deleted variant should NOT have metadata
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn test_framework_event_entity_id_for_link_created() {
        let link_id = Uuid::new_v4();
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "worker".to_string(),
            link_id,
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        // entity_id() on a Link event should return the link_id
        assert_eq!(event.entity_id(), Some(link_id));
        // entity_type() should return None for link events
        assert_eq!(event.entity_type(), None);
    }

    #[test]
    fn test_framework_event_pattern_matching_all_entity_actions() {
        let id = Uuid::new_v4();

        let created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: id,
            data: json!({}),
        });
        assert_eq!(created.action(), "created");
        assert_eq!(created.event_kind(), "entity");
        assert_eq!(created.entity_type(), Some("order"));
        assert_eq!(created.entity_id(), Some(id));

        let updated = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "order".to_string(),
            entity_id: id,
            data: json!({"status": "shipped"}),
        });
        assert_eq!(updated.action(), "updated");

        let deleted = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".to_string(),
            entity_id: id,
        });
        assert_eq!(deleted.action(), "deleted");
        assert_eq!(deleted.entity_id(), Some(id));
    }

    #[test]
    fn test_framework_event_pattern_matching_all_link_actions() {
        let link_id = Uuid::new_v4();

        let created = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "driver".to_string(),
            link_id,
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: Some(json!({"license": "B"})),
        });
        assert_eq!(created.action(), "created");
        assert_eq!(created.event_kind(), "link");
        assert_eq!(created.entity_id(), Some(link_id));

        let deleted = FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: "driver".to_string(),
            link_id,
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
        });
        assert_eq!(deleted.action(), "deleted");
        assert_eq!(deleted.event_kind(), "link");
        assert_eq!(deleted.entity_id(), Some(link_id));
    }

    #[tokio::test]
    async fn test_event_bus_without_event_log() {
        let bus = EventBus::new(16);
        assert!(bus.event_log().is_none());

        let mut rx = bus.subscribe();
        bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        let received = rx.recv().await.unwrap();
        assert_eq!(received.event.action(), "created");
    }

    #[tokio::test]
    async fn test_event_bus_with_event_log_bridge() {
        use crate::events::log::EventLog;
        use crate::events::memory::InMemoryEventLog;

        let event_log = Arc::new(InMemoryEventLog::new());
        let bus = EventBus::new(16).with_event_log(event_log.clone());

        assert!(bus.event_log().is_some());

        let mut rx = bus.subscribe();

        // Publish an event
        bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "Alice"}),
        }));

        // Should receive via broadcast
        let received = rx.recv().await.unwrap();
        assert_eq!(received.event.entity_type(), Some("user"));

        // Wait for the spawned task to complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Event should also be in the EventLog
        assert_eq!(event_log.last_seq_no().await, Some(1));
    }

    #[tokio::test]
    async fn test_event_bus_bridge_multiple_events() {
        use crate::events::log::EventLog;
        use crate::events::memory::InMemoryEventLog;
        use crate::events::types::SeekPosition;
        use tokio_stream::StreamExt;

        let event_log = Arc::new(InMemoryEventLog::new());
        let bus = EventBus::new(16).with_event_log(event_log.clone());

        // Publish 5 events
        for i in 0..5 {
            bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: format!("type_{i}"),
                entity_id: Uuid::new_v4(),
                data: json!({}),
            }));
        }

        // Wait for spawned tasks
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // All events should be in the log
        assert_eq!(event_log.last_seq_no().await, Some(5));

        // Subscribe from beginning and replay
        let stream = event_log
            .subscribe("test", SeekPosition::Beginning)
            .await
            .unwrap();
        let events: Vec<_> = stream.take(5).collect().await;
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn test_event_bus_backward_compatible_default() {
        // Default bus has no event_log — same behavior as before
        let bus = EventBus::default();
        assert!(bus.event_log().is_none());
        assert_eq!(bus.receiver_count(), 0);

        // Publishing without subscribers or log should not panic
        let receivers = bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        assert_eq!(receivers, 0);
    }
}
