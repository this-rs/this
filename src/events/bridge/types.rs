//! Obrain-db MutationEvent types (local stubs)
//!
//! These types mirror obrain-db's `MutationEvent` and `MutationBatch` types.
//! They are defined locally until obrain-db becomes a direct dependency,
//! at which point they will be replaced by re-exports.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Represents a mutation event from obrain-db's MutationBus
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MutationEvent {
    /// A graph node was created
    NodeCreated {
        node_id: Uuid,
        labels: Vec<String>,
        properties: HashMap<String, serde_json::Value>,
    },
    /// A graph node was deleted
    NodeDeleted { node_id: Uuid },
    /// A property was set on a node
    PropertySet {
        node_id: Uuid,
        key: String,
        value: serde_json::Value,
    },
    /// An edge was created between two nodes
    EdgeCreated {
        edge_id: Uuid,
        source_id: Uuid,
        target_id: Uuid,
        edge_type: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// An edge was deleted
    EdgeDeleted {
        edge_id: Uuid,
        source_id: Uuid,
        target_id: Uuid,
        edge_type: String,
    },
}

/// A batch of mutations (obrain groups mutations into batches)
#[derive(Debug, Clone)]
pub struct MutationBatch {
    /// Tenant that produced the mutations (None = no tenant / global)
    pub tenant_id: Option<Uuid>,
    /// The mutation events in this batch
    pub events: Vec<MutationEvent>,
}

/// Trait for receiving mutation events (will be implemented by real obrain-db MutationBus)
#[async_trait::async_trait]
pub trait MutationBusSubscriber: Send + Sync {
    /// Receive the next mutation batch, or `None` if the bus is closed
    async fn recv(&mut self) -> Option<MutationBatch>;
}

/// Trait for publishing mutation events
#[async_trait::async_trait]
pub trait MutationBusPublisher: Send + Sync {
    /// Publish a batch of mutations to the MutationBus
    async fn publish(&self, batch: MutationBatch) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mutation_event_node_created_serialization() {
        let mut props = HashMap::new();
        props.insert("name".to_string(), json!("Alice"));

        let event = MutationEvent::NodeCreated {
            node_id: Uuid::nil(),
            labels: vec!["user".to_string()],
            properties: props,
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["NodeCreated"]["labels"][0], "user");
    }

    #[test]
    fn test_mutation_event_edge_created_serialization() {
        let event = MutationEvent::EdgeCreated {
            edge_id: Uuid::nil(),
            source_id: Uuid::nil(),
            target_id: Uuid::nil(),
            edge_type: "owns".to_string(),
            properties: HashMap::new(),
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["EdgeCreated"]["edge_type"], "owns");
    }

    #[test]
    fn test_mutation_event_roundtrip() {
        let event = MutationEvent::PropertySet {
            node_id: Uuid::new_v4(),
            key: "status".to_string(),
            value: json!("active"),
        };

        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: MutationEvent = serde_json::from_str(&serialized).unwrap();
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_mutation_batch_creation() {
        let batch = MutationBatch {
            tenant_id: Some(Uuid::new_v4()),
            events: vec![
                MutationEvent::NodeDeleted {
                    node_id: Uuid::new_v4(),
                },
                MutationEvent::EdgeDeleted {
                    edge_id: Uuid::new_v4(),
                    source_id: Uuid::new_v4(),
                    target_id: Uuid::new_v4(),
                    edge_type: "owns".to_string(),
                },
            ],
        };

        assert!(batch.tenant_id.is_some());
        assert_eq!(batch.events.len(), 2);
    }
}
