//! Neo4j storage backend using the neo4rs Bolt protocol driver.
//!
//! Provides `Neo4jDataService<T>` and `Neo4jLinkService` implementations
//! backed by a Neo4j database via `neo4rs::Graph`.
//!
//! # Feature flag
//!
//! This module is gated behind the `neo4j` feature flag:
//! ```toml
//! [dependencies]
//! this-rs = { version = "0.0.7", features = ["neo4j"] }
//! ```
//!
//! # Storage model
//!
//! Entities are stored as Neo4j nodes. Each entity type gets its own label
//! (from `T::resource_name_singular()`). All scalar fields are stored as
//! individual node properties for searchability. A `__data` property holds
//! the full JSON string for reliable deserialization.
//!
//! Links are stored as nodes with label `_Link` (not as native relationships)
//! to maintain compatibility with the `LinkService` contract — which allows
//! creating links without requiring source/target entities to exist in the store.

use crate::core::link::LinkEntity;
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use neo4rs::{BoltMap, BoltString, BoltType, Graph, Node, query};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Conversion helpers: JSON ↔ Neo4j properties
// ---------------------------------------------------------------------------

/// Convert a serde_json::Value into a BoltType for Neo4j storage.
///
/// - Strings → BoltType::String
/// - Integers → BoltType::Integer
/// - Floats → BoltType::Float
/// - Booleans → BoltType::Boolean
/// - Null → BoltType::Null
/// - Objects/Arrays → BoltType::String (JSON serialized)
fn json_value_to_bolt(value: &serde_json::Value) -> BoltType {
    match value {
        serde_json::Value::String(s) => BoltType::from(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                BoltType::from(i)
            } else if let Some(f) = n.as_f64() {
                BoltType::from(f)
            } else {
                BoltType::from(n.to_string())
            }
        }
        serde_json::Value::Bool(b) => BoltType::from(*b),
        serde_json::Value::Null => BoltType::Null(neo4rs::BoltNull),
        other => BoltType::from(other.to_string()),
    }
}

/// Convert a serializable entity to a HashMap<BoltString, BoltType> for Neo4j.
///
/// All scalar fields become typed properties. Null fields are included
/// as BoltType::Null. A `__data` property is added with the full JSON string.
fn entity_to_bolt_props<T: Serialize>(entity: &T) -> Result<BoltType> {
    let json =
        serde_json::to_value(entity).map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;

    let obj = json
        .as_object()
        .ok_or_else(|| anyhow!("Expected JSON object"))?;

    let mut map = BoltMap::new();
    for (key, value) in obj {
        map.put(BoltString::from(key.as_str()), json_value_to_bolt(value));
    }

    // Add __data with the full JSON string for reliable deserialization
    let json_str = serde_json::to_string(entity)
        .map_err(|e| anyhow!("Failed to serialize entity to string: {}", e))?;
    map.put(BoltString::from("__data"), BoltType::from(json_str));

    Ok(BoltType::Map(map))
}

/// Extract the `__data` JSON string from a Neo4j Node and deserialize to T.
fn node_to_entity<T: DeserializeOwned>(node: &Node) -> Result<T> {
    let data: String = node
        .get("__data")
        .map_err(|_| anyhow!("Missing __data property on node"))?;
    serde_json::from_str(&data)
        .map_err(|e| anyhow!("Failed to deserialize entity from __data: {}", e))
}

/// Parse a search value string into the appropriate BoltType.
///
/// Tries boolean, integer, float, then falls back to string.
/// For search matching, we need the value type to match the stored property type.
fn parse_search_value(value: &str) -> BoltType {
    match value {
        "true" => BoltType::from(true),
        "false" => BoltType::from(false),
        _ => {
            if let Ok(i) = value.parse::<i64>() {
                return BoltType::from(i);
            }
            if value.contains('.')
                && let Ok(f) = value.parse::<f64>()
            {
                return BoltType::from(f);
            }
            BoltType::from(value.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Neo4jDataService<T>
// ---------------------------------------------------------------------------

/// Generic data storage service backed by Neo4j.
///
/// Each entity type gets its own node label from `T::resource_name_singular()`.
///
/// # Example
///
/// ```rust,ignore
/// use neo4rs::Graph;
/// use this::storage::Neo4jDataService;
///
/// let graph = Graph::new("127.0.0.1:7687", "neo4j", "password").await?;
/// let service = Neo4jDataService::<MyEntity>::new(graph);
/// ```
#[derive(Clone)]
pub struct Neo4jDataService<T> {
    graph: Graph,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Neo4jDataService<T> {
    pub fn new(graph: Graph) -> Self {
        Self {
            graph,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }
}

impl<T: Data + Serialize + DeserializeOwned> Neo4jDataService<T> {
    /// Node label for this entity type (e.g., "user", "order").
    fn label() -> &'static str {
        T::resource_name_singular()
    }

    /// Create indexes and constraints for this entity type.
    ///
    /// Creates:
    /// - Uniqueness constraint on `id` (also serves as an index)
    /// - Index on `name` for common lookups
    /// - Index on `entity_type` for type-scoped queries
    ///
    /// This method is idempotent — safe to call on every startup.
    pub async fn ensure_indexes(&self) -> Result<()> {
        let label = Self::label();

        // Uniqueness constraint on id (implicitly creates an index)
        let constraint = format!(
            "CREATE CONSTRAINT IF NOT EXISTS FOR (n:`{}`) REQUIRE n.id IS UNIQUE",
            label
        );
        self.graph
            .run(query(&constraint))
            .await
            .map_err(|e| anyhow!("Failed to create uniqueness constraint: {}", e))?;

        // Index on name for search
        let name_idx = format!("CREATE INDEX IF NOT EXISTS FOR (n:`{}`) ON (n.name)", label);
        self.graph
            .run(query(&name_idx))
            .await
            .map_err(|e| anyhow!("Failed to create name index: {}", e))?;

        // Index on entity_type for filtered listing
        let type_idx = format!(
            "CREATE INDEX IF NOT EXISTS FOR (n:`{}`) ON (n.entity_type)",
            label
        );
        self.graph
            .run(query(&type_idx))
            .await
            .map_err(|e| anyhow!("Failed to create entity_type index: {}", e))?;

        Ok(())
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> DataService<T> for Neo4jDataService<T> {
    async fn create(&self, entity: T) -> Result<T> {
        let props = entity_to_bolt_props(&entity)?;
        let id = entity.id().to_string();

        // MERGE on id to get upsert behavior — avoids creating duplicate nodes
        // when the same UUID is inserted twice (Neo4j CREATE always creates a
        // new node regardless of property values).
        let cypher = format!(
            "MERGE (n:`{}` {{id: $id}}) SET n = $props RETURN n",
            Self::label()
        );

        let mut result = self
            .graph
            .execute(query(&cypher).param("id", id).param("props", props))
            .await
            .map_err(|e| anyhow!("Failed to create entity: {}", e))?;

        let row = result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to read result: {}", e))?
            .ok_or_else(|| anyhow!("No result returned from CREATE"))?;

        let node: Node = row
            .get("n")
            .map_err(|e| anyhow!("Failed to get node from row: {}", e))?;

        node_to_entity(&node)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let cypher = format!("MATCH (n:`{}` {{id: $id}}) RETURN n", Self::label());

        let mut result = self
            .graph
            .execute(query(&cypher).param("id", id.to_string()))
            .await
            .map_err(|e| anyhow!("Failed to get entity: {}", e))?;

        match result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to read result: {}", e))?
        {
            Some(row) => {
                let node: Node = row
                    .get("n")
                    .map_err(|e| anyhow!("Failed to get node: {}", e))?;
                Ok(Some(node_to_entity(&node)?))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<T>> {
        let cypher = format!(
            "MATCH (n:`{}`) RETURN n ORDER BY n.created_at DESC",
            Self::label()
        );

        let mut result = self
            .graph
            .execute(query(&cypher))
            .await
            .map_err(|e| anyhow!("Failed to list entities: {}", e))?;

        let mut entities = Vec::new();
        while let Some(row) = result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to iterate: {}", e))?
        {
            let node: Node = row
                .get("n")
                .map_err(|e| anyhow!("Failed to get node: {}", e))?;
            entities.push(node_to_entity(&node)?);
        }

        Ok(entities)
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        let props = entity_to_bolt_props(&entity)?;
        let cypher = format!(
            "MATCH (n:`{}` {{id: $id}}) SET n = $props RETURN n",
            Self::label()
        );

        let mut result = self
            .graph
            .execute(
                query(&cypher)
                    .param("id", id.to_string())
                    .param("props", props),
            )
            .await
            .map_err(|e| anyhow!("Failed to update entity: {}", e))?;

        match result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to read result: {}", e))?
        {
            Some(row) => {
                let node: Node = row
                    .get("n")
                    .map_err(|e| anyhow!("Failed to get node: {}", e))?;
                node_to_entity(&node)
            }
            None => Err(anyhow!("Entity not found: {}", id)),
        }
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let cypher = format!("MATCH (n:`{}` {{id: $id}}) DELETE n", Self::label());

        self.graph
            .run(query(&cypher).param("id", id.to_string()))
            .await
            .map_err(|e| anyhow!("Failed to delete entity: {}", e))?;

        Ok(())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        // Build Cypher with field name interpolated (safe: field comes from our code)
        // and value as a parameterized query argument with type-smart parsing
        let cypher = format!(
            "MATCH (n:`{}`) WHERE n.`{}` = $value RETURN n",
            Self::label(),
            field
        );

        let bolt_value = parse_search_value(value);

        let mut result = self
            .graph
            .execute(query(&cypher).param("value", bolt_value))
            .await
            .map_err(|e| anyhow!("Failed to search entities: {}", e))?;

        let mut entities = Vec::new();
        while let Some(row) = result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to iterate: {}", e))?
        {
            let node: Node = row
                .get("n")
                .map_err(|e| anyhow!("Failed to get node: {}", e))?;
            entities.push(node_to_entity(&node)?);
        }

        Ok(entities)
    }
}

// ---------------------------------------------------------------------------
// Neo4jLinkService
// ---------------------------------------------------------------------------

/// Link storage service backed by Neo4j.
///
/// Links are stored as nodes with label `_Link` (not as native Neo4j
/// relationships) to maintain compatibility with the `LinkService` contract.
///
/// # Example
///
/// ```rust,ignore
/// use neo4rs::Graph;
/// use this::storage::Neo4jLinkService;
///
/// let graph = Graph::new("127.0.0.1:7687", "neo4j", "password").await?;
/// let service = Neo4jLinkService::new(graph);
/// ```
#[derive(Clone)]
pub struct Neo4jLinkService {
    graph: Graph,
}

impl Neo4jLinkService {
    pub fn new(graph: Graph) -> Self {
        Self { graph }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Create indexes on the `_Link` nodes for efficient querying.
    ///
    /// Creates:
    /// - Uniqueness constraint on `id`
    /// - Index on `source_id` for `find_by_source`
    /// - Index on `target_id` for `find_by_target`
    /// - Composite index on `source_id, link_type` for filtered queries
    /// - Composite index on `target_id, link_type` for filtered queries
    ///
    /// This method is idempotent — safe to call on every startup.
    pub async fn ensure_indexes(&self) -> Result<()> {
        let queries = [
            "CREATE CONSTRAINT IF NOT EXISTS FOR (l:`_Link`) REQUIRE l.id IS UNIQUE",
            "CREATE INDEX IF NOT EXISTS FOR (l:`_Link`) ON (l.source_id)",
            "CREATE INDEX IF NOT EXISTS FOR (l:`_Link`) ON (l.target_id)",
            "CREATE INDEX IF NOT EXISTS FOR (l:`_Link`) ON (l.source_id, l.link_type)",
            "CREATE INDEX IF NOT EXISTS FOR (l:`_Link`) ON (l.target_id, l.link_type)",
        ];

        for cypher in &queries {
            self.graph
                .run(query(cypher))
                .await
                .map_err(|e| anyhow!("Failed to create _Link index: {}", e))?;
        }

        Ok(())
    }
}

#[async_trait]
impl LinkService for Neo4jLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let props = entity_to_bolt_props(&link)?;

        let mut result = self
            .graph
            .execute(query("CREATE (l:`_Link`) SET l = $props RETURN l").param("props", props))
            .await
            .map_err(|e| anyhow!("Failed to create link: {}", e))?;

        let row = result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to read result: {}", e))?
            .ok_or_else(|| anyhow!("No result from CREATE"))?;

        let node: Node = row
            .get("l")
            .map_err(|e| anyhow!("Failed to get node: {}", e))?;

        node_to_entity(&node)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let mut result = self
            .graph
            .execute(query("MATCH (l:`_Link` {id: $id}) RETURN l").param("id", id.to_string()))
            .await
            .map_err(|e| anyhow!("Failed to get link: {}", e))?;

        match result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to read result: {}", e))?
        {
            Some(row) => {
                let node: Node = row
                    .get("l")
                    .map_err(|e| anyhow!("Failed to get node: {}", e))?;
                Ok(Some(node_to_entity(&node)?))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let mut result = self
            .graph
            .execute(query(
                "MATCH (l:`_Link`) RETURN l ORDER BY l.created_at DESC",
            ))
            .await
            .map_err(|e| anyhow!("Failed to list links: {}", e))?;

        let mut links = Vec::new();
        while let Some(row) = result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to iterate: {}", e))?
        {
            let node: Node = row
                .get("l")
                .map_err(|e| anyhow!("Failed to get node: {}", e))?;
            links.push(node_to_entity(&node)?);
        }

        Ok(links)
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let q = if let Some(lt) = link_type {
            query("MATCH (l:`_Link` {source_id: $sid, link_type: $lt}) RETURN l ORDER BY l.created_at DESC")
                .param("sid", source_id.to_string())
                .param("lt", lt.to_string())
        } else {
            query("MATCH (l:`_Link` {source_id: $sid}) RETURN l ORDER BY l.created_at DESC")
                .param("sid", source_id.to_string())
        };

        let mut result = self
            .graph
            .execute(q)
            .await
            .map_err(|e| anyhow!("Failed to find links by source: {}", e))?;

        let mut links = Vec::new();
        while let Some(row) = result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to iterate: {}", e))?
        {
            let node: Node = row
                .get("l")
                .map_err(|e| anyhow!("Failed to get node: {}", e))?;
            links.push(node_to_entity(&node)?);
        }

        Ok(links)
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let q = if let Some(lt) = link_type {
            query("MATCH (l:`_Link` {target_id: $tid, link_type: $lt}) RETURN l ORDER BY l.created_at DESC")
                .param("tid", target_id.to_string())
                .param("lt", lt.to_string())
        } else {
            query("MATCH (l:`_Link` {target_id: $tid}) RETURN l ORDER BY l.created_at DESC")
                .param("tid", target_id.to_string())
        };

        let mut result = self
            .graph
            .execute(q)
            .await
            .map_err(|e| anyhow!("Failed to find links by target: {}", e))?;

        let mut links = Vec::new();
        while let Some(row) = result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to iterate: {}", e))?
        {
            let node: Node = row
                .get("l")
                .map_err(|e| anyhow!("Failed to get node: {}", e))?;
            links.push(node_to_entity(&node)?);
        }

        Ok(links)
    }

    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        let props = entity_to_bolt_props(&link)?;

        let mut result = self
            .graph
            .execute(
                query("MATCH (l:`_Link` {id: $id}) SET l = $props RETURN l")
                    .param("id", id.to_string())
                    .param("props", props),
            )
            .await
            .map_err(|e| anyhow!("Failed to update link: {}", e))?;

        match result
            .next()
            .await
            .map_err(|e| anyhow!("Failed to read result: {}", e))?
        {
            Some(row) => {
                let node: Node = row
                    .get("l")
                    .map_err(|e| anyhow!("Failed to get node: {}", e))?;
                node_to_entity(&node)
            }
            None => Err(anyhow!("Link not found: {}", id)),
        }
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        self.graph
            .run(query("MATCH (l:`_Link` {id: $id}) DELETE l").param("id", id.to_string()))
            .await
            .map_err(|e| anyhow!("Failed to delete link: {}", e))?;

        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let eid = entity_id.to_string();
        self.graph
            .run(
                query("MATCH (l:`_Link`) WHERE l.source_id = $eid OR l.target_id = $eid DELETE l")
                    .param("eid", eid),
            )
            .await
            .map_err(|e| anyhow!("Failed to delete links by entity: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "neo4j")]
mod tests {
    use super::*;
    use serde_json::json;

    // === json_value_to_bolt ===

    #[test]
    fn test_json_value_to_bolt_string() {
        let val = json!("hello");
        let bolt = json_value_to_bolt(&val);
        // BoltType::String variant
        assert!(
            matches!(bolt, BoltType::String(_)),
            "expected String variant, got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_json_value_to_bolt_integer() {
        let val = json!(42);
        let bolt = json_value_to_bolt(&val);
        assert!(
            matches!(bolt, BoltType::Integer(_)),
            "expected Integer variant, got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_json_value_to_bolt_float() {
        let val = json!(3.14);
        let bolt = json_value_to_bolt(&val);
        // JSON numbers that have decimals should become Float
        assert!(
            matches!(bolt, BoltType::Float(_)),
            "expected Float variant, got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_json_value_to_bolt_bool_true() {
        let val = json!(true);
        let bolt = json_value_to_bolt(&val);
        assert!(
            matches!(bolt, BoltType::Boolean(_)),
            "expected Boolean variant, got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_json_value_to_bolt_bool_false() {
        let val = json!(false);
        let bolt = json_value_to_bolt(&val);
        assert!(
            matches!(bolt, BoltType::Boolean(_)),
            "expected Boolean variant, got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_json_value_to_bolt_null() {
        let val = json!(null);
        let bolt = json_value_to_bolt(&val);
        assert!(
            matches!(bolt, BoltType::Null(_)),
            "expected Null variant, got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_json_value_to_bolt_object_becomes_string() {
        let val = json!({"nested": "object"});
        let bolt = json_value_to_bolt(&val);
        // Objects/Arrays are serialized to JSON string
        assert!(
            matches!(bolt, BoltType::String(_)),
            "expected String variant for object, got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_json_value_to_bolt_array_becomes_string() {
        let val = json!([1, 2, 3]);
        let bolt = json_value_to_bolt(&val);
        assert!(
            matches!(bolt, BoltType::String(_)),
            "expected String variant for array, got: {:?}",
            bolt
        );
    }

    // === parse_search_value ===

    #[test]
    fn test_parse_search_value_true() {
        let bolt = parse_search_value("true");
        assert!(
            matches!(bolt, BoltType::Boolean(_)),
            "expected Boolean for 'true', got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_parse_search_value_false() {
        let bolt = parse_search_value("false");
        assert!(
            matches!(bolt, BoltType::Boolean(_)),
            "expected Boolean for 'false', got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_parse_search_value_integer() {
        let bolt = parse_search_value("42");
        assert!(
            matches!(bolt, BoltType::Integer(_)),
            "expected Integer for '42', got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_parse_search_value_negative_integer() {
        let bolt = parse_search_value("-7");
        assert!(
            matches!(bolt, BoltType::Integer(_)),
            "expected Integer for '-7', got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_parse_search_value_float() {
        let bolt = parse_search_value("3.14");
        assert!(
            matches!(bolt, BoltType::Float(_)),
            "expected Float for '3.14', got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_parse_search_value_string_fallback() {
        let bolt = parse_search_value("hello world");
        assert!(
            matches!(bolt, BoltType::String(_)),
            "expected String for 'hello world', got: {:?}",
            bolt
        );
    }

    #[test]
    fn test_parse_search_value_number_without_dot_is_integer() {
        // "100" should be parsed as Integer, not Float
        let bolt = parse_search_value("100");
        assert!(
            matches!(bolt, BoltType::Integer(_)),
            "expected Integer for '100', got: {:?}",
            bolt
        );
    }

    // === entity_to_bolt_props ===

    #[test]
    fn test_entity_to_bolt_props_returns_map() {
        #[derive(Serialize)]
        struct Simple {
            name: String,
            count: i32,
        }
        let entity = Simple {
            name: "test".to_string(),
            count: 5,
        };
        let result = entity_to_bolt_props(&entity).expect("should convert");
        assert!(
            matches!(result, BoltType::Map(_)),
            "expected Map variant, got: {:?}",
            result
        );
    }

    #[test]
    fn test_entity_to_bolt_props_includes_data_key() {
        #[derive(Serialize)]
        struct Item {
            id: String,
        }
        let entity = Item {
            id: "abc".to_string(),
        };
        let result = entity_to_bolt_props(&entity).expect("should convert");
        if let BoltType::Map(map) = result {
            // The map should contain __data key
            let has_data = map.value.iter().any(|(k, _)| k.value == "__data");
            assert!(has_data, "map should contain __data key");
        } else {
            panic!("expected Map variant");
        }
    }

    #[test]
    fn test_entity_to_bolt_props_non_object_returns_error() {
        // A bare string will serialize to a JSON string, not an object
        let result = entity_to_bolt_props(&"not an object");
        assert!(result.is_err(), "non-object should return error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Expected JSON object"),
            "error should mention JSON object: {}",
            err
        );
    }
}
