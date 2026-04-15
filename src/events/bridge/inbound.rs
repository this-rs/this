//! Inbound bridge (T9): MutationEvent → EventBus
//!
//! When obrain-db emits mutation events (external graph changes from other
//! services or cognitive layers), the inbound bridge translates them into
//! `FrameworkEvent`s so that WS/SSE/GraphQL subscription clients see them.
//!
//! Only mutations whose node labels match a registered entity type are
//! translated. Internal graph mutations (e.g. pheromone trails, scar nodes)
//! are silently ignored.

use super::dedup::DedupFilter;
use super::types::{MutationBusSubscriber, MutationEvent};
use crate::core::events::{EntityEvent, EventBus, EventEnvelope, FrameworkEvent, LinkEvent};
use std::collections::HashSet;
use std::sync::Arc;

/// Inbound bridge that translates `MutationEvent`s into `FrameworkEvent`s
pub struct InboundBridge {
    event_bus: Arc<EventBus>,
    mutation_subscriber: Box<dyn MutationBusSubscriber>,
    dedup: Arc<DedupFilter>,
    /// Set of entity type names registered in this-rs (e.g. "order", "user")
    /// Only mutations with matching labels are forwarded.
    registered_types: HashSet<String>,
}

impl InboundBridge {
    /// Create a new inbound bridge
    ///
    /// # Arguments
    ///
    /// * `event_bus` - The this-rs EventBus to publish translated events to
    /// * `mutation_subscriber` - A subscriber to obrain-db's MutationBus
    /// * `dedup` - Shared dedup filter (same instance as the outbound bridge)
    /// * `registered_types` - Entity types registered in this-rs. Only mutations
    ///   whose labels intersect with this set are forwarded.
    pub fn new(
        event_bus: Arc<EventBus>,
        mutation_subscriber: Box<dyn MutationBusSubscriber>,
        dedup: Arc<DedupFilter>,
        registered_types: HashSet<String>,
    ) -> Self {
        Self {
            event_bus,
            mutation_subscriber,
            dedup,
            registered_types,
        }
    }

    /// Start the bridge as a background task
    ///
    /// Subscribes to the `MutationBus`, translates each `MutationEvent` into
    /// a `FrameworkEvent`, marks it in the `DedupFilter`, and publishes it
    /// to the `EventBus`.
    ///
    /// Returns a `JoinHandle` for the spawned task.
    pub fn start(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match self.mutation_subscriber.recv().await {
                    Some(batch) => {
                        for mutation in &batch.events {
                            if let Some(framework_event) =
                                translate_to_framework(mutation, &self.registered_types)
                            {
                                let envelope = match batch.tenant_id {
                                    Some(tenant_id) => {
                                        EventEnvelope::new_with_tenant(framework_event, tenant_id)
                                    }
                                    None => EventEnvelope::new(framework_event),
                                };

                                // Mark in dedup filter BEFORE publishing so the
                                // outbound bridge sees it when it receives the event
                                self.dedup.mark(envelope.id);

                                let receivers = self.event_bus.publish_envelope_raw(envelope);

                                tracing::debug!(
                                    receivers = receivers,
                                    tenant_id = ?batch.tenant_id,
                                    "InboundBridge: forwarded mutation to EventBus"
                                );
                            }
                        }
                    }
                    None => {
                        tracing::info!("InboundBridge: MutationBus closed, shutting down");
                        break;
                    }
                }
            }
        })
    }
}

/// Translate a `MutationEvent` into a `FrameworkEvent`
///
/// Only translates events whose labels/types match a registered entity type.
/// Returns `None` for internal graph mutations that have no corresponding
/// this-rs entity.
fn translate_to_framework(
    event: &MutationEvent,
    registered_types: &HashSet<String>,
) -> Option<FrameworkEvent> {
    match event {
        MutationEvent::NodeCreated {
            node_id,
            labels,
            properties,
        } => {
            // Find the first label that matches a registered type
            let entity_type = labels
                .iter()
                .find(|l| registered_types.contains(l.as_str()))?;
            let data = serde_json::Value::Object(
                properties
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
            Some(FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: entity_type.clone(),
                entity_id: *node_id,
                data,
            }))
        }
        MutationEvent::NodeDeleted { node_id } => {
            // We don't know the entity_type from a bare NodeDeleted, so we use
            // a sentinel. In practice, obrain-db would include labels, but our
            // stub doesn't. We emit a delete with an empty entity_type which
            // downstream handlers can interpret.
            //
            // NOTE: A real integration should carry labels on NodeDeleted too.
            Some(FrameworkEvent::Entity(EntityEvent::Deleted {
                entity_type: String::new(),
                entity_id: *node_id,
            }))
        }
        MutationEvent::PropertySet {
            node_id,
            key,
            value,
        } => {
            // PropertySet becomes an entity update with a single field
            let data = serde_json::json!({ key.clone(): value.clone() });
            Some(FrameworkEvent::Entity(EntityEvent::Updated {
                entity_type: String::new(),
                entity_id: *node_id,
                data,
            }))
        }
        MutationEvent::EdgeCreated {
            edge_id,
            source_id,
            target_id,
            edge_type,
            properties,
        } => {
            let metadata = if properties.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(
                    properties
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                ))
            };
            Some(FrameworkEvent::Link(LinkEvent::Created {
                link_type: edge_type.clone(),
                link_id: *edge_id,
                source_id: *source_id,
                target_id: *target_id,
                metadata,
            }))
        }
        MutationEvent::EdgeDeleted {
            edge_id,
            source_id,
            target_id,
            edge_type,
        } => Some(FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: edge_type.clone(),
            link_id: *edge_id,
            source_id: *source_id,
            target_id: *target_id,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn registered() -> HashSet<String> {
        let mut set = HashSet::new();
        set.insert("order".to_string());
        set.insert("user".to_string());
        set
    }

    #[test]
    fn test_translate_node_created_matching_label() {
        let mut props = HashMap::new();
        props.insert("name".to_string(), json!("Alice"));

        let event = MutationEvent::NodeCreated {
            node_id: Uuid::nil(),
            labels: vec!["user".to_string()],
            properties: props,
        };

        let result = translate_to_framework(&event, &registered()).unwrap();
        match result {
            FrameworkEvent::Entity(EntityEvent::Created {
                entity_type,
                entity_id,
                data,
            }) => {
                assert_eq!(entity_type, "user");
                assert_eq!(entity_id, Uuid::nil());
                assert_eq!(data["name"], json!("Alice"));
            }
            other => panic!("Expected Entity::Created, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_node_created_no_matching_label() {
        let event = MutationEvent::NodeCreated {
            node_id: Uuid::nil(),
            labels: vec!["pheromone_trail".to_string()],
            properties: HashMap::new(),
        };

        // Internal graph node — should be ignored
        assert!(translate_to_framework(&event, &registered()).is_none());
    }

    #[test]
    fn test_translate_node_created_multiple_labels_picks_registered() {
        let event = MutationEvent::NodeCreated {
            node_id: Uuid::nil(),
            labels: vec![
                "internal_meta".to_string(),
                "order".to_string(),
                "tracked".to_string(),
            ],
            properties: HashMap::new(),
        };

        let result = translate_to_framework(&event, &registered()).unwrap();
        match result {
            FrameworkEvent::Entity(EntityEvent::Created { entity_type, .. }) => {
                assert_eq!(entity_type, "order");
            }
            other => panic!("Expected Entity::Created, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_node_deleted() {
        let id = Uuid::new_v4();
        let event = MutationEvent::NodeDeleted { node_id: id };

        let result = translate_to_framework(&event, &registered()).unwrap();
        match result {
            FrameworkEvent::Entity(EntityEvent::Deleted { entity_id, .. }) => {
                assert_eq!(entity_id, id);
            }
            other => panic!("Expected Entity::Deleted, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_property_set() {
        let id = Uuid::new_v4();
        let event = MutationEvent::PropertySet {
            node_id: id,
            key: "status".to_string(),
            value: json!("active"),
        };

        let result = translate_to_framework(&event, &registered()).unwrap();
        match result {
            FrameworkEvent::Entity(EntityEvent::Updated {
                entity_id, data, ..
            }) => {
                assert_eq!(entity_id, id);
                assert_eq!(data["status"], json!("active"));
            }
            other => panic!("Expected Entity::Updated, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_edge_created_with_properties() {
        let event = MutationEvent::EdgeCreated {
            edge_id: Uuid::nil(),
            source_id: Uuid::nil(),
            target_id: Uuid::nil(),
            edge_type: "follows".to_string(),
            properties: {
                let mut m = HashMap::new();
                m.insert("since".to_string(), json!("2025-01-01"));
                m
            },
        };

        let result = translate_to_framework(&event, &registered()).unwrap();
        match result {
            FrameworkEvent::Link(LinkEvent::Created {
                link_type,
                metadata,
                ..
            }) => {
                assert_eq!(link_type, "follows");
                assert_eq!(metadata.unwrap()["since"], json!("2025-01-01"));
            }
            other => panic!("Expected Link::Created, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_edge_created_no_properties() {
        let event = MutationEvent::EdgeCreated {
            edge_id: Uuid::nil(),
            source_id: Uuid::nil(),
            target_id: Uuid::nil(),
            edge_type: "owns".to_string(),
            properties: HashMap::new(),
        };

        let result = translate_to_framework(&event, &registered()).unwrap();
        match result {
            FrameworkEvent::Link(LinkEvent::Created { metadata, .. }) => {
                assert!(metadata.is_none());
            }
            other => panic!("Expected Link::Created, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_edge_deleted() {
        let edge_id = Uuid::new_v4();
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();
        let event = MutationEvent::EdgeDeleted {
            edge_id,
            source_id: source,
            target_id: target,
            edge_type: "owns".to_string(),
        };

        let result = translate_to_framework(&event, &registered()).unwrap();
        match result {
            FrameworkEvent::Link(LinkEvent::Deleted {
                link_type,
                link_id,
                source_id,
                target_id,
            }) => {
                assert_eq!(link_type, "owns");
                assert_eq!(link_id, edge_id);
                assert_eq!(source_id, source);
                assert_eq!(target_id, target);
            }
            other => panic!("Expected Link::Deleted, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_inbound_bridge_forwards_mutations() {
        use super::super::types::MutationBatch;
        use std::time::Duration;
        use tokio::sync::mpsc;

        // Mock subscriber backed by an mpsc channel
        struct MockSubscriber {
            rx: mpsc::Receiver<MutationBatch>,
        }

        #[async_trait::async_trait]
        impl MutationBusSubscriber for MockSubscriber {
            async fn recv(&mut self) -> Option<MutationBatch> {
                self.rx.recv().await
            }
        }

        let (tx, rx) = mpsc::channel(16);
        let event_bus = Arc::new(EventBus::new(16));
        let dedup = Arc::new(DedupFilter::new(Duration::from_secs(5)));

        let mut event_rx = event_bus.subscribe();

        let bridge = InboundBridge::new(
            event_bus.clone(),
            Box::new(MockSubscriber { rx }),
            dedup.clone(),
            registered(),
        );
        let handle = bridge.start();

        // Send a mutation batch
        let node_id = Uuid::new_v4();
        tx.send(MutationBatch {
            tenant_id: None,
            events: vec![MutationEvent::NodeCreated {
                node_id,
                labels: vec!["order".to_string()],
                properties: {
                    let mut m = HashMap::new();
                    m.insert("total".to_string(), json!(99));
                    m
                },
            }],
        })
        .await
        .unwrap();

        // Receive the translated event
        let received = tokio::time::timeout(Duration::from_millis(200), event_rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");

        assert_eq!(received.event.event_kind(), "entity");
        assert_eq!(received.event.action(), "created");
        assert_eq!(received.event.entity_id(), Some(node_id));

        // The event should be marked in the dedup filter
        assert!(dedup.is_duplicate(&received));

        // Close the channel to shut down the bridge
        drop(tx);
        let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
    }

    #[tokio::test]
    async fn test_inbound_bridge_ignores_unregistered_types() {
        use super::super::types::MutationBatch;
        use std::time::Duration;
        use tokio::sync::mpsc;

        struct MockSubscriber {
            rx: mpsc::Receiver<MutationBatch>,
        }

        #[async_trait::async_trait]
        impl MutationBusSubscriber for MockSubscriber {
            async fn recv(&mut self) -> Option<MutationBatch> {
                self.rx.recv().await
            }
        }

        let (tx, rx) = mpsc::channel(16);
        let event_bus = Arc::new(EventBus::new(16));
        let dedup = Arc::new(DedupFilter::new(Duration::from_secs(5)));
        let mut event_rx = event_bus.subscribe();

        let bridge = InboundBridge::new(
            event_bus.clone(),
            Box::new(MockSubscriber { rx }),
            dedup,
            registered(),
        );
        let handle = bridge.start();

        // Send a mutation with an unregistered type (should be ignored)
        tx.send(MutationBatch {
            tenant_id: None,
            events: vec![MutationEvent::NodeCreated {
                node_id: Uuid::new_v4(),
                labels: vec!["pheromone_trail".to_string()],
                properties: HashMap::new(),
            }],
        })
        .await
        .unwrap();

        // Then send one with a registered type
        let node_id = Uuid::new_v4();
        tx.send(MutationBatch {
            tenant_id: None,
            events: vec![MutationEvent::NodeCreated {
                node_id,
                labels: vec!["user".to_string()],
                properties: HashMap::new(),
            }],
        })
        .await
        .unwrap();

        // We should only receive the registered one
        let received = tokio::time::timeout(Duration::from_millis(200), event_rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");
        assert_eq!(received.event.entity_id(), Some(node_id));

        drop(tx);
        let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
    }

    #[tokio::test]
    async fn test_inbound_bridge_tenant_propagation() {
        use super::super::types::MutationBatch;
        use std::time::Duration;
        use tokio::sync::mpsc;

        struct MockSubscriber {
            rx: mpsc::Receiver<MutationBatch>,
        }

        #[async_trait::async_trait]
        impl MutationBusSubscriber for MockSubscriber {
            async fn recv(&mut self) -> Option<MutationBatch> {
                self.rx.recv().await
            }
        }

        let (tx, rx) = mpsc::channel(16);
        let event_bus = Arc::new(EventBus::new(16));
        let dedup = Arc::new(DedupFilter::new(Duration::from_secs(5)));
        let mut event_rx = event_bus.subscribe();

        let bridge = InboundBridge::new(
            event_bus.clone(),
            Box::new(MockSubscriber { rx }),
            dedup,
            registered(),
        );
        let handle = bridge.start();

        let tenant = Uuid::new_v4();
        tx.send(MutationBatch {
            tenant_id: Some(tenant),
            events: vec![MutationEvent::NodeDeleted {
                node_id: Uuid::new_v4(),
            }],
        })
        .await
        .unwrap();

        let received = tokio::time::timeout(Duration::from_millis(200), event_rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");

        assert_eq!(received.tenant_id(), Some(tenant));

        drop(tx);
        let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
    }
}
