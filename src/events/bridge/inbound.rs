//! Inbound bridge (T9): MutationEvent → EventBus
//!
//! When obrain-db emits mutation events (external graph changes from other
//! services or cognitive layers), the inbound bridge translates them into
//! `FrameworkEvent`s so that WS/SSE/GraphQL subscription clients see them.
//!
//! Only mutations whose node labels match a registered entity type are
//! translated. Internal graph mutations (e.g. pheromone trails, scar nodes)
//! are silently ignored.

use super::codec::{properties_to_json, uuid_of_edge, uuid_of_node, uuid_of_tenant};
use super::dedup::DedupFilter;
use super::types::{MutationBusSubscriber, MutationEvent};
use crate::core::events::{EntityEvent, EventBus, EventEnvelope, FrameworkEvent, LinkEvent};
use obrain_common::types::ArcStr;
use serde_json::Value as Json;
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
                                // Le tenant n'est plus un champ du lot : il vit dans
                                // `EventContext`, et c'est une CHAÎNE cote obrain alors
                                // que l'enveloppe this-rs porte un Uuid. On derive donc
                                // une poignee stable (`uuid_of_tenant`) : un tenant qui
                                // est deja un UUID garde son identite, un nom libre
                                // obtient une poignee deterministe — meme nom, meme
                                // routage, sans table.
                                let tenant_id = batch
                                    .context
                                    .as_ref()
                                    .and_then(|c| c.tenant_id.as_deref())
                                    .map(uuid_of_tenant);
                                let envelope = match tenant_id {
                                    Some(t) => EventEnvelope::new_with_tenant(framework_event, t),
                                    None => EventEnvelope::new(framework_event),
                                };

                                // Mark in dedup filter BEFORE publishing so the
                                // outbound bridge sees it when it receives the event
                                self.dedup.mark(envelope.id);

                                let receivers = self.event_bus.publish_envelope_raw(envelope);

                                tracing::debug!(
                                    receivers = receivers,
                                    tenant_id = ?tenant_id,
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
        MutationEvent::NodeCreated { node } => {
            let entity_type = label_connu(&node.labels, registered_types)?;
            Some(FrameworkEvent::Entity(EntityEvent::Created {
                entity_type,
                entity_id: uuid_of_node(node.id),
                data: Json::Object(properties_to_json(&node.properties)),
            }))
        }
        // `NodeUpdated` n'existait PAS dans le stub : une ecriture de propriete y
        // etait un delta (`PropertySet`). Le vrai modele porte deux etats.
        MutationEvent::NodeUpdated { after, .. } => {
            let entity_type = label_connu(&after.labels, registered_types)?;
            Some(FrameworkEvent::Entity(EntityEvent::Updated {
                entity_type,
                entity_id: uuid_of_node(after.id),
                data: Json::Object(properties_to_json(&after.properties)),
            }))
        }
        MutationEvent::NodeDeleted { node } => {
            // CE QUE LE COUPLAGE A CORRIGE. L'ancienne branche, faute de labels
            // dans le stub, emettait une sentinelle `String::new()` — le code
            // portait meme la dette en commentaire (« A real integration should
            // carry labels on NodeDeleted too »). Le vrai `NodeDeleted` porte
            // l'instantane complet : les labels sont la, la sentinelle disparait.
            // Un noeud supprime dont aucun label n'est un type enregistre est
            // desormais ignore au lieu d'etre diffuse avec un type vide.
            let entity_type = label_connu(&node.labels, registered_types)?;
            Some(FrameworkEvent::Entity(EntityEvent::Deleted {
                entity_type,
                entity_id: uuid_of_node(node.id),
            }))
        }
        MutationEvent::EdgeCreated { edge } => Some(FrameworkEvent::Link(LinkEvent::Created {
            link_type: edge.edge_type.as_str().to_string(),
            link_id: uuid_of_edge(edge.id),
            source_id: uuid_of_node(edge.src),
            target_id: uuid_of_node(edge.dst),
            metadata: if edge.properties.is_empty() {
                None
            } else {
                Some(Json::Object(properties_to_json(&edge.properties)))
            },
        })),
        MutationEvent::EdgeDeleted { edge } => Some(FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: edge.edge_type.as_str().to_string(),
            link_id: uuid_of_edge(edge.id),
            source_id: uuid_of_node(edge.src),
            target_id: uuid_of_node(edge.dst),
        })),
        // this-rs n'a que `LinkEvent::Created` et `LinkEvent::Deleted` : une
        // mise a jour d'arete n'a pas de pendant cote client. On ne l'invente
        // pas — perte assumee et ecrite ici plutot que silencieuse.
        MutationEvent::EdgeUpdated { .. } => None,
    }
}

/// Premier label de l'instantane qui correspond a un type d'entite this-rs.
///
/// Retourne `None` si aucun ne correspond : c'est le filtre qui ecarte les
/// mutations internes d'obrain (traces de pheromones, noeuds de cicatrice).
fn label_connu(labels: &[ArcStr], registered: &HashSet<String>) -> Option<String> {
    labels
        .iter()
        .find(|l| registered.contains(l.as_str()))
        .map(|l| l.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::{NodeId, PropertyKey, Value};
    use serde_json::json;

    fn registered() -> HashSet<String> {
        let mut set = HashSet::new();
        set.insert("order".to_string());
        set.insert("user".to_string());
        set
    }

    fn snap(id: u64, label: &str) -> obrain_reactive::NodeSnapshot {
        obrain_reactive::NodeSnapshot {
            id: NodeId(id),
            labels: vec![ArcStr::from(label)].into_iter().collect(),
            properties: vec![(PropertyKey::from("name"), Value::from("Alice"))],
        }
    }

    #[test]
    fn node_created_matching_label_becomes_entity_created() {
        let e = MutationEvent::NodeCreated {
            node: snap(7, "user"),
        };
        match translate_to_framework(&e, &registered()).unwrap() {
            FrameworkEvent::Entity(EntityEvent::Created {
                entity_type,
                entity_id,
                data,
            }) => {
                assert_eq!(entity_type, "user");
                assert_eq!(entity_id, uuid_of_node(NodeId(7)), "poignee stable");
                assert_eq!(data["name"], json!("Alice"));
            }
            other => panic!("attendu Created, obtenu {other:?}"),
        }
    }

    /// LA correction apportee par le couplage : les labels sont disponibles sur
    /// une suppression, donc plus de sentinelle vide — et un noeud dont aucun
    /// label n'est enregistre est ECARTE au lieu d'etre diffuse vide.
    #[test]
    fn node_deleted_carries_labels_now() {
        let e = MutationEvent::NodeDeleted {
            node: snap(9, "order"),
        };
        match translate_to_framework(&e, &registered()).unwrap() {
            FrameworkEvent::Entity(EntityEvent::Deleted { entity_type, .. }) => {
                assert_eq!(
                    entity_type, "order",
                    "le label reel, plus la sentinelle vide"
                );
            }
            other => panic!("attendu Deleted, obtenu {other:?}"),
        }
        // label inconnu : ecarte
        let inconnu = MutationEvent::NodeDeleted {
            node: snap(9, "pheromone"),
        };
        assert!(translate_to_framework(&inconnu, &registered()).is_none());
    }

    #[test]
    fn node_updated_becomes_entity_updated() {
        let e = MutationEvent::NodeUpdated {
            before: snap(7, "user"),
            after: snap(7, "user"),
        };
        assert!(matches!(
            translate_to_framework(&e, &registered()).unwrap(),
            FrameworkEvent::Entity(EntityEvent::Updated { .. })
        ));
    }

    #[test]
    fn a_foreign_uuid_is_never_translated() {
        // Un evenement dont l'identite n'est pas une poignee obrain ne doit pas
        // produire d'identite stranger : on verifie l'invariant par le codec.
        assert!(node_of_uuid_check(uuid::Uuid::new_v4()));
        fn node_of_uuid_check(u: uuid::Uuid) -> bool {
            super::super::codec::node_of_uuid(u).is_none()
        }
    }

    #[test]
    fn edge_created_and_deleted_use_edge_handles() {
        let edge = obrain_reactive::EdgeSnapshot {
            id: obrain_common::types::EdgeId(3),
            src: NodeId(1),
            dst: NodeId(2),
            edge_type: ArcStr::from("owns"),
            properties: vec![],
        };
        match translate_to_framework(
            &MutationEvent::EdgeCreated { edge: edge.clone() },
            &registered(),
        )
        .unwrap()
        {
            FrameworkEvent::Link(LinkEvent::Created {
                link_type, link_id, ..
            }) => {
                assert_eq!(link_type, "owns");
                assert_eq!(link_id, uuid_of_edge(obrain_common::types::EdgeId(3)));
            }
            other => panic!("attendu Link::Created, obtenu {other:?}"),
        }
        assert!(matches!(
            translate_to_framework(&MutationEvent::EdgeDeleted { edge }, &registered()).unwrap(),
            FrameworkEvent::Link(LinkEvent::Deleted { .. })
        ));
    }

    #[test]
    fn edge_updated_has_no_counterpart_and_is_not_invented() {
        let edge = obrain_reactive::EdgeSnapshot {
            id: obrain_common::types::EdgeId(3),
            src: NodeId(1),
            dst: NodeId(2),
            edge_type: ArcStr::from("owns"),
            properties: vec![],
        };
        let e = MutationEvent::EdgeUpdated {
            before: edge.clone(),
            after: edge,
        };
        assert!(translate_to_framework(&e, &registered()).is_none());
    }

    #[test]
    fn tenant_handle_is_stable_and_uuid_aware() {
        let a = uuid_of_tenant("tenant-a");
        assert_eq!(a, uuid_of_tenant("tenant-a"), "meme nom, meme poignee");
        assert_ne!(a, uuid_of_tenant("tenant-b"));
        let as_uuid = "0b6f4a3c-0000-4000-8000-000000000001";
        assert_eq!(
            uuid_of_tenant(as_uuid),
            uuid::Uuid::parse_str(as_uuid).unwrap(),
            "un tenant deja UUID garde son identite"
        );
    }
}
