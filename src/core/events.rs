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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
}

impl EventEnvelope {
    /// Create a new event envelope
    pub fn new(event: FrameworkEvent) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event,
        }
    }
}

/// Broadcast-based event bus for the framework
///
/// Uses `tokio::sync::broadcast` which allows multiple receivers and is
/// designed for exactly this kind of pub/sub pattern.
///
/// The bus is cheap to clone (Arc internally) and can be shared across threads.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<EventEnvelope>,
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
        Self { sender }
    }

    /// Publish an event to all subscribers
    ///
    /// This is non-blocking and will never fail. If there are no subscribers,
    /// the event is simply dropped. If subscribers are lagging, they will
    /// receive a `Lagged` error on their next recv().
    ///
    /// Returns the number of receivers that will receive the event.
    pub fn publish(&self, event: FrameworkEvent) -> usize {
        let envelope = EventEnvelope::new(event);
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
}
