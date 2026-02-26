//! WebSocket message protocol definitions
//!
//! Defines the JSON messages exchanged between WebSocket clients and the server.
//!
//! ## Client → Server Messages
//!
//! ```json
//! // Subscribe to events
//! {"type": "subscribe", "filter": {"entity_type": "order", "event_type": "created"}}
//!
//! // Unsubscribe
//! {"type": "unsubscribe", "subscription_id": "sub_abc123"}
//!
//! // Keepalive
//! {"type": "ping"}
//! ```
//!
//! ## Server → Client Messages
//!
//! ```json
//! // Event notification
//! {"type": "event", "subscription_id": "sub_abc123", "data": {...}}
//!
//! // Subscription confirmed
//! {"type": "subscribed", "subscription_id": "sub_abc123", "filter": {...}}
//!
//! // Unsubscription confirmed
//! {"type": "unsubscribed", "subscription_id": "sub_abc123"}
//!
//! // Keepalive response
//! {"type": "pong"}
//!
//! // Error
//! {"type": "error", "message": "Invalid subscription filter"}
//! ```

use crate::core::events::{EventEnvelope, FrameworkEvent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Messages sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Subscribe to events matching a filter
    Subscribe {
        /// Filter criteria for events
        filter: SubscriptionFilter,
    },
    /// Unsubscribe from a specific subscription
    Unsubscribe {
        /// The subscription ID to remove
        subscription_id: String,
    },
    /// Keepalive ping
    Ping,
}

/// Messages sent from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// An event matching a subscription
    Event {
        /// Which subscription matched this event
        subscription_id: String,
        /// The event envelope with metadata
        data: EventEnvelope,
    },
    /// Subscription confirmation
    Subscribed {
        /// The assigned subscription ID
        subscription_id: String,
        /// The filter that was registered
        filter: SubscriptionFilter,
    },
    /// Unsubscription confirmation
    Unsubscribed {
        /// The subscription ID that was removed
        subscription_id: String,
    },
    /// Keepalive response
    Pong,
    /// Error message
    Error {
        /// Human-readable error description
        message: String,
    },
    /// Welcome message on connection
    Welcome {
        /// Unique connection ID
        connection_id: String,
    },
}

/// Filter criteria for event subscriptions
///
/// All fields are optional. When a field is `None`, it acts as a wildcard
/// (matches everything). When set, only events matching that field are delivered.
///
/// # Examples
///
/// Subscribe to all events:
/// ```json
/// {}
/// ```
///
/// Subscribe to all order events:
/// ```json
/// {"entity_type": "order"}
/// ```
///
/// Subscribe to a specific entity:
/// ```json
/// {"entity_type": "order", "entity_id": "550e8400-e29b-41d4-a716-446655440000"}
/// ```
///
/// Subscribe to all creation events:
/// ```json
/// {"event_type": "created"}
/// ```
///
/// Subscribe to order creations only:
/// ```json
/// {"entity_type": "order", "event_type": "created"}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubscriptionFilter {
    /// Filter by entity type (e.g., "order", "invoice")
    /// None = match all entity types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,

    /// Filter by specific entity ID
    /// None = match all entities of the type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<Uuid>,

    /// Filter by event type: "created", "updated", "deleted"
    /// None = match all event types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,

    /// Filter by event kind: "entity" or "link"
    /// None = match both entity and link events
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl SubscriptionFilter {
    /// Check if an event matches this filter
    ///
    /// All fields act as AND conditions. A `None` field matches everything.
    pub fn matches(&self, event: &FrameworkEvent) -> bool {
        // Check kind filter
        if let Some(ref kind) = self.kind
            && event.event_kind() != kind
        {
            return false;
        }

        // Check entity_type filter
        if let Some(ref entity_type) = self.entity_type {
            match event.entity_type() {
                Some(et) if et == entity_type => {}
                Some(_) => return false,
                // Link events don't have entity_type — if filtering by entity_type, skip links
                None => return false,
            }
        }

        // Check entity_id filter
        if let Some(entity_id) = self.entity_id {
            match event.entity_id() {
                Some(eid) if eid == entity_id => {}
                Some(_) => return false,
                None => return false,
            }
        }

        // Check event_type (action) filter
        if let Some(ref event_type) = self.event_type
            && event.action() != event_type
        {
            return false;
        }

        true
    }
}

/// A subscription with its filter and a unique ID
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Unique subscription ID
    pub id: String,
    /// The filter for this subscription
    pub filter: SubscriptionFilter,
}

impl Subscription {
    /// Create a new subscription with a generated ID
    pub fn new(filter: SubscriptionFilter) -> Self {
        Self {
            id: format!("sub_{}", Uuid::new_v4().simple()),
            filter,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, LinkEvent};
    use serde_json::json;

    // === Serialization tests ===

    #[test]
    fn test_client_message_subscribe_serialization() {
        let msg = ClientMessage::Subscribe {
            filter: SubscriptionFilter {
                entity_type: Some("order".to_string()),
                entity_id: None,
                event_type: Some("created".to_string()),
                kind: None,
            },
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "subscribe");
        assert_eq!(json["filter"]["entity_type"], "order");
        assert_eq!(json["filter"]["event_type"], "created");
    }

    #[test]
    fn test_client_message_unsubscribe_serialization() {
        let msg = ClientMessage::Unsubscribe {
            subscription_id: "sub_123".to_string(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "unsubscribe");
        assert_eq!(json["subscription_id"], "sub_123");
    }

    #[test]
    fn test_client_message_ping_serialization() {
        let msg = ClientMessage::Ping;
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "ping");
    }

    #[test]
    fn test_server_message_event_serialization() {
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"amount": 42}),
        }));

        let msg = ServerMessage::Event {
            subscription_id: "sub_123".to_string(),
            data: envelope,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "event");
        assert_eq!(json["subscription_id"], "sub_123");
        assert!(json["data"]["event"].is_object());
    }

    #[test]
    fn test_server_message_pong_serialization() {
        let msg = ServerMessage::Pong;
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "pong");
    }

    #[test]
    fn test_server_message_error_serialization() {
        let msg = ServerMessage::Error {
            message: "Something went wrong".to_string(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["message"], "Something went wrong");
    }

    // === Deserialization round-trip tests ===

    #[test]
    fn test_client_message_subscribe_roundtrip() {
        let json_str =
            r#"{"type":"subscribe","filter":{"entity_type":"order","event_type":"created"}}"#;
        let msg: ClientMessage = serde_json::from_str(json_str).unwrap();

        match msg {
            ClientMessage::Subscribe { filter } => {
                assert_eq!(filter.entity_type.as_deref(), Some("order"));
                assert_eq!(filter.event_type.as_deref(), Some("created"));
                assert!(filter.entity_id.is_none());
                assert!(filter.kind.is_none());
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_client_message_ping_roundtrip() {
        let json_str = r#"{"type":"ping"}"#;
        let msg: ClientMessage = serde_json::from_str(json_str).unwrap();
        assert!(matches!(msg, ClientMessage::Ping));
    }

    #[test]
    fn test_subscription_filter_empty_roundtrip() {
        let json_str = r#"{}"#;
        let filter: SubscriptionFilter = serde_json::from_str(json_str).unwrap();
        assert!(filter.entity_type.is_none());
        assert!(filter.entity_id.is_none());
        assert!(filter.event_type.is_none());
        assert!(filter.kind.is_none());
    }

    // === Filter matching tests ===

    #[test]
    fn test_filter_empty_matches_everything() {
        let filter = SubscriptionFilter::default();

        let event1 = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let event2 = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "has_invoice".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(filter.matches(&event1));
        assert!(filter.matches(&event2));
    }

    #[test]
    fn test_filter_by_entity_type() {
        let filter = SubscriptionFilter {
            entity_type: Some("order".to_string()),
            ..Default::default()
        };

        let order_event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let invoice_event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "invoice".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link_event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "has_invoice".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(filter.matches(&order_event));
        assert!(!filter.matches(&invoice_event));
        assert!(!filter.matches(&link_event)); // Links don't have entity_type
    }

    #[test]
    fn test_filter_by_entity_id() {
        let target_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();

        let filter = SubscriptionFilter {
            entity_id: Some(target_id),
            ..Default::default()
        };

        let matching = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "order".to_string(),
            entity_id: target_id,
            data: json!({}),
        });

        let not_matching = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "order".to_string(),
            entity_id: other_id,
            data: json!({}),
        });

        assert!(filter.matches(&matching));
        assert!(!filter.matches(&not_matching));
    }

    #[test]
    fn test_filter_by_event_type() {
        let filter = SubscriptionFilter {
            event_type: Some("deleted".to_string()),
            ..Default::default()
        };

        let deleted = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
        });

        let created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(filter.matches(&deleted));
        assert!(!filter.matches(&created));
    }

    #[test]
    fn test_filter_by_kind() {
        let filter = SubscriptionFilter {
            kind: Some("link".to_string()),
            ..Default::default()
        };

        let entity_event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link_event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "has_invoice".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(!filter.matches(&entity_event));
        assert!(filter.matches(&link_event));
    }

    #[test]
    fn test_filter_combined_entity_type_and_action() {
        let filter = SubscriptionFilter {
            entity_type: Some("order".to_string()),
            event_type: Some("created".to_string()),
            ..Default::default()
        };

        let order_created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let order_deleted = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
        });

        let invoice_created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "invoice".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(filter.matches(&order_created));
        assert!(!filter.matches(&order_deleted));
        assert!(!filter.matches(&invoice_created));
    }

    #[test]
    fn test_subscription_generates_unique_id() {
        let sub1 = Subscription::new(SubscriptionFilter::default());
        let sub2 = Subscription::new(SubscriptionFilter::default());

        assert_ne!(sub1.id, sub2.id);
        assert!(sub1.id.starts_with("sub_"));
        assert!(sub2.id.starts_with("sub_"));
    }

    #[test]
    fn test_malformed_json_deserialization_error() {
        let malformed = r#"{"type": "subscribe", "filter": "not_an_object"}"#;
        let result = serde_json::from_str::<ClientMessage>(malformed);
        assert!(result.is_err(), "malformed JSON should fail to deserialize");
    }

    #[test]
    fn test_unknown_message_type_deserialization_error() {
        let unknown = r#"{"type": "unknown_action", "data": {}}"#;
        let result = serde_json::from_str::<ClientMessage>(unknown);
        assert!(
            result.is_err(),
            "unknown message type should fail to deserialize"
        );
    }

    #[test]
    fn test_missing_required_fields_deserialization_error() {
        // Subscribe requires a "filter" field
        let missing_filter = r#"{"type": "subscribe"}"#;
        let result = serde_json::from_str::<ClientMessage>(missing_filter);
        assert!(
            result.is_err(),
            "subscribe without filter should fail to deserialize"
        );

        // Unsubscribe requires a "subscription_id" field
        let missing_sub_id = r#"{"type": "unsubscribe"}"#;
        let result = serde_json::from_str::<ClientMessage>(missing_sub_id);
        assert!(
            result.is_err(),
            "unsubscribe without subscription_id should fail to deserialize"
        );
    }

    #[test]
    fn test_server_message_welcome_roundtrip() {
        let msg = ServerMessage::Welcome {
            connection_id: "conn_abc123".to_string(),
        };

        let json_str = serde_json::to_string(&msg).expect("Welcome should serialize");
        let deserialized: ServerMessage =
            serde_json::from_str(&json_str).expect("Welcome should deserialize");

        match deserialized {
            ServerMessage::Welcome { connection_id } => {
                assert_eq!(connection_id, "conn_abc123");
            }
            _ => panic!("Expected Welcome message"),
        }
    }

    #[test]
    fn test_server_message_subscribed_roundtrip() {
        let msg = ServerMessage::Subscribed {
            subscription_id: "sub_xyz789".to_string(),
            filter: SubscriptionFilter {
                entity_type: Some("invoice".to_string()),
                entity_id: None,
                event_type: Some("created".to_string()),
                kind: None,
            },
        };

        let json_str = serde_json::to_string(&msg).expect("Subscribed should serialize");
        let deserialized: ServerMessage =
            serde_json::from_str(&json_str).expect("Subscribed should deserialize");

        match deserialized {
            ServerMessage::Subscribed {
                subscription_id,
                filter,
            } => {
                assert_eq!(subscription_id, "sub_xyz789");
                assert_eq!(filter.entity_type.as_deref(), Some("invoice"));
                assert_eq!(filter.event_type.as_deref(), Some("created"));
                assert!(filter.entity_id.is_none());
                assert!(filter.kind.is_none());
            }
            _ => panic!("Expected Subscribed message"),
        }
    }
}
