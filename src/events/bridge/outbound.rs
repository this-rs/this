//! Outbound bridge (T8): FrameworkEvent → MutationBus
//!
//! When this-rs CRUD events happen (entity created, link deleted, etc.),
//! the outbound bridge translates them into obrain-db `MutationEvent`s
//! and publishes them to the `MutationBus` so cognitive listeners
//! (stigmergy, co-change detection, etc.) can react.

use super::codec::{edge_of_uuid, json_to_properties, node_of_uuid};
use super::dedup::DedupFilter;
use super::types::{
    EdgeSnapshot, EventContext, MutationBatch, MutationBusPublisher, MutationEvent, NodeSnapshot,
};
use crate::core::events::{EntityEvent, EventBus, FrameworkEvent, LinkEvent};
use obrain_common::types::ArcStr;
use serde_json::Value as Json;
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
                            // Le tenant vit dans le CONTEXTE d'evenement cote
                            // obrain, et c'est une chaine : on y met la forme
                            // texte de la poignee, qui se re-parse a l'identique
                            // au retour (voir `codec::uuid_of_tenant`).
                            let batch = match envelope.tenant_id {
                                Some(t) => MutationBatch::with_context(
                                    mutations,
                                    Arc::new(EventContext::new(Some(t.to_string()), None, None)),
                                ),
                                None => MutationBatch::new(mutations),
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
                        tracing::warn!(lagged = n, "OutboundBridge lagged, {} events dropped", n);
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
                // Un identifiant qui n'est pas une poignee obrain ne se traduit
                // pas : on preferera perdre l'evenement plutot que fabriquer une
                // identite etrangere dans le graphe.
                let id = node_of_uuid(*entity_id)?;
                Some(vec![MutationEvent::NodeCreated {
                    node: NodeSnapshot {
                        id,
                        labels: std::iter::once(ArcStr::from(entity_type.as_str())).collect(),
                        properties: json_to_properties(&objets(data)),
                    },
                }])
            }
            EntityEvent::Updated {
                entity_type,
                entity_id,
                data,
            } => {
                let id = node_of_uuid(*entity_id)?;
                let properties = json_to_properties(&objets(data));
                if properties.is_empty() {
                    return None;
                }
                // ⚠️ LIMITE DE MODELE, ECRITE ET NON MASQUEE : this-rs ne
                // transporte que l'etat NOUVEAU (`EntityEvent::Updated { data }`).
                // obrain, lui, definit une mise a jour comme un couple
                // `before` / `after`. On ne peut donc pas produire un `before`
                // fidele, et on n'invente pas de valeurs : `before` porte
                // l'IDENTITE (et les labels), avec des proprietes VIDES, ce qui
                // signifie « inconnu », pas « vide ».
                // Remede propre : que `EntityEvent::Updated` porte le precedent
                // (ou le delta) — c'est un changement du modele this-rs, pas du
                // pont, et il est volontairement laisse a decider.
                Some(vec![MutationEvent::NodeUpdated {
                    before: NodeSnapshot {
                        id,
                        labels: std::iter::once(ArcStr::from(entity_type.as_str())).collect(),
                        properties: Vec::new(),
                    },
                    after: NodeSnapshot {
                        id,
                        labels: std::iter::once(ArcStr::from(entity_type.as_str())).collect(),
                        properties,
                    },
                }])
            }
            EntityEvent::Deleted { entity_id, .. } => {
                let id = node_of_uuid(*entity_id)?;
                // Meme limite : l'evenement de suppression this-rs ne porte ni
                // labels ni proprietes. L'instantane est donc reduit a l'identite.
                Some(vec![MutationEvent::NodeDeleted {
                    node: NodeSnapshot {
                        id,
                        labels: Default::default(),
                        properties: Vec::new(),
                    },
                }])
            }
        },
        FrameworkEvent::Link(LinkEvent::Created {
            link_type,
            link_id,
            source_id,
            target_id,
            metadata,
        }) => Some(vec![MutationEvent::EdgeCreated {
            edge: EdgeSnapshot {
                id: edge_of_uuid(*link_id)?,
                src: node_of_uuid(*source_id)?,
                dst: node_of_uuid(*target_id)?,
                edge_type: ArcStr::from(link_type.as_str()),
                properties: metadata
                    .as_ref()
                    .map(objets)
                    .map(|o| json_to_properties(&o))
                    .unwrap_or_default(),
            },
        }]),
        FrameworkEvent::Link(LinkEvent::Deleted {
            link_type,
            link_id,
            source_id,
            target_id,
        }) => Some(vec![MutationEvent::EdgeDeleted {
            edge: EdgeSnapshot {
                id: edge_of_uuid(*link_id)?,
                src: node_of_uuid(*source_id)?,
                dst: node_of_uuid(*target_id)?,
                edge_type: ArcStr::from(link_type.as_str()),
                properties: Vec::new(),
            },
        }]),
        // Signaux cognitifs et effacements RGPD : ils viennent d'obrain lui-meme
        // ou n'ont pas de pendant graphe — on ne les renvoie pas (le filtre de
        // dedup couvre deja la boucle, ceci est la seconde ceinture).
        _ => None,
    }
}

/// Objet JSON d'une charge utile, ou objet vide si ce n'en est pas un.
///
/// obrain n'a pas de types composites : une charge qui n'est pas un objet ne
/// peut pas devenir des proprietes. On rend un objet vide plutot que de perdre
/// silencieusement une structure non representable.
fn objets(value: &Json) -> serde_json::Map<String, Json> {
    value.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::{EdgeId, NodeId};
    use serde_json::json;
    use uuid::Uuid;

    fn node_uuid(id: u64) -> Uuid {
        super::super::codec::uuid_of_node(NodeId(id))
    }
    fn edge_uuid(id: u64) -> Uuid {
        super::super::codec::uuid_of_edge(EdgeId(id))
    }

    #[test]
    fn created_becomes_node_created_with_a_real_snapshot() {
        let ev = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".into(),
            entity_id: node_uuid(7),
            data: json!({"total": 42, "ref": "A-1"}),
        });
        let muts = translate_to_mutations(&ev).unwrap();
        assert_eq!(muts.len(), 1);
        match &muts[0] {
            MutationEvent::NodeCreated { node } => {
                assert_eq!(node.id, NodeId(7));
                assert_eq!(node.labels.len(), 1);
                assert_eq!(node.labels[0].as_str(), "order");
                assert_eq!(node.properties.len(), 2);
                assert!(
                    node.properties.iter().any(|(k, v)| k.as_str() == "ref"
                        && *v == obrain_common::types::Value::from("A-1"))
                );
            }
            other => panic!("attendu NodeCreated, obtenu {other:?}"),
        }
    }

    #[test]
    fn updated_becomes_node_updated_and_before_is_unknown_not_fabricated() {
        let ev = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "order".into(),
            entity_id: node_uuid(7),
            data: json!({"total": 43}),
        });
        match &translate_to_mutations(&ev).unwrap()[0] {
            MutationEvent::NodeUpdated { before, after } => {
                assert_eq!(after.properties.len(), 1);
                assert!(
                    before.properties.is_empty(),
                    "l'etat precedent n'est pas connu : on ne l'invente pas"
                );
                assert_eq!(before.id, after.id, "mais l'identite est portee");
            }
            other => panic!("attendu NodeUpdated, obtenu {other:?}"),
        }
    }

    #[test]
    fn updated_without_fields_produces_nothing() {
        let ev = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "order".into(),
            entity_id: node_uuid(7),
            data: json!({}),
        });
        assert!(translate_to_mutations(&ev).is_none());
    }

    #[test]
    fn deleted_becomes_node_deleted() {
        let ev = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".into(),
            entity_id: node_uuid(9),
        });
        match &translate_to_mutations(&ev).unwrap()[0] {
            MutationEvent::NodeDeleted { node } => assert_eq!(node.id, NodeId(9)),
            other => panic!("attendu NodeDeleted, obtenu {other:?}"),
        }
    }

    #[test]
    fn links_use_edge_handles() {
        let created = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "owns".into(),
            link_id: edge_uuid(3),
            source_id: node_uuid(1),
            target_id: node_uuid(2),
            metadata: Some(json!({"since": 2024})),
        });
        match &translate_to_mutations(&created).unwrap()[0] {
            MutationEvent::EdgeCreated { edge } => {
                assert_eq!(edge.id, EdgeId(3));
                assert_eq!(edge.src, NodeId(1));
                assert_eq!(edge.dst, NodeId(2));
                assert_eq!(edge.edge_type.as_str(), "owns");
                assert_eq!(edge.properties.len(), 1);
            }
            other => panic!("attendu EdgeCreated, obtenu {other:?}"),
        }
        let deleted = FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: "owns".into(),
            link_id: edge_uuid(3),
            source_id: node_uuid(1),
            target_id: node_uuid(2),
        });
        assert!(matches!(
            translate_to_mutations(&deleted).unwrap()[0],
            MutationEvent::EdgeDeleted { .. }
        ));
    }

    #[test]
    fn a_foreign_identity_is_never_written_into_the_graph() {
        let ev = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".into(),
            entity_id: Uuid::new_v4(), // pas une poignee obrain
            data: json!({"a": 1}),
        });
        assert!(
            translate_to_mutations(&ev).is_none(),
            "on prefere perdre l'evenement plutot que creer une identite etrangere"
        );
    }

    /// Un composite n'est pas seulement CONSERVE : il garde sa structure.
    ///
    /// Ce test affirmait l'inverse avant le couplage aux vrais types — il
    /// documentait une perte (tableau JSON -> chaine) qui venait d'une erreur de
    /// ma part sur les capacites d'obrain. `Value::List` existe, donc la
    /// conversion est sans perte et le test dit maintenant la verite.
    #[test]
    fn composite_payload_keeps_its_structure() {
        let ev = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".into(),
            entity_id: node_uuid(5),
            data: json!({"items": [1, 2, 3], "meta": {"v": 2}}),
        });
        match &translate_to_mutations(&ev).unwrap()[0] {
            MutationEvent::NodeCreated { node } => {
                let get = |k: &str| {
                    node.properties
                        .iter()
                        .find(|(key, _)| key.as_str() == k)
                        .map(|(_, v)| v.clone())
                        .expect("propriete attendue")
                };
                match get("items") {
                    obrain_common::types::Value::List(items) => {
                        assert_eq!(items.len(), 3, "la liste EST une liste, pas une chaine");
                    }
                    other => panic!("attendu une List, obtenu {other:?}"),
                }
                match get("meta") {
                    obrain_common::types::Value::Map(m) => {
                        assert_eq!(m.len(), 1, "l'objet EST un objet");
                    }
                    other => panic!("attendu une Map, obtenu {other:?}"),
                }
            }
            other => panic!("attendu NodeCreated, obtenu {other:?}"),
        }
    }
}
