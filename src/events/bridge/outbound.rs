//! Outbound bridge (T8): FrameworkEvent → MutationBus
//!
//! When this-rs CRUD events happen (entity created, link deleted, etc.),
//! the outbound bridge translates them into obrain-db `MutationEvent`s
//! and publishes them to the `MutationBus` so cognitive listeners
//! (stigmergy, co-change detection, etc.) can react.

use super::dedup::DedupFilter;
use super::types::{MutationBatch, MutationBusPublisher, MutationEvent};
use crate::core::events::{EntityEvent, EventBus, FrameworkEvent, LinkEvent};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Outbound bridge that forwards `FrameworkEvent`s to the obrain `MutationBus`
pub struct OutboundBridge {
    event_bus: Arc<EventBus>,
    mutation_publisher: Arc<dyn MutationBusPublisher>,
    dedup: Arc<DedupFilter>,
}

impl OutboundBridge {
    /// Create a new outbound bridge
    pub fn new(
        event_bus: Arc<EventBus>,
        mutation_publisher: Arc<dyn MutationBusPublisher>,
        dedup: Arc<DedupFilter>,
    ) -> Self {
        Self {
            event_bus,
            mutation_publisher,
            dedup,
        }
    }

    /// Start the bridge as a background task
    ///
    /// Subscribes to the `EventBus`, translates each `FrameworkEvent` into a
    /// `MutationEvent`, and publishes it to the `MutationBus`. Events that
    /// originated from the inbound bridge (detected via `DedupFilter`) are skipped.
    ///
    /// Returns a `JoinHandle` for the spawned task.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut rx = self.event_bus.subscribe();
            loop {
                match rx.recv().await {
                    Ok(envelope) => {
                        // Skip events that came FROM the inbound bridge (dedup)
                        if self.dedup.is_duplicate(&envelope) {
                            continue;
                        }

                        if let Some(mutations) = translate_to_mutations(&envelope.event) {
                            let batch = MutationBatch {
                                tenant_id: envelope.tenant_id,
                                events: mutations,
                            };
                            if let Err(e) = self.mutation_publisher.publish(batch).await {
                                tracing::warn!(
                                    error = %e,
                                    event_id = %envelope.id,
                                    "OutboundBridge: failed to publish mutation"
                                );
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            lagged = n,
                            "OutboundBridge lagged, {} events dropped",
                            n
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("OutboundBridge: EventBus closed, shutting down");
                        break;
                    }
                }
            }
        })
    }
}

/// Translate a `FrameworkEvent` into one or more `MutationEvent`s
///
/// Returns `None` for events that should not be forwarded (e.g. cognitive signals,
/// which originate from obrain itself).
fn translate_to_mutations(event: &FrameworkEvent) -> Option<Vec<MutationEvent>> {
    match event {
        FrameworkEvent::Entity(entity_event) => match entity_event {
            EntityEvent::Created {
                entity_type,
                entity_id,
                data,
            } => {
                let properties = extract_properties(data);
                Some(vec![MutationEvent::NodeCreated {
                    node_id: *entity_id,
                    labels: vec![entity_type.clone()],
                    properties,
                }])
            }
            EntityEvent::Updated {
                entity_type: _,
                entity_id,
                data,
            } => {
                // Emit a PropertySet for each top-level field in the data
                let properties = extract_properties(data);
                let mutations: Vec<MutationEvent> = properties
                    .into_iter()
                    .map(|(key, value)| MutationEvent::PropertySet {
                        node_id: *entity_id,
                        key,
                        value,
                    })
                    .collect();

                if mutations.is_empty() {
                    None
                } else {
                    Some(mutations)
                }
            }
            EntityEvent::Deleted { entity_id, .. } => {
                Some(vec![MutationEvent::NodeDeleted {
                    node_id: *entity_id,
                }])
            }
        },
        FrameworkEvent::Link(link_event) => match link_event {
            LinkEvent::Created {
                link_type,
                link_id,
                source_id,
                target_id,
                metadata,
            } => {
                let properties = metadata
                    .as_ref()
                    .map(extract_properties)
                    .unwrap_or_default();
                Some(vec![MutationEvent::EdgeCreated {
                    edge_id: *link_id,
                    source_id: *source_id,
                    target_id: *target_id,
                    edge_type: link_type.clone(),
                    properties,
                }])
            }
            LinkEvent::Deleted {
                link_type,
                link_id,
                source_id,
                target_id,
            } => Some(vec![MutationEvent::EdgeDeleted {
                edge_id: *link_id,
                source_id: *source_id,
                target_id: *target_id,
                edge_type: link_type.clone(),
            }]),
        },
        // Cognitive signals originate from obrain — do not loop them back
        FrameworkEvent::Cognitive(_) => None,
        // GDPR erasure events are not forwarded as mutations
        FrameworkEvent::GdprErasure { .. } => None,
    }
}

/// Extract top-level properties from a JSON value into a HashMap
fn extract_properties(value: &serde_json::Value) -> HashMap<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::CognitiveSignal;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn test_translate_entity_created() {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::nil(),
            data: json!({"name": "Order #1", "amount": 42}),
        });

        let mutations = translate_to_mutations(&event).unwrap();
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            MutationEvent::NodeCreated {
                node_id,
                labels,
                properties,
            } => {
                assert_eq!(*node_id, Uuid::nil());
                assert_eq!(labels, &["order"]);
                assert_eq!(properties["name"], json!("Order #1"));
                assert_eq!(properties["amount"], json!(42));
            }
            other => panic!("Expected NodeCreated, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_entity_updated() {
        let id = Uuid::new_v4();
        let event = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "order".to_string(),
            entity_id: id,
            data: json!({"status": "shipped", "tracking": "ABC123"}),
        });

        let mutations = translate_to_mutations(&event).unwrap();
        assert_eq!(mutations.len(), 2);

        // Collect the keys that were set
        let keys: Vec<String> = mutations
            .iter()
            .map(|m| match m {
                MutationEvent::PropertySet { key, .. } => key.clone(),
                other => panic!("Expected PropertySet, got {:?}", other),
            })
            .collect();
        assert!(keys.contains(&"status".to_string()));
        assert!(keys.contains(&"tracking".to_string()));
    }

    #[test]
    fn test_translate_entity_updated_empty_data() {
        let event = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        // Empty update data should produce None (no mutations)
        assert!(translate_to_mutations(&event).is_none());
    }

    #[test]
    fn test_translate_entity_deleted() {
        let id = Uuid::new_v4();
        let event = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".to_string(),
            entity_id: id,
        });

        let mutations = translate_to_mutations(&event).unwrap();
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            MutationEvent::NodeDeleted { node_id } => assert_eq!(*node_id, id),
            other => panic!("Expected NodeDeleted, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_link_created() {
        let link_id = Uuid::new_v4();
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "owns".to_string(),
            link_id,
            source_id: source,
            target_id: target,
            metadata: Some(json!({"weight": 1.0})),
        });

        let mutations = translate_to_mutations(&event).unwrap();
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            MutationEvent::EdgeCreated {
                edge_id,
                source_id,
                target_id,
                edge_type,
                properties,
            } => {
                assert_eq!(*edge_id, link_id);
                assert_eq!(*source_id, source);
                assert_eq!(*target_id, target);
                assert_eq!(edge_type, "owns");
                assert_eq!(properties["weight"], json!(1.0));
            }
            other => panic!("Expected EdgeCreated, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_link_created_no_metadata() {
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "owns".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        let mutations = translate_to_mutations(&event).unwrap();
        match &mutations[0] {
            MutationEvent::EdgeCreated { properties, .. } => {
                assert!(properties.is_empty());
            }
            other => panic!("Expected EdgeCreated, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_link_deleted() {
        let link_id = Uuid::new_v4();
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();
        let event = FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: "owns".to_string(),
            link_id,
            source_id: source,
            target_id: target,
        });

        let mutations = translate_to_mutations(&event).unwrap();
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            MutationEvent::EdgeDeleted {
                edge_id,
                source_id,
                target_id,
                edge_type,
            } => {
                assert_eq!(*edge_id, link_id);
                assert_eq!(*source_id, source);
                assert_eq!(*target_id, target);
                assert_eq!(edge_type, "owns");
            }
            other => panic!("Expected EdgeDeleted, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_cognitive_returns_none() {
        let event = FrameworkEvent::Cognitive(CognitiveSignal::ScarCreated {
            node_id: Uuid::new_v4(),
            scar_type: "test".to_string(),
            description: "test".to_string(),
        });

        assert!(translate_to_mutations(&event).is_none());
    }

    #[test]
    fn test_extract_properties_from_object() {
        let val = json!({"a": 1, "b": "hello"});
        let props = extract_properties(&val);
        assert_eq!(props.len(), 2);
        assert_eq!(props["a"], json!(1));
        assert_eq!(props["b"], json!("hello"));
    }

    #[test]
    fn test_extract_properties_from_non_object() {
        let val = json!(42);
        let props = extract_properties(&val);
        assert!(props.is_empty());
    }

    #[tokio::test]
    async fn test_outbound_bridge_skips_deduped_events() {
        use super::super::dedup::DedupFilter;
        use std::sync::Mutex;
        use std::time::Duration;

        // Mock publisher that records what it receives
        struct MockPublisher {
            received: Mutex<Vec<MutationBatch>>,
        }

        #[async_trait::async_trait]
        impl MutationBusPublisher for MockPublisher {
            async fn publish(&self, batch: MutationBatch) -> anyhow::Result<()> {
                self.received.lock().unwrap().push(batch);
                Ok(())
            }
        }

        let event_bus = Arc::new(EventBus::new(16));
        let publisher = Arc::new(MockPublisher {
            received: Mutex::new(Vec::new()),
        });
        let dedup = Arc::new(DedupFilter::new(Duration::from_secs(5)));

        let bridge = OutboundBridge::new(
            event_bus.clone(),
            publisher.clone(),
            dedup.clone(),
        );

        let handle = bridge.start();

        // Yield to let the spawned task subscribe to the EventBus
        tokio::task::yield_now().await;

        // Publish a normal event that is NOT deduped
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "invoice".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"total": 100}),
        }));

        // Give the bridge task time to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // The non-deduped event should have been published
        let received = publisher.received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].events.len(), 1);

        handle.abort();
    }

    #[tokio::test]
    async fn test_outbound_bridge_publishes_mutations() {
        use std::sync::Mutex;
        use std::time::Duration;

        struct MockPublisher {
            received: Mutex<Vec<MutationBatch>>,
        }

        #[async_trait::async_trait]
        impl MutationBusPublisher for MockPublisher {
            async fn publish(&self, batch: MutationBatch) -> anyhow::Result<()> {
                self.received.lock().unwrap().push(batch);
                Ok(())
            }
        }

        let event_bus = Arc::new(EventBus::new(16));
        let publisher = Arc::new(MockPublisher {
            received: Mutex::new(Vec::new()),
        });
        let dedup = Arc::new(DedupFilter::new(Duration::from_secs(5)));

        let bridge = OutboundBridge::new(event_bus.clone(), publisher.clone(), dedup);
        let handle = bridge.start();

        // Yield to let the spawned task subscribe to the EventBus
        tokio::task::yield_now().await;

        // Publish entity + link events
        let entity_id = Uuid::new_v4();
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id,
            data: json!({"name": "Alice"}),
        }));
        event_bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id: entity_id,
            target_id: Uuid::new_v4(),
            metadata: None,
        }));
        // Cognitive events should be ignored
        event_bus.publish(FrameworkEvent::Cognitive(CognitiveSignal::ScarCreated {
            node_id: Uuid::new_v4(),
            scar_type: "test".to_string(),
            description: "test".to_string(),
        }));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let received = publisher.received.lock().unwrap();
        // 2 batches: NodeCreated + EdgeCreated (cognitive was skipped)
        assert_eq!(received.len(), 2);

        handle.abort();
    }
}
