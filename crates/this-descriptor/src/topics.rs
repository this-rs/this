//! # Le flux d'événements d'un module, dérivé de sa déclaration
//!
//! Un module ne déclare pas ses événements à part : **ses entités sont son flux**. Ce
//! module donne le vocabulaire commun à toutes les surfaces (REST, GraphQL, gRPC,
//! WebSocket, SSE) pour qu'elles publient et s'abonnent sur les mêmes sujets, sans
//! qu'aucune ne code en dur des noms d'entités.
//!
//! ```no_run
//! # fn demo(desc: &this_descriptor::ModuleDescriptor) {
//! for topic in desc.event_topics() {
//!     println!("{} : {:?}", topic.topic, topic.operations);
//! }
//! // shop.customer : [Create, Update, Delete]
//! // shop.order    : [Create, Update, Delete]
//! # }
//! ```

use serde::{Deserialize, Serialize};

use crate::module::ModuleDescriptor;
use crate::perm::Operation;

/// Un sujet écoutable, dérivé d'une entité déclarée.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTopic {
    /// L'entité déclarée (`order`).
    pub entity: String,
    /// Le nom canonique du sujet : `<module>.<entité>`.
    pub topic: String,
    /// Les opérations qui produisent un événement.
    ///
    /// Ce sont les écritures, pas toutes les opérations : un `list` ne notifie personne.
    /// La liste est fixe par construction, ce qui évite qu'une surface oublie `delete`.
    pub operations: Vec<Operation>,
}

/// Les opérations qui émettent un événement, dans l'ordre où elles arrivent.
pub const EMITTING_OPERATIONS: [Operation; 3] =
    [Operation::Create, Operation::Update, Operation::Delete];

impl ModuleDescriptor {
    /// Les sujets d'événements du module, un par entité déclarée.
    ///
    /// L'ordre suit la déclaration : c'est celui que verra un client qui affiche la liste
    /// des flux disponibles.
    pub fn event_topics(&self) -> Vec<EventTopic> {
        self.entities
            .iter()
            .map(|entity| EventTopic {
                entity: entity.name.clone(),
                topic: format!("{}.{}", self.name, entity.name),
                operations: EMITTING_OPERATIONS.to_vec(),
            })
            .collect()
    }

    /// Le sujet d'une entité, s'il est déclaré.
    pub fn topic_of(&self, entity: &str) -> Option<String> {
        self.entities
            .iter()
            .any(|declared| declared.name == entity)
            .then(|| format!("{}.{entity}", self.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityDesc;

    fn descriptor() -> ModuleDescriptor {
        let mut descriptor = ModuleDescriptor::new("shop", "0.2.0");
        descriptor.entities = vec![
            EntityDesc::new("customer"),
            EntityDesc::new("order"),
            EntityDesc::new("order_item"),
        ];
        descriptor
    }

    #[test]
    fn every_declared_entity_gets_a_topic() {
        let topics = descriptor().event_topics();
        assert_eq!(topics.len(), 3);
        assert_eq!(topics[1].entity, "order");
        assert_eq!(topics[1].topic, "shop.order");
    }

    #[test]
    fn the_topic_comes_from_the_module_name_not_a_constant() {
        let mut descriptor = descriptor();
        descriptor.name = "billing".into();
        assert_eq!(descriptor.event_topics()[0].topic, "billing.customer");
    }

    #[test]
    fn only_writes_emit() {
        let topics = descriptor().event_topics();
        assert_eq!(
            topics[0].operations,
            vec![Operation::Create, Operation::Update, Operation::Delete]
        );
        assert!(!topics[0].operations.contains(&Operation::List));
        assert!(!topics[0].operations.contains(&Operation::Get));
    }

    #[test]
    fn an_undeclared_entity_has_no_topic() {
        let descriptor = descriptor();
        assert_eq!(descriptor.topic_of("order").as_deref(), Some("shop.order"));
        assert!(descriptor.topic_of("fantome").is_none());
    }

    #[test]
    fn topics_are_serialisable_for_a_client_to_list() {
        let raw = serde_json::to_value(descriptor().event_topics()).unwrap();
        assert_eq!(raw[0]["topic"], "shop.customer");
        assert_eq!(raw[0]["operations"][0], "create");
    }
}
