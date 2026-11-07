//! Rows of a declarative entity, stored as **nodes of an `ObrainStore`**.
//!
//! This is the missing half announced by `json_entity.rs` since the declarative adapter
//! exists: *"Durable storage of declarative rows is a follow-up"*. The mapping is the one
//! `storage::obrain` already uses for compiled entities, so both kinds of entity read the
//! same way in the graph:
//!
//! | node | property |
//! |---|---|
//! | label | the **entity name** (`customer`) — `MATCH (c:customer)` works |
//! | `_uuid` | the row id, as a string |
//! | `_data` | the whole row, serialized JSON — set **last**, so a creation interrupted
//!   half-way leaves a node this store ignores instead of serving a truncated row |
//!
//! # No index to rebuild, and an index that cannot go stale
//!
//! `ObrainStore`'s uuid indexes do not survive `open()` and must be rebuilt by hand. Rather
//! than export that trap, this store keeps its own `Uuid -> NodeId` map, filled by scanning
//! the label on first use — and it is safe **because of a guarantee the engine now gives**:
//! one writable `ObrainDB` per base and per process (obrain-db `dcba1fab`,
//! `OBRAIN-X001`). No other process can write rows behind our back, so a map built once and
//! maintained by our own mutations cannot go stale.
//!
//! The limit of that reasoning, stated instead of assumed: **one live instance per
//! (database, entity)**. Two instances of the same label would each cache their own map and
//! could disagree until they re-scan. `ObrainRowStores` therefore creates exactly one store
//! per entity, and the loader drops a service (and its store) when the module is unloaded —
//! so a reload starts from a fresh scan, which is also what makes it see rows written
//! before the reload.
//!
//! # Durability
//!
//! Every mutation ends with `commit_wal()`, so a row is on disk once the call returns. The
//! price is measured, not assumed: one commit is ~35 ms median (~45 ms p95) on APFS with
//! `F_FULLFSYNC`, while a statement costs ~5 ms. A caller that writes many rows should group
//! them; this store cannot decide that for it, so it commits per mutation — the safe default.

use super::rows::RowStore;
use crate::storage::obrain::ObrainStore;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use obrain_common::types::{NodeId, Value as ObrainValue};
use obrain_core::graph::GraphStoreMut;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Rows of one entity, as nodes labelled with the entity name.
pub struct ObrainRowStore {
    store: Arc<ObrainStore>,
    label: String,
    /// `Uuid -> NodeId`, built on first use (see the module docs for why it cannot stale).
    index: Mutex<Option<HashMap<Uuid, NodeId>>>,
    /// Serializes read-modify-write in this process (one `patch` at a time).
    lock: Mutex<()>,
}

impl std::fmt::Debug for ObrainRowStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObrainRowStore")
            .field("label", &self.label)
            .finish()
    }
}

impl ObrainRowStore {
    pub fn new(store: Arc<ObrainStore>, entity: impl Into<String>) -> Self {
        Self {
            store,
            label: entity.into(),
            index: Mutex::new(None),
            lock: Mutex::new(()),
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    fn graph(&self) -> Arc<dyn GraphStoreMut> {
        self.store.db().graph_store_mut()
    }

    fn guard(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.lock
            .lock()
            .map_err(|_| anyhow!("obrain row store: lock poisoned"))
    }

    /// The `Uuid -> NodeId` map, built once by scanning the label.
    ///
    /// A node without `_uuid` or `_data` is skipped, never served: it is the trace of an
    /// interrupted creation (`_data` is written last).
    fn nodes(&self) -> Result<std::sync::MutexGuard<'_, Option<HashMap<Uuid, NodeId>>>> {
        let mut slot = self
            .index
            .lock()
            .map_err(|_| anyhow!("obrain row store: index lock poisoned"))?;
        if slot.is_none() {
            let graph = self.graph();
            let mut map = HashMap::new();
            for id in graph.nodes_by_label(&self.label) {
                let Some(node) = graph.get_node(id) else {
                    continue;
                };
                let uuid = node
                    .get_property("_uuid")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());
                let has_data = node
                    .get_property("_data")
                    .and_then(|v| v.as_str())
                    .is_some();
                if let (Some(uuid), true) = (uuid, has_data) {
                    map.insert(uuid, id);
                }
            }
            *slot = Some(map);
        }
        Ok(slot)
    }

    fn node_id(&self, id: &Uuid) -> Result<Option<NodeId>> {
        Ok(self.nodes()?.as_ref().and_then(|m| m.get(id).copied()))
    }

    fn read(&self, node_id: NodeId) -> Result<Option<Value>> {
        let graph = self.graph();
        let Some(node) = graph.get_node(node_id) else {
            return Ok(None);
        };
        let Some(raw) = node.get_property("_data").and_then(|v| v.as_str()) else {
            return Ok(None);
        };
        Ok(serde_json::from_str::<Value>(raw).ok())
    }

    fn write_data(&self, node_id: NodeId, row: &Value) -> Result<()> {
        let json = serde_json::to_string(row)?;
        self.graph()
            .set_node_property(node_id, "_data", ObrainValue::from(json.as_str()));
        Ok(())
    }

    fn commit(&self) -> Result<()> {
        self.store.commit_wal()
    }

    fn writable(&self) -> Result<()> {
        if self.store.is_read_only() {
            return Err(anyhow!("obrain row store: the database is read-only"));
        }
        self.store.check_write()
    }
}

#[async_trait]
impl RowStore for ObrainRowStore {
    async fn get(&self, id: &Uuid) -> Result<Option<Value>> {
        match self.node_id(id)? {
            Some(node_id) => self.read(node_id),
            None => Ok(None),
        }
    }

    async fn list(&self, limit: Option<usize>, offset: usize) -> Result<Vec<Value>> {
        let graph = self.graph();
        let mut nodes: Vec<NodeId> = graph.nodes_by_label(&self.label);
        // NodeId is monotonic in obrain: sorting it keeps insertion order stable across
        // calls, with no `created_at` to parse.
        nodes.sort_unstable();
        let limit = limit.unwrap_or(usize::MAX);
        let mut out = Vec::new();
        let mut skipped = 0usize;
        for node_id in nodes {
            let Some(row) = self.read(node_id)? else {
                continue;
            };
            if skipped < offset {
                skipped += 1;
                continue;
            }
            if out.len() >= limit {
                break;
            }
            out.push(row);
        }
        Ok(out)
    }

    async fn insert(&self, id: Uuid, row: Value) -> Result<()> {
        self.writable()?;
        let _g = self.guard()?;
        let graph = self.graph();
        let node_id = graph.create_node(&[self.label.as_str()]);
        let uuid = id.to_string();
        graph.set_node_property(node_id, "_uuid", ObrainValue::from(uuid.as_str()));
        // `_data` last: an interrupted creation is a node the scan ignores.
        self.write_data(node_id, &row)?;
        self.commit()?;
        if let Some(map) = self.nodes()?.as_mut() {
            map.insert(id, node_id);
        }
        Ok(())
    }

    async fn patch(&self, id: &Uuid, patch: &Value) -> Result<Option<Value>> {
        self.writable()?;
        let _g = self.guard()?;
        let Some(node_id) = self.node_id(id)? else {
            return Ok(None);
        };
        let Some(mut row) = self.read(node_id)? else {
            return Ok(None);
        };
        let (Value::Object(target), Value::Object(patch)) = (&mut row, patch) else {
            return Err(anyhow!(
                "obrain row store: patch must be an object, got {}",
                patch
            ));
        };
        for (key, value) in patch {
            target.insert(key.clone(), value.clone());
        }
        self.write_data(node_id, &row)?;
        self.commit()?;
        Ok(Some(row))
    }

    async fn remove(&self, id: &Uuid) -> Result<bool> {
        self.writable()?;
        let _g = self.guard()?;
        let Some(node_id) = self.node_id(id)? else {
            return Ok(false);
        };
        self.graph().delete_node(node_id);
        self.commit()?;
        if let Some(map) = self.nodes()?.as_mut() {
            map.remove(id);
        }
        Ok(true)
    }

    async fn count(&self) -> Result<usize> {
        Ok(self.nodes()?.as_ref().map(HashMap::len).unwrap_or(0))
    }
}

/// A factory of `obrain` row stores over **one shared** database.
///
/// The `Arc<ObrainStore>` is shared, never reopened per entity: obrain allows **one
/// writable instance per base and per process** (`OBRAIN-X001`), so a factory that opened
/// its own database per entity would panic on the second one.
pub struct ObrainRowStores {
    store: Arc<ObrainStore>,
}

impl ObrainRowStores {
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self { store }
    }
}

impl super::rows::RowStoreFactory for ObrainRowStores {
    fn for_entity(&self, name: &str, _plural: &str) -> Arc<dyn RowStore> {
        Arc::new(ObrainRowStore::new(Arc::clone(&self.store), name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("this_rows_{tag}_{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn a_row_survives_a_reopen() {
        let dir = temp_dir("reopen");
        let id = Uuid::new_v4();
        {
            let store = ObrainStore::open(&dir).unwrap();
            let rows = ObrainRowStore::new(Arc::clone(&store), "customer");
            rows.insert(id, json!({ "name": "Alix", "city": "Lyon" }))
                .await
                .unwrap();
            assert_eq!(rows.count().await.unwrap(), 1);
            // The store must be dropped before the base can be opened again: obrain
            // allows one writable instance per base and per process.
        }
        let store = ObrainStore::open(&dir).unwrap();
        let rows = ObrainRowStore::new(Arc::clone(&store), "customer");
        let row = rows
            .get(&id)
            .await
            .unwrap()
            .expect("row survives the reopen");
        assert_eq!(row["name"], "Alix");
        assert_eq!(row["city"], "Lyon");
        drop(rows);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn entities_do_not_share_their_label() {
        let dir = temp_dir("labels");
        let store = ObrainStore::open(&dir).unwrap();
        let customers = ObrainRowStore::new(Arc::clone(&store), "customer");
        let orders = ObrainRowStore::new(Arc::clone(&store), "order");
        customers
            .insert(Uuid::new_v4(), json!({ "name": "Alix" }))
            .await
            .unwrap();
        assert_eq!(customers.count().await.unwrap(), 1);
        assert_eq!(orders.count().await.unwrap(), 0);
        drop(customers);
        drop(orders);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn patch_and_remove_are_durable_too() {
        let dir = temp_dir("patch");
        let (kept, removed) = (Uuid::new_v4(), Uuid::new_v4());
        {
            let store = ObrainStore::open(&dir).unwrap();
            let rows = ObrainRowStore::new(Arc::clone(&store), "customer");
            rows.insert(kept, json!({ "name": "Alix", "city": "Lyon" }))
                .await
                .unwrap();
            rows.insert(removed, json!({ "name": "Gus" }))
                .await
                .unwrap();
            let merged = rows
                .patch(&kept, &json!({ "city": "Paris" }))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(merged["city"], "Paris");
            assert_eq!(merged["name"], "Alix");
            assert!(rows.remove(&removed).await.unwrap());
            assert!(!rows.remove(&removed).await.unwrap());
        }
        let store = ObrainStore::open(&dir).unwrap();
        let rows = ObrainRowStore::new(Arc::clone(&store), "customer");
        assert_eq!(rows.count().await.unwrap(), 1);
        let row = rows.get(&kept).await.unwrap().unwrap();
        assert_eq!(row["city"], "Paris");
        assert!(rows.get(&removed).await.unwrap().is_none());
        drop(rows);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// La chaîne complète : un descripteur → `JsonEntityService` → obrain → réouverture.
    ///
    /// C'est la promesse que `json_entity.rs` annonçait comme « a follow-up » : les
    /// lignes d'une entité DÉCLARATIVE survivent à l'arrêt du processus, avec la
    /// validation et les timestamps du service inchangés.
    #[tokio::test]
    async fn a_declarative_entity_served_from_obrain_survives_a_reopen() {
        use crate::core::module::{EntityCreator, EntityFetcher};
        use crate::descriptor::{CompiledEntity, CustomRegistry, EntityDesc, FieldDesc, FieldType};
        use crate::server::module_runtime::json_entity::JsonEntityService;

        let mut desc = EntityDesc::new("customer");
        desc.fields = vec![FieldDesc::new("name", FieldType::String)];
        let compiled = Arc::new(CompiledEntity::new("shop", desc).unwrap());

        let open = |store: Arc<ObrainStore>| {
            JsonEntityService::with_row_store(
                Arc::clone(&compiled),
                CustomRegistry::default(),
                None,
                Arc::new(ObrainRowStore::new(store, "customer")),
            )
        };

        let dir = temp_dir("service");
        let (id, created_at) = {
            let store = ObrainStore::open(&dir).unwrap();
            let svc = open(store);
            let row = svc
                .create_from_json(json!({ "name": "Alix" }))
                .await
                .unwrap();
            let id = Uuid::parse_str(row["id"].as_str().unwrap()).unwrap();
            (id, row["created_at"].clone())
        };

        let store = ObrainStore::open(&dir).unwrap();
        let svc = open(Arc::clone(&store));
        let row = svc
            .fetch_as_json(&id)
            .await
            .expect("the row must survive the reopen");
        assert_eq!(row["name"], "Alix");
        assert_eq!(row["created_at"], created_at, "timestamps are durable too");
        assert_eq!(svc.list_as_json(None, None).await.unwrap().len(), 1);

        // The service's own validation still applies on the durable backend.
        let refused = svc.create_from_json(json!({ "unknown": 1 })).await;
        assert!(refused.is_err(), "an unknown field must be refused");
        drop(svc);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_node_without_data_is_never_served() {
        let dir = temp_dir("partial");
        let store = ObrainStore::open(&dir).unwrap();
        // Simulate a creation interrupted between the node and `_data`.
        let graph = store.db().graph_store_mut();
        let orphan = graph.create_node(&["customer"]);
        graph.set_node_property(
            orphan,
            "_uuid",
            ObrainValue::from(Uuid::new_v4().to_string().as_str()),
        );
        store.commit_wal().unwrap();

        let rows = ObrainRowStore::new(Arc::clone(&store), "customer");
        assert_eq!(rows.count().await.unwrap(), 0, "no `_data`, no row");
        assert!(rows.list(None, 0).await.unwrap().is_empty());
        drop(rows);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }
}
