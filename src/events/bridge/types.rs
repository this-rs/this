//! Pont de types entre this-rs et obrain — **ré-exports des vrais types**.
//!
//! # Ce qui a changé, et pourquoi
//!
//! Ce fichier contenait des **stubs locaux** qui « reproduisaient » `MutationEvent`
//! et `MutationBatch` « jusqu'à ce que obrain-db devienne une dépendance directe ».
//! La dépendance est maintenant déclarée (`obrain-reactive`, feature `obrain`),
//! donc les stubs sont remplacés par des ré-exports.
//!
//! Il était temps : **les deux définitions avaient déjà divergé**, ce que le
//! commentaire d'origine annonçait comme un risque futur et qui était déjà un fait.
//!
//! | stub local | vrai type obrain |
//! |---|---|
//! | `node_id: Uuid` | `id: NodeId` |
//! | `properties: HashMap<String, serde_json::Value>` | `properties: Vec<(PropertyKey, Value)>` |
//! | `labels: Vec<String>` | `labels: SmallVec<[ArcStr; 2]>` |
//! | variante `PropertySet { node_id, key, value }` | **absente** : une écriture de propriété est un `NodeUpdated { before, after }` |
//! | — | `NodeUpdated`, `EdgeUpdated` (états avant/après) |
//! | `MutationBatch { tenant_id, events }` | `{ events, timestamp: Instant, context: Option<Arc<EventContext>> }` |
//!
//! # Conséquence de fond : rien ne se sérialise
//!
//! Les événements d'obrain dérivent `Debug, Clone` **seulement** — pas `Serialize`.
//! C'est cohérent avec le chemin rapide : le pont est **in-process**, il n'y a
//! aucun octet à mettre sur un fil. Les anciens tests de sérialisation JSON
//! testaient donc une propriété que le vrai modèle n'a pas ; ils sont remplacés
//! par des tests de structure et de contrat.
//!
//! # Ce que this-rs garde
//!
//! Les deux traits de bus. obrain n'a pas d'équivalent : son `MutationBus` expose
//! `subscribe() -> broadcast::Receiver`. Garder l'abstraction côté this-rs permet
//! de brancher un bus obrain réel **ou** un bus de test, sans que les appelants
//! connaissent `tokio::broadcast`.

pub use obrain_reactive::{
    EdgeSnapshot, EventContext, MutationBatch, MutationBus, MutationEvent, NodeSnapshot,
};

use async_trait::async_trait;

/// Réception d'un lot de mutations.
#[async_trait]
pub trait MutationBusSubscriber: Send + Sync {
    /// Reçoit le prochain lot, ou `None` si le bus est fermé.
    async fn recv(&mut self) -> Option<MutationBatch>;
}

/// Émission d'un lot de mutations.
#[async_trait]
pub trait MutationBusPublisher: Send + Sync {
    /// Publie un lot vers le bus de mutations.
    async fn publish(&self, batch: MutationBatch) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::{NodeId, PropertyKey, Value};

    fn node(id: u64, nom: &str) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId(id),
            labels: Default::default(),
            properties: vec![(PropertyKey::from("name"), Value::from(nom))],
        }
    }

    /// Un `NodeCreated` porte l'INSTANTANÉ complet : plus besoin de requêter le
    /// store en retour, ce que l'ancien stub (id + map JSON) ne permettait pas.
    #[test]
    fn node_created_carries_the_full_snapshot() {
        let e = MutationEvent::NodeCreated {
            node: node(7, "Alice"),
        };
        match e {
            MutationEvent::NodeCreated { node } => {
                assert_eq!(node.properties.len(), 1);
                assert_eq!(node.properties[0].0, PropertyKey::from("name"));
                assert_eq!(node.properties[0].1, Value::from("Alice"));
            }
            _ => panic!("variante inattendue"),
        }
    }

    /// Une écriture de propriété est une MISE À JOUR avec état avant/après —
    /// il n'existe pas de variante de delta de propriété. C'est la décision de
    /// modèle que le stub masquait.
    #[test]
    fn a_property_write_is_an_update_with_before_and_after() {
        let before = node(7, "Alice");
        let after = node(7, "Alicia");
        let e = MutationEvent::NodeUpdated { before, after };
        match e {
            MutationEvent::NodeUpdated { before, after } => {
                assert_ne!(before.properties, after.properties);
                assert_eq!(before.id, after.id, "même entité des deux côtés");
            }
            _ => panic!("variante inattendue"),
        }
    }

    /// Le lot obrain porte un horodatage et un contexte optionnel — le tenant
    /// n'est plus un champ du lot comme dans le stub, il vit dans le contexte.
    #[test]
    fn batch_carries_timestamp_and_optional_context() {
        let b = MutationBatch::new(vec![MutationEvent::NodeDeleted {
            node: node(9, "Bob"),
        }]);
        assert_eq!(b.events.len(), 1);
        assert!(b.context.is_none(), "bootstrap : pas de contexte");
        let ctx = EventContext::new(Some("tenant-a".into()), None, None);
        // `with_context` est une FONCTION ASSOCIÉE qui prend les événements :
        // ce n'est pas un builder. (Corrigé après vérification de la signature.)
        let b2 = MutationBatch::with_context(vec![], std::sync::Arc::new(ctx));
        assert_eq!(
            b2.context.as_ref().and_then(|c| c.tenant_id.clone()),
            Some("tenant-a".to_string())
        );
    }

    /// Le bus d'obrain est utilisable comme source de réception : c'est le
    /// chemin in-process (broadcast `tokio`, pas de socket, pas d'octets).
    #[test]
    fn the_real_bus_delivers_in_process() {
        let bus = MutationBus::new();
        let mut rx = bus.subscribe();
        // `publish` rend un booléen (au moins un abonné a reçu), pas un compte.
        let delivered = bus.publish(MutationEvent::NodeDeleted { node: node(1, "x") });
        assert!(delivered, "un abonné était présent");
        let got = rx
            .try_recv()
            .expect("le lot doit être disponible sans bloquer");
        assert_eq!(got.events.len(), 1);
    }
}
