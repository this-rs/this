//! Obrain-backed storage implementations of DataService and LinkService
//!
//! Provides graph-oriented storage using obrain-engine's native property graph.
//! Entities are stored as graph nodes (label = entity_type, properties = JSON-serialized
//! fields), and links as graph edges (edge_type = link_type).
//!
//! # Architecture
//!
//! Both `ObrainDataService` and `ObrainLinkService` share an `Arc<ObrainStore>` which
//! holds the `ObrainDB` instance and a UUID→NodeId index. This index is necessary
//! because this.rs uses `Uuid` identifiers while obrain uses internal `NodeId` (u64).
//!
//! # Feature flag
//!
//! Gated behind `#[cfg(feature = "obrain")]`. Enable with:
//! ```toml
//! this-rs = { version = "0.0.9", features = ["obrain"] }
//! ```

use crate::core::field::FieldValue;
use crate::core::link::LinkEntity;
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use obrain_common::types::{NodeId, Value as ObrainValue};
use obrain_core::Node;
use obrain_engine::ObrainDB;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

// Re-export for advanced users
pub use obrain_common::types::NodeId as ObrainNodeId;

// ---------------------------------------------------------------------------
// ObrainStore — shared state between DataService and LinkService
// ---------------------------------------------------------------------------

/// Shared obrain database handle with UUID↔NodeId mapping.
///
/// Both `ObrainDataService` and `ObrainLinkService` hold an `Arc<ObrainStore>`
/// to share the same database and index. The UUID index is necessary because
/// this.rs identifies entities by `Uuid` while obrain-engine uses internal `NodeId`.
pub struct ObrainStore {
    /// The obrain database instance
    db: ObrainDB,
    /// UUID → NodeId index for O(1) entity lookups
    uuid_to_node: RwLock<HashMap<Uuid, NodeId>>,
    /// Link UUID → (EdgeId-placeholder: source_node, target_node, link_uuid)
    /// We store links as nodes with label `_link:{link_type}` because obrain edges
    /// require both endpoints to exist as nodes. Since LinkService is entity-agnostic
    /// (source/target may not exist yet), we use labeled nodes for links.
    link_nodes: RwLock<HashMap<Uuid, NodeId>>,
}

impl ObrainStore {
    /// Create a new in-memory obrain store
    pub fn new_in_memory() -> Arc<Self> {
        Arc::new(Self {
            db: ObrainDB::new_in_memory(),
            uuid_to_node: RwLock::new(HashMap::new()),
            link_nodes: RwLock::new(HashMap::new()),
        })
    }

    /// Create a new persistent obrain store
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Arc<Self>> {
        let db = ObrainDB::open(path).map_err(|e| anyhow!("Failed to open ObrainDB: {}", e))?;
        Ok(Arc::new(Self {
            db,
            uuid_to_node: RwLock::new(HashMap::new()),
            link_nodes: RwLock::new(HashMap::new()),
        }))
    }

    /// Create a store wrapping an existing ObrainDB instance
    pub fn from_db(db: ObrainDB) -> Arc<Self> {
        Arc::new(Self {
            db,
            uuid_to_node: RwLock::new(HashMap::new()),
            link_nodes: RwLock::new(HashMap::new()),
        })
    }

    /// Access the underlying ObrainDB
    pub fn db(&self) -> &ObrainDB {
        &self.db
    }

    /// Switch the session to a named graph (for multi-tenant isolation)
    pub fn use_graph(&self, name: &str) {
        let session = self.db.session();
        session.use_graph(name);
    }
}

// ---------------------------------------------------------------------------
// ObrainDataService
// ---------------------------------------------------------------------------

/// Obrain-backed data service using the native property graph engine.
///
/// Entities are stored as graph nodes with:
/// - **Label**: `entity_type()` (e.g., `"user"`, `"product"`)
/// - **Property `_uuid`**: The entity's UUID as a string
/// - **Property `_data`**: The full entity serialized as JSON string
/// - **Indexed properties**: Each field from `indexed_fields()` stored individually
///   for efficient `search()` queries via `find_nodes_by_property`
///
/// # Type bounds
///
/// Requires `T: Data + Serialize + DeserializeOwned` because entities must be
/// serialized to/from JSON for storage in node properties. This is the same
/// pattern used by `DynamoDBDataService`.
///
/// # Example
///
/// ```rust,ignore
/// use this::storage::obrain::{ObrainStore, ObrainDataService};
///
/// let store = ObrainStore::new_in_memory();
/// let service = ObrainDataService::<MyEntity>::new(store);
/// let entity = service.create(my_entity).await?;
/// ```
pub struct ObrainDataService<T: Data> {
    store: Arc<ObrainStore>,
    _marker: PhantomData<T>,
}

impl<T: Data> ObrainDataService<T> {
    /// Create a new obrain data service backed by the given store
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self {
            store,
            _marker: PhantomData,
        }
    }
}

impl<T: Data> Clone for ObrainDataService<T> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            _marker: PhantomData,
        }
    }
}

/// Convert a FieldValue to an obrain Value for property storage
fn field_value_to_obrain(fv: &FieldValue) -> ObrainValue {
    match fv {
        FieldValue::String(s) => ObrainValue::from(s.as_str()),
        FieldValue::Integer(i) => ObrainValue::from(*i),
        FieldValue::Float(f) => ObrainValue::from(*f),
        FieldValue::Boolean(b) => ObrainValue::from(*b),
        FieldValue::Uuid(u) => ObrainValue::from(u.to_string().as_str()),
        FieldValue::DateTime(dt) => ObrainValue::from(dt.to_rfc3339().as_str()),
        FieldValue::Null => ObrainValue::Null,
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> DataService<T> for ObrainDataService<T> {
    async fn create(&self, entity: T) -> Result<T> {
        let session = self.store.db.session();
        let entity_type = entity.entity_type().to_string();
        let uuid = entity.id();
        let uuid_str = uuid.to_string();

        // Serialize entity to JSON for _data property
        let json_data =
            serde_json::to_string(&entity).map_err(|e| anyhow!("Serialize failed: {}", e))?;

        // Build properties: _uuid + _data + indexed fields
        let mut props: Vec<(&str, ObrainValue)> = Vec::new();
        // We need to own the strings for the property values
        let uuid_val = ObrainValue::from(uuid_str.as_str());
        let data_val = ObrainValue::from(json_data.as_str());
        props.push(("_uuid", uuid_val));
        props.push(("_data", data_val));

        // Store indexed fields as individual properties for search
        let indexed = T::indexed_fields();
        // Collect field values before building props to avoid borrow issues
        let field_vals: Vec<(String, ObrainValue)> = indexed
            .iter()
            .filter_map(|&field| {
                entity
                    .field_value(field)
                    .map(|fv| (format!("f_{}", field), field_value_to_obrain(&fv)))
            })
            .collect();

        // Create node with label and properties
        // We need to build the full props list with owned references
        let label = entity_type.as_str();
        let node_id = {
            // Build property pairs referencing our collected data
            let mut all_props: Vec<(&str, ObrainValue)> = Vec::with_capacity(2 + field_vals.len());
            all_props.push(("_uuid", ObrainValue::from(uuid_str.as_str())));
            all_props.push(("_data", ObrainValue::from(json_data.as_str())));
            for (key, val) in &field_vals {
                all_props.push((key.as_str(), val.clone()));
            }
            session.create_node_with_props(&[label], all_props)
        };

        // Update UUID index
        self.store
            .uuid_to_node
            .write()
            .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?
            .insert(uuid, node_id);

        Ok(entity)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let node_id = {
            let index = self
                .store
                .uuid_to_node
                .read()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;
            match index.get(id) {
                Some(&nid) => nid,
                None => return Ok(None),
            }
        };

        let session = self.store.db.session();
        let node = match session.get_node(node_id) {
            Some(n) => n,
            None => return Ok(None),
        };

        // Deserialize from _data property
        let data_val = node
            .get_property("_data")
            .ok_or_else(|| anyhow!("Node {} missing _data property", id))?;

        let json_str = data_val
            .as_str()
            .ok_or_else(|| anyhow!("_data property is not a string for node {}", id))?;

        let entity: T =
            serde_json::from_str(json_str).map_err(|e| anyhow!("Deserialize failed: {}", e))?;

        Ok(Some(entity))
    }

    async fn list(&self) -> Result<Vec<T>> {
        let session = self.store.db.session();

        // Query all nodes by label (entity_type)
        // We need a dummy instance to get entity_type, but Data doesn't provide that statically.
        // Instead, iterate all nodes in our UUID index that match.
        let index = self
            .store
            .uuid_to_node
            .read()
            .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;

        let mut entities = Vec::with_capacity(index.len());
        for (&_uuid, &node_id) in index.iter() {
            if let Some(node) = session.get_node(node_id) {
                // Check label matches T's entity_type — we can't get entity_type statically
                // from T, so we deserialize all nodes in our index. Since each
                // ObrainDataService<T> only inserts nodes of type T, this is safe.
                if let Some(data_val) = node.get_property("_data")
                    && let Some(json_str) = data_val.as_str()
                    && let Ok(entity) = serde_json::from_str::<T>(json_str)
                {
                    entities.push(entity);
                }
            }
        }

        Ok(entities)
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        let node_id = {
            let index = self
                .store
                .uuid_to_node
                .read()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;
            *index
                .get(id)
                .ok_or_else(|| anyhow!("Node not found: {}", id))?
        };

        let session = self.store.db.session();

        // Verify node still exists
        if session.get_node(node_id).is_none() {
            return Err(anyhow!("Node not found in graph: {}", id));
        }

        // Update _data property via the underlying store (GraphStoreMut)
        let json_data =
            serde_json::to_string(&entity).map_err(|e| anyhow!("Serialize failed: {}", e))?;

        let store = self.store.db.store();
        store.set_node_property(node_id, "_data", ObrainValue::from(json_data.as_str()));

        // Update indexed field properties
        let indexed = T::indexed_fields();
        for &field in indexed {
            let prop_key = format!("f_{}", field);
            if let Some(fv) = entity.field_value(field) {
                store.set_node_property(node_id, &prop_key, field_value_to_obrain(&fv));
            }
        }

        Ok(entity)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let node_id = {
            let mut index = self
                .store
                .uuid_to_node
                .write()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;
            match index.remove(id) {
                Some(nid) => nid,
                None => return Ok(()), // Already deleted or never existed
            }
        };

        let store = self.store.db.store();
        // DETACH DELETE — removes edges connected to this node too
        store.delete_node_edges(node_id);
        store.delete_node(node_id);

        Ok(())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        let store = self.store.db.store();
        let session = self.store.db.session();
        let prop_key = format!("f_{}", field);

        // Use obrain's native property search via GraphStore
        let search_value = ObrainValue::from(value);
        let matching_nodes = store.find_nodes_by_property(&prop_key, &search_value);

        let mut results = Vec::new();
        for node_id in matching_nodes {
            if let Some(node) = session.get_node(node_id)
                && let Some(data_val) = node.get_property("_data")
                && let Some(json_str) = data_val.as_str()
                && let Ok(entity) = serde_json::from_str::<T>(json_str)
            {
                results.push(entity);
            }
        }

        // Also try matching FieldValue variants that convert to string differently
        // (e.g., UUID.to_string(), DateTime.to_rfc3339())
        if results.is_empty() {
            // Fall back to full scan with FieldValue comparison
            let index = self
                .store
                .uuid_to_node
                .read()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;

            for &node_id in index.values() {
                if let Some(node) = session.get_node(node_id)
                    && let Some(data_val) = node.get_property("_data")
                    && let Some(json_str) = data_val.as_str()
                    && let Ok(entity) = serde_json::from_str::<T>(json_str)
                    && entity.field_value(field).is_some_and(|fv| match &fv {
                        FieldValue::String(s) => s == value,
                        FieldValue::Integer(i) => i.to_string() == value,
                        FieldValue::Float(f) => f.to_string() == value,
                        FieldValue::Boolean(b) => b.to_string() == value,
                        FieldValue::Uuid(u) => u.to_string() == value,
                        FieldValue::DateTime(dt) => dt.to_rfc3339() == value,
                        FieldValue::Null => false,
                    })
                {
                    results.push(entity);
                }
            }
        }

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// ObrainLinkService
// ---------------------------------------------------------------------------

/// Obrain-backed link service using graph nodes with link labels.
///
/// Links are stored as graph nodes (not edges) with label `_link:{link_type}`
/// because `LinkService` is entity-agnostic — the source and target entities
/// may be managed by different `ObrainDataService` instances or may not exist
/// yet. Storing links as nodes avoids the constraint that edge endpoints must
/// exist.
///
/// Each link node has properties:
/// - `_uuid`: Link UUID as string
/// - `_data`: Full `LinkEntity` serialized as JSON
/// - `_source_id`: Source entity UUID as string
/// - `_target_id`: Target entity UUID as string
/// - `_link_type`: Link type string
///
/// When both endpoints exist as entity nodes, graph edges are also created
/// to enable native traversal queries (opportunistic edge creation).
///
/// # Example
///
/// ```rust,ignore
/// use this::storage::obrain::{ObrainStore, ObrainLinkService};
///
/// let store = ObrainStore::new_in_memory();
/// let service = ObrainLinkService::new(store);
/// let link = service.create(link_entity).await?;
/// ```
#[derive(Clone)]
pub struct ObrainLinkService {
    store: Arc<ObrainStore>,
}

impl ObrainLinkService {
    /// Create a new obrain link service backed by the given store
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self { store }
    }

    /// Deserialize a LinkEntity from a node's _data property
    fn link_from_node(&self, node: &Node) -> Option<LinkEntity> {
        let data_val = node.get_property("_data")?;
        let json_str = data_val.as_str()?;
        serde_json::from_str(json_str).ok()
    }

    /// Try to create a graph edge between source and target entity nodes
    /// for native traversal. This is opportunistic — if endpoints don't exist
    /// as entity nodes, no edge is created.
    fn try_create_edge(&self, link: &LinkEntity) {
        let index = match self.store.uuid_to_node.read() {
            Ok(idx) => idx,
            Err(_) => return,
        };

        let source_node = index.get(&link.source_id);
        let target_node = index.get(&link.target_id);

        if let (Some(&src), Some(&tgt)) = (source_node, target_node) {
            let session = self.store.db.session();
            session.create_edge(src, tgt, &link.link_type);
        }
    }
}

#[async_trait]
impl LinkService for ObrainLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let session = self.store.db.session();
        let uuid_str = link.id.to_string();
        let source_str = link.source_id.to_string();
        let target_str = link.target_id.to_string();

        let json_data =
            serde_json::to_string(&link).map_err(|e| anyhow!("Serialize link failed: {}", e))?;

        let label = format!("_link:{}", link.link_type);

        let node_id = session.create_node_with_props(
            &[label.as_str()],
            vec![
                ("_uuid", ObrainValue::from(uuid_str.as_str())),
                ("_data", ObrainValue::from(json_data.as_str())),
                ("_source_id", ObrainValue::from(source_str.as_str())),
                ("_target_id", ObrainValue::from(target_str.as_str())),
                ("_link_type", ObrainValue::from(link.link_type.as_str())),
            ],
        );

        // Update link index
        self.store
            .link_nodes
            .write()
            .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?
            .insert(link.id, node_id);

        // Opportunistically create graph edge for traversal
        self.try_create_edge(&link);

        Ok(link)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let node_id = {
            let index = self
                .store
                .link_nodes
                .read()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;
            match index.get(id) {
                Some(&nid) => nid,
                None => return Ok(None),
            }
        };

        let session = self.store.db.session();
        let node = match session.get_node(node_id) {
            Some(n) => n,
            None => return Ok(None),
        };

        Ok(self.link_from_node(&node))
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let session = self.store.db.session();
        let index = self
            .store
            .link_nodes
            .read()
            .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;

        let mut links = Vec::with_capacity(index.len());
        for &node_id in index.values() {
            if let Some(node) = session.get_node(node_id)
                && let Some(link) = self.link_from_node(&node)
            {
                links.push(link);
            }
        }

        Ok(links)
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let store = self.store.db.store();
        let session = self.store.db.session();
        let source_str = source_id.to_string();
        let search_val = ObrainValue::from(source_str.as_str());

        // Use property search for _source_id via GraphStore
        let matching = store.find_nodes_by_property("_source_id", &search_val);

        let mut results = Vec::new();
        for node_id in matching {
            if let Some(node) = session.get_node(node_id)
                && let Some(link) = self.link_from_node(&node)
                && link_type.is_none_or(|lt| link.link_type == lt)
            {
                results.push(link);
            }
        }

        Ok(results)
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let store = self.store.db.store();
        let session = self.store.db.session();
        let target_str = target_id.to_string();
        let search_val = ObrainValue::from(target_str.as_str());

        // Use property search for _target_id via GraphStore
        let matching = store.find_nodes_by_property("_target_id", &search_val);

        let mut results = Vec::new();
        for node_id in matching {
            if let Some(node) = session.get_node(node_id)
                && let Some(link) = self.link_from_node(&node)
                && link_type.is_none_or(|lt| link.link_type == lt)
            {
                results.push(link);
            }
        }

        Ok(results)
    }

    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        let node_id = {
            let index = self
                .store
                .link_nodes
                .read()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;
            *index
                .get(id)
                .ok_or_else(|| anyhow!("Edge not found: {}", id))?
        };

        let session = self.store.db.session();
        let store = self.store.db.store();

        if session.get_node(node_id).is_none() {
            return Err(anyhow!("Link node not found in graph: {}", id));
        }

        // Update properties via GraphStoreMut
        let json_data =
            serde_json::to_string(&link).map_err(|e| anyhow!("Serialize link failed: {}", e))?;
        let source_str = link.source_id.to_string();
        let target_str = link.target_id.to_string();

        store.set_node_property(node_id, "_data", ObrainValue::from(json_data.as_str()));
        store.set_node_property(
            node_id,
            "_source_id",
            ObrainValue::from(source_str.as_str()),
        );
        store.set_node_property(
            node_id,
            "_target_id",
            ObrainValue::from(target_str.as_str()),
        );
        store.set_node_property(
            node_id,
            "_link_type",
            ObrainValue::from(link.link_type.as_str()),
        );

        Ok(link)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let node_id = {
            let mut index = self
                .store
                .link_nodes
                .write()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;
            match index.remove(id) {
                Some(nid) => nid,
                None => return Ok(()),
            }
        };

        let store = self.store.db.store();
        store.delete_node(node_id);

        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let entity_str = entity_id.to_string();
        let session = self.store.db.session();

        // Find all links where entity is source or target
        let mut to_remove = Vec::new();
        {
            let index = self
                .store
                .link_nodes
                .read()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;

            for (&link_uuid, &node_id) in index.iter() {
                if let Some(node) = session.get_node(node_id) {
                    let is_source = node
                        .get_property("_source_id")
                        .and_then(|v| v.as_str().map(|s| s == entity_str))
                        .unwrap_or(false);
                    let is_target = node
                        .get_property("_target_id")
                        .and_then(|v| v.as_str().map(|s| s == entity_str))
                        .unwrap_or(false);

                    if is_source || is_target {
                        to_remove.push((link_uuid, node_id));
                    }
                }
            }
        }

        // Remove matching links
        if !to_remove.is_empty() {
            let store = self.store.db.store();
            let mut index = self
                .store
                .link_nodes
                .write()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;

            for (link_uuid, node_id) in to_remove {
                index.remove(&link_uuid);
                store.delete_node(node_id);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity::Entity;
    use crate::core::field::FieldValue;
    use chrono::{DateTime, Utc};

    // -----------------------------------------------------------------------
    // Test entity for ObrainDataService tests
    // -----------------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
    struct TestDataEntity {
        id: Uuid,
        entity_name: String,
        status: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    impl TestDataEntity {
        fn new(name: &str) -> Self {
            let now = Utc::now();
            Self {
                id: Uuid::new_v4(),
                entity_name: name.to_string(),
                status: "active".to_string(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    impl Entity for TestDataEntity {
        type Service = ();

        fn resource_name() -> &'static str {
            "test_data_entities"
        }

        fn resource_name_singular() -> &'static str {
            "test_data_entity"
        }

        fn service_from_host(
            _: &std::sync::Arc<dyn std::any::Any + Send + Sync>,
        ) -> anyhow::Result<std::sync::Arc<Self::Service>> {
            Ok(std::sync::Arc::new(()))
        }

        fn id(&self) -> Uuid {
            self.id
        }

        fn entity_type(&self) -> &str {
            "test_data"
        }

        fn created_at(&self) -> DateTime<Utc> {
            self.created_at
        }

        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }

        fn deleted_at(&self) -> Option<DateTime<Utc>> {
            None
        }

        fn status(&self) -> &str {
            &self.status
        }
    }

    impl crate::core::Data for TestDataEntity {
        fn name(&self) -> &str {
            &self.entity_name
        }

        fn indexed_fields() -> &'static [&'static str] {
            &["entity_name", "status"]
        }

        fn field_value(&self, field: &str) -> Option<FieldValue> {
            match field {
                "entity_name" => Some(FieldValue::String(self.entity_name.clone())),
                "status" => Some(FieldValue::String(self.status.clone())),
                _ => None,
            }
        }
    }

    /// Helper to create a fresh in-memory store for each test
    fn test_store() -> Arc<ObrainStore> {
        ObrainStore::new_in_memory()
    }

    // -----------------------------------------------------------------------
    // ObrainDataService CRUD tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_data_create_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Alice");

        let created = service.create(entity.clone()).await.unwrap();
        assert_eq!(created.id, entity.id);
        assert_eq!(created.entity_name, "Alice");
    }

    #[tokio::test]
    async fn test_data_get_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Bob");

        service.create(entity.clone()).await.unwrap();

        let retrieved = service.get(&entity.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().entity_name, "Bob");
    }

    #[tokio::test]
    async fn test_data_get_nonexistent() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        let retrieved = service.get(&Uuid::new_v4()).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_data_list_entities() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();
        service.create(TestDataEntity::new("Bob")).await.unwrap();
        service
            .create(TestDataEntity::new("Charlie"))
            .await
            .unwrap();

        let all = service.list().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_data_list_empty() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        let all = service.list().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_data_update_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let mut entity = TestDataEntity::new("Alice");

        service.create(entity.clone()).await.unwrap();

        entity.entity_name = "Alice Updated".to_string();
        let updated = service.update(&entity.id, entity.clone()).await.unwrap();

        assert_eq!(updated.entity_name, "Alice Updated");

        // Verify persisted
        let retrieved = service.get(&entity.id).await.unwrap().unwrap();
        assert_eq!(retrieved.entity_name, "Alice Updated");
    }

    #[tokio::test]
    async fn test_data_update_nonexistent() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Ghost");
        let id = entity.id;

        let result = service.update(&id, entity).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_data_delete_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Alice");

        service.create(entity.clone()).await.unwrap();
        assert!(service.get(&entity.id).await.unwrap().is_some());

        service.delete(&entity.id).await.unwrap();
        assert!(service.get(&entity.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_data_delete_nonexistent() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        // Deleting a nonexistent entity should succeed silently
        let result = service.delete(&Uuid::new_v4()).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // ObrainDataService search tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_data_search_by_indexed_field() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();
        service.create(TestDataEntity::new("Bob")).await.unwrap();
        service.create(TestDataEntity::new("Alice")).await.unwrap();

        let results = service.search("entity_name", "Alice").await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.entity_name == "Alice"));
    }

    #[tokio::test]
    async fn test_data_search_no_results() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        let results = service.search("entity_name", "Zara").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_data_search_by_status() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        let mut inactive = TestDataEntity::new("Inactive");
        inactive.status = "inactive".to_string();

        service.create(TestDataEntity::new("Active")).await.unwrap();
        service.create(inactive).await.unwrap();

        let results = service.search("status", "inactive").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity_name, "Inactive");
    }

    #[tokio::test]
    async fn test_data_search_unknown_field() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Unknown field returns no results (not indexed, not in FieldValue)
        let results = service
            .search("nonexistent_field", "anything")
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_data_clone_shares_state() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let cloned = service.clone();

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Clone shares the same backing store
        let all = cloned.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Extended test entity for FieldValue variant coverage in search
    // -----------------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
    struct ExtendedTestEntity {
        id: Uuid,
        entity_name: String,
        status: String,
        ref_id: Uuid,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    impl ExtendedTestEntity {
        fn new(name: &str, ref_id: Uuid) -> Self {
            let now = Utc::now();
            Self {
                id: Uuid::new_v4(),
                entity_name: name.to_string(),
                status: "active".to_string(),
                ref_id,
                created_at: now,
                updated_at: now,
            }
        }
    }

    impl Entity for ExtendedTestEntity {
        type Service = ();

        fn resource_name() -> &'static str {
            "extended_test_entities"
        }

        fn resource_name_singular() -> &'static str {
            "extended_test_entity"
        }

        fn service_from_host(
            _: &std::sync::Arc<dyn std::any::Any + Send + Sync>,
        ) -> anyhow::Result<std::sync::Arc<Self::Service>> {
            Ok(std::sync::Arc::new(()))
        }

        fn id(&self) -> Uuid {
            self.id
        }

        fn entity_type(&self) -> &str {
            "extended_test"
        }

        fn created_at(&self) -> DateTime<Utc> {
            self.created_at
        }

        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }

        fn deleted_at(&self) -> Option<DateTime<Utc>> {
            None
        }

        fn status(&self) -> &str {
            &self.status
        }
    }

    impl crate::core::Data for ExtendedTestEntity {
        fn name(&self) -> &str {
            &self.entity_name
        }

        fn indexed_fields() -> &'static [&'static str] {
            &["entity_name", "status", "ref_id", "created_at"]
        }

        fn field_value(&self, field: &str) -> Option<FieldValue> {
            match field {
                "entity_name" => Some(FieldValue::String(self.entity_name.clone())),
                "status" => Some(FieldValue::String(self.status.clone())),
                "ref_id" => Some(FieldValue::Uuid(self.ref_id)),
                "created_at" => Some(FieldValue::DateTime(self.created_at)),
                _ => None,
            }
        }
    }

    #[tokio::test]
    async fn test_data_search_by_uuid_field() {
        let store = test_store();
        let service = ObrainDataService::<ExtendedTestEntity>::new(store);
        let target_ref = Uuid::new_v4();
        let other_ref = Uuid::new_v4();

        service
            .create(ExtendedTestEntity::new("Alpha", target_ref))
            .await
            .expect("create Alpha should succeed");
        service
            .create(ExtendedTestEntity::new("Beta", other_ref))
            .await
            .expect("create Beta should succeed");
        service
            .create(ExtendedTestEntity::new("Gamma", target_ref))
            .await
            .expect("create Gamma should succeed");

        let results = service
            .search("ref_id", &target_ref.to_string())
            .await
            .expect("search by ref_id should succeed");
        assert_eq!(
            results.len(),
            2,
            "should find 2 entities with matching ref_id"
        );
        assert!(results.iter().all(|e| e.ref_id == target_ref));
    }

    #[tokio::test]
    async fn test_data_search_by_datetime_field() {
        let store = test_store();
        let service = ObrainDataService::<ExtendedTestEntity>::new(store);
        let entity = ExtendedTestEntity::new("Timed", Uuid::new_v4());
        let created_rfc3339 = entity.created_at.to_rfc3339();

        service.create(entity).await.expect("create should succeed");

        let results = service
            .search("created_at", &created_rfc3339)
            .await
            .expect("search by created_at should succeed");
        assert_eq!(
            results.len(),
            1,
            "should find entity by its created_at timestamp"
        );
        assert_eq!(results[0].entity_name, "Timed");
    }

    // -----------------------------------------------------------------------
    // ObrainLinkService tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = LinkEntity::new("owner", user_id, car_id, None);

        let created = service.create(link.clone()).await.unwrap();

        assert_eq!(created.link_type, "owner");
        assert_eq!(created.source_id, user_id);
        assert_eq!(created.target_id, car_id);
    }

    #[tokio::test]
    async fn test_get_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, link.id);
    }

    #[tokio::test]
    async fn test_get_link_nonexistent() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        let retrieved = service.get(&Uuid::new_v4()).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_list_links() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        let link1 = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);
        let link2 = LinkEntity::new("driver", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link1).await.unwrap();
        service.create(link2).await.unwrap();

        let links = service.list().await.unwrap();
        assert_eq!(links.len(), 2);
    }

    #[tokio::test]
    async fn test_find_by_source() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        // User owns car1
        service
            .create(LinkEntity::new("owner", user_id, car1_id, None))
            .await
            .unwrap();

        // User drives car2
        service
            .create(LinkEntity::new("driver", user_id, car2_id, None))
            .await
            .unwrap();

        // Find all links from user
        let links = service.find_by_source(&user_id, None, None).await.unwrap();
        assert_eq!(links.len(), 2);

        // Find only owner links
        let owner_links = service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .unwrap();
        assert_eq!(owner_links.len(), 1);
        assert_eq!(owner_links[0].link_type, "owner");
    }

    #[tokio::test]
    async fn test_find_by_target() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user1_id = Uuid::new_v4();
        let user2_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        // User1 owns car
        service
            .create(LinkEntity::new("owner", user1_id, car_id, None))
            .await
            .unwrap();

        // User2 drives car
        service
            .create(LinkEntity::new("driver", user2_id, car_id, None))
            .await
            .unwrap();

        // Find all links to car
        let links = service.find_by_target(&car_id, None, None).await.unwrap();
        assert_eq!(links.len(), 2);

        // Find only driver links
        let driver_links = service
            .find_by_target(&car_id, Some("driver"), None)
            .await
            .unwrap();
        assert_eq!(driver_links.len(), 1);
        assert_eq!(driver_links[0].link_type, "driver");
    }

    #[tokio::test]
    async fn test_update_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let company_id = Uuid::new_v4();

        let mut link = LinkEntity::new(
            "worker",
            user_id,
            company_id,
            Some(serde_json::json!({"role": "Developer"})),
        );

        service.create(link.clone()).await.unwrap();

        // Update metadata
        link.metadata = Some(serde_json::json!({"role": "Senior Developer"}));
        link.touch();

        let updated = service.update(&link.id, link.clone()).await.unwrap();
        assert_eq!(
            updated.metadata,
            Some(serde_json::json!({"role": "Senior Developer"}))
        );
    }

    #[tokio::test]
    async fn test_update_link_nonexistent() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);
        let id = link.id;

        let result = service.update(&id, link).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_some());

        service.delete(&link.id).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_delete_link_nonexistent() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        // Deleting a nonexistent edge should succeed silently
        let result = service.delete(&Uuid::new_v4()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_by_entity() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        service
            .create(LinkEntity::new("owner", user_id, car1_id, None))
            .await
            .unwrap();
        service
            .create(LinkEntity::new("driver", user_id, car2_id, None))
            .await
            .unwrap();
        service
            .create(LinkEntity::new("owner", Uuid::new_v4(), car1_id, None))
            .await
            .unwrap();

        let links = service.list().await.unwrap();
        assert_eq!(links.len(), 3);

        // Delete all links involving user_id
        service.delete_by_entity(&user_id).await.unwrap();

        let remaining = service.list().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_ne!(remaining[0].source_id, user_id);
        assert_ne!(remaining[0].target_id, user_id);
    }

    // -----------------------------------------------------------------------
    // Shared store tests (DataService + LinkService work together)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_shared_store_data_and_links() {
        let store = test_store();
        let data_svc = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));
        let link_svc = ObrainLinkService::new(Arc::clone(&store));

        let alice = TestDataEntity::new("Alice");
        let bob = TestDataEntity::new("Bob");

        data_svc.create(alice.clone()).await.unwrap();
        data_svc.create(bob.clone()).await.unwrap();

        let link = LinkEntity::new("friend", alice.id, bob.id, None);
        link_svc.create(link.clone()).await.unwrap();

        // Both services share the same DB
        let entities = data_svc.list().await.unwrap();
        assert_eq!(entities.len(), 2);

        let links = link_svc
            .find_by_source(&alice.id, None, None)
            .await
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_id, bob.id);
    }

    #[tokio::test]
    async fn test_link_with_metadata() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        let link = LinkEntity::new(
            "worker",
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some(serde_json::json!({"role": "Developer", "since": "2024-01-01"})),
        );

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap().unwrap();
        assert_eq!(
            retrieved.metadata,
            Some(serde_json::json!({"role": "Developer", "since": "2024-01-01"}))
        );
    }
}
