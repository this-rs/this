//! # Le magasin d'enregistrements, à granularité d'entité
//!
//! Une **piste réactive par entité** : lire les produits n'abonne qu'aux produits. Un
//! changement de stock n'invalide donc pas les vues qui ne lisent que les commandes — c'est
//! l'équivalent, côté this-rs, d'un atome par collection dans un magasin à signaux fins.
//!
//! Le magasin ne connaît ni le module ni le transport : il reçoit un [`Module`] (la liste
//! des entités) et applique des [`Change`]. Les surfaces s'occupent de faire venir les
//! changements (SSE, événement Tauri, WebSocket).
//!
//! ```ignore
//! let store = Store::of(&module);        // une piste par entité
//! store.load(&snapshot);                 // état initial
//! store.apply(&change);                  // un seul enregistrement bouge
//! ```

use std::collections::BTreeMap;

use leptos::prelude::*;
use serde_json::Value;
use this_app::{Change, ChangeOp};

use crate::model::Module;

/// Les enregistrements d'une entité, indexés par identifiant.
pub type Index = BTreeMap<String, Value>;

pub struct Store {
    tracks: Vec<(String, RwSignal<Index>)>,
}

impl Store {
    /// Crée une piste par entité déclarée par le module. À appeler dans un contexte
    /// réactif (corps d'un composant).
    pub fn of(module: &Module) -> Self {
        Self {
            tracks: module
                .entity_names()
                .into_iter()
                .map(|name| (name, RwSignal::new(Index::new())))
                .collect(),
        }
    }

    fn track(&self, entity: &str) -> Option<RwSignal<Index>> {
        self.tracks
            .iter()
            .find(|(name, _)| name == entity)
            .map(|(_, signal)| *signal)
    }

    /// Remplace tout le contenu (chargement initial, réconciliation après erreur).
    pub fn load(&self, snapshot: &BTreeMap<String, Vec<Value>>) {
        for (name, signal) in &self.tracks {
            let index = snapshot
                .get(name)
                .map(|records| {
                    records
                        .iter()
                        .filter_map(|record| {
                            record["id"]
                                .as_str()
                                .map(|id| (id.to_string(), record.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            signal.set(index);
        }
    }

    /// Applique un changement : un seul enregistrement bouge, une seule piste est invalidée.
    pub fn apply(&self, change: &Change) {
        let Some(track) = self.track(&change.entity) else {
            return;
        };
        track.update(|index| match change.op {
            ChangeOp::Deleted => {
                index.remove(&change.id);
            }
            ChangeOp::Created | ChangeOp::Updated => {
                if let Some(record) = &change.record {
                    index.insert(change.id.clone(), record.clone());
                }
            }
        });
    }

    /// Lecture réactive des enregistrements d'une entité.
    pub fn records(&self, entity: &str) -> Vec<Value> {
        self.track(entity)
            .map(|track| track.get().into_values().collect())
            .unwrap_or_default()
    }

    pub fn count(&self, entity: &str) -> usize {
        self.track(entity)
            .map(|track| track.get().len())
            .unwrap_or(0)
    }

    pub fn get(&self, entity: &str, id: &str) -> Option<Value> {
        self.track(entity)
            .and_then(|track| track.get().get(id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entity, Module};
    use serde_json::json;
    use this_app::Change;

    fn module() -> Module {
        Module {
            name: "shop".into(),
            entities: ["customer", "product"]
                .iter()
                .map(|name| Entity {
                    name: (*name).into(),
                    fields: vec![],
                })
                .collect(),
            ..Default::default()
        }
    }

    fn snapshot() -> BTreeMap<String, Vec<Value>> {
        let mut snapshot = BTreeMap::new();
        snapshot.insert(
            "product".to_string(),
            vec![
                json!({ "id": "p1", "stock": 10 }),
                json!({ "id": "p2", "stock": 3 }),
            ],
        );
        snapshot.insert("customer".to_string(), vec![json!({ "id": "c1" })]);
        snapshot
    }

    #[test]
    fn load_indexes_records_by_id() {
        let module = module();
        let store = Store::of(&module);
        store.load(&snapshot());
        assert_eq!(store.count("product"), 2);
        assert_eq!(store.count("customer"), 1);
        assert_eq!(store.get("product", "p1").unwrap()["stock"], 10);
        // une entité déclarée mais sans données reste vide, pas en erreur
        assert!(store.records("order").is_empty());
    }

    #[test]
    fn apply_patches_a_single_record() {
        let module = module();
        let store = Store::of(&module);
        store.load(&snapshot());

        store.apply(&Change::updated(
            "product",
            json!({ "id": "p1", "stock": 9 }),
        ));
        assert_eq!(store.get("product", "p1").unwrap()["stock"], 9);
        assert_eq!(store.get("product", "p2").unwrap()["stock"], 3, "p2 intact");

        store.apply(&Change::created(
            "product",
            json!({ "id": "p3", "stock": 7 }),
        ));
        assert_eq!(store.count("product"), 3);

        store.apply(&Change::deleted("product", "p2"));
        assert_eq!(store.count("product"), 2);
        assert!(store.get("product", "p2").is_none());
    }

    #[test]
    fn a_change_on_an_unknown_entity_is_ignored() {
        let module = module();
        let store = Store::of(&module);
        store.load(&snapshot());
        // aucun panic : un module peut recevoir un écho d'une entité qu'il ne connaît pas
        store.apply(&Change::deleted("fantome", "x"));
        assert_eq!(store.count("customer"), 1);
    }

    #[test]
    fn a_deletion_removes_the_record() {
        let module = module();
        let store = Store::of(&module);
        store.load(&snapshot());
        store.apply(&Change::deleted("customer", "c1"));
        assert_eq!(store.count("customer"), 0);
    }
}
