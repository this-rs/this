//! Where the rows of a DECLARATIVE entity live.
//!
//! A declarative module (a `ModuleDescriptor` read from a store, no Rust code) is served
//! by [`JsonEntityService`](super::JsonEntityService). Its rows used to live in a
//! `RwLock<IndexMap<Uuid, Value>>` — correct, and lost when the process stops. This module
//! is the seam that makes the store swappable, so the same entity can be served from
//! memory, from `obrain`, or from anything that implements [`RowStore`], **without the
//! routes, the validation or the events changing**.
//!
//! The trait is deliberately small and *entity-scoped*: one instance per declared entity,
//! five operations. Everything above it (validation by the descriptor, `created_at` /
//! `updated_at`, events, REST, filters) stays in `JsonEntityService` — a store is not a
//! place to put business rules.
//!
//! # `patch`, and why the merge is here
//!
//! `update` is a read-modify-write on ONE row: it must merge the validated patch into the
//! stored object **atomically**. Exposing `get` then `replace` would let two concurrent
//! updates overwrite each other's keys. So the store owns the merge ([`RowStore::patch`])
//! and answers with the merged row, which is exactly what the service needs to publish.

use anyhow::Result;
use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// The rows of one declared entity.
#[async_trait]
pub trait RowStore: Send + Sync {
    /// The row, or `None` if it does not exist (or was deleted).
    async fn get(&self, id: &Uuid) -> Result<Option<Value>>;

    /// Rows in insertion order, `offset` skipped then at most `limit` returned.
    async fn list(&self, limit: Option<usize>, offset: usize) -> Result<Vec<Value>>;

    /// Store a NEW row under `id`. Overwriting an existing id is a caller bug: the service
    /// generates a fresh UUID per create.
    async fn insert(&self, id: Uuid, row: Value) -> Result<()>;

    /// Merge `patch` (an object) into the stored row, atomically, and return the merged
    /// row. `None` = the row does not exist, and nothing was written.
    async fn patch(&self, id: &Uuid, patch: &Value) -> Result<Option<Value>>;

    /// Remove the row. `false` = it was not there.
    async fn remove(&self, id: &Uuid) -> Result<bool>;

    /// Number of rows (used by tests and diagnostics; default = a full `list`).
    async fn count(&self) -> Result<usize> {
        Ok(self.list(None, 0).await?.len())
    }
}

/// One store per declared entity — the injection point a host implements.
///
/// It receives the entity's name and plural (what a store needs to name a table, a
/// collection or a graph label) and never the descriptor: a store must not be able to
/// re-read, let alone reinterpret, the schema it is serving.
pub trait RowStoreFactory: Send + Sync {
    fn for_entity(&self, name: &str, plural: &str) -> Arc<dyn RowStore>;
}

/// The default: rows in memory, one map per entity. Behaviour of every declarative module
/// before this seam existed — nothing survives a restart, and that is now a stated choice
/// instead of a missing feature.
#[derive(Default)]
pub struct MemoryRowStore {
    rows: RwLock<IndexMap<Uuid, Value>>,
}

impl MemoryRowStore {
    pub fn new() -> Self {
        Self::default()
    }
}

/// A factory of in-memory stores, one per entity.
#[derive(Default)]
pub struct MemoryRowStores;

impl RowStoreFactory for MemoryRowStores {
    fn for_entity(&self, _name: &str, _plural: &str) -> Arc<dyn RowStore> {
        Arc::new(MemoryRowStore::new())
    }
}

#[async_trait]
impl RowStore for MemoryRowStore {
    async fn get(&self, id: &Uuid) -> Result<Option<Value>> {
        Ok(self.rows.read().await.get(id).cloned())
    }

    async fn list(&self, limit: Option<usize>, offset: usize) -> Result<Vec<Value>> {
        let rows = self.rows.read().await;
        let limit = limit.unwrap_or(usize::MAX);
        Ok(rows.values().skip(offset).take(limit).cloned().collect())
    }

    async fn insert(&self, id: Uuid, row: Value) -> Result<()> {
        self.rows.write().await.insert(id, row);
        Ok(())
    }

    async fn patch(&self, id: &Uuid, patch: &Value) -> Result<Option<Value>> {
        let mut rows = self.rows.write().await;
        let Some(row) = rows.get_mut(id) else {
            return Ok(None);
        };
        if let (Value::Object(target), Value::Object(patch)) = (&mut *row, patch) {
            for (key, value) in patch {
                target.insert(key.clone(), value.clone());
            }
        }
        Ok(Some(row.clone()))
    }

    async fn remove(&self, id: &Uuid) -> Result<bool> {
        Ok(self.rows.write().await.shift_remove(id).is_some())
    }

    async fn count(&self) -> Result<usize> {
        Ok(self.rows.read().await.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(name: &str) -> Value {
        json!({ "id": Uuid::new_v4().to_string(), "name": name })
    }

    #[tokio::test]
    async fn a_stored_row_comes_back_by_id() {
        let store = MemoryRowStore::new();
        let id = Uuid::new_v4();
        store.insert(id, row("Alix")).await.unwrap();
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got["name"], "Alix");
        assert!(store.get(&Uuid::new_v4()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_keeps_insertion_order_and_honours_the_window() {
        let store = MemoryRowStore::new();
        for name in ["a", "b", "c", "d"] {
            store.insert(Uuid::new_v4(), row(name)).await.unwrap();
        }
        let names = |v: Vec<Value>| {
            v.iter()
                .map(|r| r["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(store.list(None, 0).await.unwrap()),
            ["a", "b", "c", "d"]
        );
        assert_eq!(names(store.list(Some(2), 1).await.unwrap()), ["b", "c"]);
        assert_eq!(names(store.list(Some(10), 3).await.unwrap()), ["d"]);
    }

    #[tokio::test]
    async fn patch_merges_without_touching_the_other_keys() {
        let store = MemoryRowStore::new();
        let id = Uuid::new_v4();
        store
            .insert(id, json!({ "name": "Alix", "city": "Lyon" }))
            .await
            .unwrap();
        let merged = store
            .patch(&id, &json!({ "city": "Paris", "updated_at": "now" }))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(merged["name"], "Alix", "an untouched key must survive");
        assert_eq!(merged["city"], "Paris");
        assert_eq!(merged["updated_at"], "now");
        // and the store really holds the merged row, not just the answer
        assert_eq!(store.get(&id).await.unwrap().unwrap()["city"], "Paris");
    }

    #[tokio::test]
    async fn patching_an_absent_row_writes_nothing() {
        let store = MemoryRowStore::new();
        assert!(
            store
                .patch(&Uuid::new_v4(), &json!({ "city": "Paris" }))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn remove_reports_whether_the_row_was_there() {
        let store = MemoryRowStore::new();
        let id = Uuid::new_v4();
        store.insert(id, row("Alix")).await.unwrap();
        assert!(store.remove(&id).await.unwrap());
        assert!(
            !store.remove(&id).await.unwrap(),
            "second remove changes nothing"
        );
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_memory_factory_gives_one_independent_store_per_entity() {
        let factory = MemoryRowStores;
        let a = factory.for_entity("customer", "customers");
        let b = factory.for_entity("order", "orders");
        a.insert(Uuid::new_v4(), row("Alix")).await.unwrap();
        assert_eq!(a.count().await.unwrap(), 1);
        assert_eq!(b.count().await.unwrap(), 0, "entities must not share rows");
    }
}
