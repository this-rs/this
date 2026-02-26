//! MongoDB storage backend using the official MongoDB async driver.
//!
//! Provides `MongoDataService<T>` and `MongoLinkService` implementations
//! backed by a MongoDB database via `mongodb::Database`.
//!
//! # Feature flag
//!
//! This module is gated behind the `mongodb_backend` feature flag:
//! ```toml
//! [dependencies]
//! this-rs = { version = "0.0.7", features = ["mongodb_backend"] }
//! ```
//!
//! # Storage model
//!
//! Unlike the SQL backends that use shared tables with entity_type filtering,
//! MongoDB uses a **collection-per-entity-type** pattern. Each `MongoDataService<T>`
//! operates on a collection named after `T::resource_name()` (e.g., "users", "companies").
//!
//! Links are stored in a single `links` collection with indexed fields for
//! efficient source/target traversal queries.
//!
//! # Serialization strategy
//!
//! Entities are serialized via `serde_json::Value` as an intermediate format,
//! then converted to BSON documents. This ensures consistent handling of
//! UUID (stored as strings) and DateTime (stored as ISO 8601 strings) types.
//! The `id` field is mapped to MongoDB's `_id` convention.

use crate::core::link::LinkEntity;
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::TryStreamExt;
use mongodb::Database;
use mongodb::bson::{Bson, Document, doc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a serde_json::Value (expected to be an Object) into a BSON Document,
/// renaming `id` → `_id` for MongoDB convention.
fn json_to_document(json: serde_json::Value) -> Result<Document> {
    let bson_val = mongodb::bson::to_bson(&json)
        .map_err(|e| anyhow!("Failed to convert JSON to BSON: {}", e))?;

    let mut doc = match bson_val {
        Bson::Document(d) => d,
        _ => return Err(anyhow!("Expected BSON document, got non-object")),
    };

    // MongoDB convention: rename id → _id
    if let Some(id) = doc.remove("id") {
        doc.insert("_id", id);
    }

    Ok(doc)
}

/// Convert a BSON Document back into a serde_json::Value,
/// renaming `_id` → `id` for domain entity convention.
fn document_to_json(mut doc: Document) -> serde_json::Value {
    // MongoDB convention: rename _id → id
    if let Some(id) = doc.remove("_id") {
        doc.insert("id", id);
    }

    Bson::Document(doc).into_relaxed_extjson()
}

/// Convert a UUID to its BSON string representation for queries.
fn uuid_bson(id: &Uuid) -> Bson {
    Bson::String(id.to_string())
}

// ---------------------------------------------------------------------------
// MongoDataService<T>
// ---------------------------------------------------------------------------

/// Generic data storage service backed by MongoDB.
///
/// Each entity type gets its own collection, named by `T::resource_name()`
/// (the pluralized entity name, e.g., "users", "companies").
///
/// # Type bounds
///
/// `T` must implement:
/// - `Data` — entity trait hierarchy (Entity + Data)
/// - `Serialize` — for serializing entity to BSON
/// - `DeserializeOwned` — for deserializing BSON to entity
///
/// # Example
///
/// ```rust,ignore
/// use mongodb::Client;
/// use this::storage::MongoDataService;
///
/// let client = Client::with_uri_str("mongodb://localhost:27017").await?;
/// let db = client.database("mydb");
/// let service = MongoDataService::<MyEntity>::new(db);
/// let entity = service.create(my_entity).await?;
/// ```
#[derive(Clone, Debug)]
pub struct MongoDataService<T> {
    database: Database,
    _marker: std::marker::PhantomData<T>,
}

impl<T> MongoDataService<T> {
    /// Create a new `MongoDataService` with the given database handle.
    pub fn new(database: Database) -> Self {
        Self {
            database,
            _marker: std::marker::PhantomData,
        }
    }

    /// Get a reference to the underlying database.
    pub fn database(&self) -> &Database {
        &self.database
    }
}

impl<T: Data + Serialize + DeserializeOwned> MongoDataService<T> {
    /// Get the MongoDB collection for this entity type.
    ///
    /// Uses `T::resource_name()` as the collection name (pluralized).
    fn collection(&self) -> mongodb::Collection<Document> {
        self.database.collection(T::resource_name())
    }

    /// Convert a domain entity into a MongoDB document.
    fn entity_to_document(entity: &T) -> Result<Document> {
        let json = serde_json::to_value(entity)
            .map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;
        json_to_document(json)
    }

    /// Convert a MongoDB document back into a domain entity.
    fn document_to_entity(doc: Document) -> Result<T> {
        let json = document_to_json(doc);
        serde_json::from_value(json)
            .map_err(|e| anyhow!("Failed to deserialize entity from document: {}", e))
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> DataService<T> for MongoDataService<T> {
    /// Insert a new entity into the collection.
    ///
    /// Inserts the document and reads it back to return the stored version.
    async fn create(&self, entity: T) -> Result<T> {
        let doc = Self::entity_to_document(&entity)?;
        let id_bson = uuid_bson(&entity.id());

        self.collection()
            .insert_one(doc)
            .await
            .map_err(|e| anyhow!("Failed to create entity: {}", e))?;

        // Read back the inserted entity
        let result = self
            .collection()
            .find_one(doc! { "_id": id_bson })
            .await
            .map_err(|e| anyhow!("Failed to read back created entity: {}", e))?
            .ok_or_else(|| anyhow!("Entity not found after insert"))?;

        Self::document_to_entity(result)
    }

    /// Fetch an entity by UUID.
    ///
    /// Returns `Ok(None)` if the entity does not exist.
    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let doc = self
            .collection()
            .find_one(doc! { "_id": uuid_bson(id) })
            .await
            .map_err(|e| anyhow!("Failed to get entity: {}", e))?;

        match doc {
            Some(d) => Ok(Some(Self::document_to_entity(d)?)),
            None => Ok(None),
        }
    }

    /// List all entities, ordered by creation time (newest first).
    async fn list(&self) -> Result<Vec<T>> {
        let cursor = self
            .collection()
            .find(doc! {})
            .sort(doc! { "created_at": -1 })
            .await
            .map_err(|e| anyhow!("Failed to list entities: {}", e))?;

        let docs: Vec<Document> = cursor
            .try_collect()
            .await
            .map_err(|e| anyhow!("Failed to collect entities: {}", e))?;

        docs.into_iter().map(Self::document_to_entity).collect()
    }

    /// Update an existing entity.
    ///
    /// Returns `Err` if the entity does not exist (no document matched).
    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        let doc = Self::entity_to_document(&entity)?;
        let id_bson = uuid_bson(id);

        let result = self
            .collection()
            .replace_one(doc! { "_id": &id_bson }, doc)
            .await
            .map_err(|e| anyhow!("Failed to update entity: {}", e))?;

        if result.matched_count == 0 {
            return Err(anyhow!("Entity not found: {}", id));
        }

        // Read back the updated entity
        let updated = self
            .collection()
            .find_one(doc! { "_id": id_bson })
            .await
            .map_err(|e| anyhow!("Failed to read back updated entity: {}", e))?
            .ok_or_else(|| anyhow!("Entity not found after update"))?;

        Self::document_to_entity(updated)
    }

    /// Delete an entity by UUID.
    ///
    /// Silently succeeds if the entity does not exist (idempotent).
    async fn delete(&self, id: &Uuid) -> Result<()> {
        self.collection()
            .delete_one(doc! { "_id": uuid_bson(id) })
            .await
            .map_err(|e| anyhow!("Failed to delete entity: {}", e))?;

        Ok(())
    }

    /// Search entities by field value.
    ///
    /// Since `DataService::search` receives both field and value as strings,
    /// but MongoDB stores values with native BSON types (integers, booleans,
    /// floats), we use `$in` with multiple type variants to match correctly.
    ///
    /// For example, searching for `("age", "25")` matches documents where
    /// `age` is either the string `"25"` or the integer `25`.
    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        // Build a list of BSON values to match (string + native type)
        let mut variants: Vec<Bson> = vec![Bson::String(value.to_string())];

        match value {
            "true" => variants.push(Bson::Boolean(true)),
            "false" => variants.push(Bson::Boolean(false)),
            _ => {
                if let Ok(i) = value.parse::<i64>() {
                    variants.push(Bson::Int64(i));
                }
                if value.contains('.')
                    && let Ok(f) = value.parse::<f64>()
                {
                    variants.push(Bson::Double(f));
                }
            }
        }

        let filter = if variants.len() == 1 {
            doc! { field: variants.into_iter().next().unwrap() }
        } else {
            doc! { field: { "$in": variants } }
        };

        let cursor = self
            .collection()
            .find(filter)
            .await
            .map_err(|e| anyhow!("Failed to search entities: {}", e))?;

        let docs: Vec<Document> = cursor
            .try_collect()
            .await
            .map_err(|e| anyhow!("Failed to collect search results: {}", e))?;

        docs.into_iter().map(Self::document_to_entity).collect()
    }
}

// ---------------------------------------------------------------------------
// MongoLinkService
// ---------------------------------------------------------------------------

/// Link storage service backed by MongoDB.
///
/// All links are stored in a single `links` collection with indexed fields
/// for efficient source/target traversal queries.
///
/// # Example
///
/// ```rust,ignore
/// use mongodb::Client;
/// use this::storage::MongoLinkService;
///
/// let client = Client::with_uri_str("mongodb://localhost:27017").await?;
/// let db = client.database("mydb");
/// let service = MongoLinkService::new(db);
/// let link = service.create(my_link).await?;
/// ```
#[derive(Clone, Debug)]
pub struct MongoLinkService {
    database: Database,
}

impl MongoLinkService {
    /// Create a new `MongoLinkService` with the given database handle.
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    /// Get a reference to the underlying database.
    pub fn database(&self) -> &Database {
        &self.database
    }

    /// Get the MongoDB collection for links.
    fn collection(&self) -> mongodb::Collection<Document> {
        self.database.collection("links")
    }

    /// Create indexes on the links collection for efficient querying.
    ///
    /// Creates the following indexes:
    /// - `source_id: 1` — fast `find_by_source` lookups
    /// - `target_id: 1` — fast `find_by_target` lookups
    /// - `source_id: 1, link_type: 1` — fast filtered source queries
    /// - `target_id: 1, link_type: 1` — fast filtered target queries
    ///
    /// This method is idempotent — safe to call on every startup.
    pub async fn ensure_indexes(&self) -> Result<()> {
        use mongodb::IndexModel;

        let indexes = vec![
            IndexModel::builder().keys(doc! { "source_id": 1 }).build(),
            IndexModel::builder().keys(doc! { "target_id": 1 }).build(),
            IndexModel::builder()
                .keys(doc! { "source_id": 1, "link_type": 1 })
                .build(),
            IndexModel::builder()
                .keys(doc! { "target_id": 1, "link_type": 1 })
                .build(),
        ];

        self.collection()
            .create_indexes(indexes)
            .await
            .map_err(|e| anyhow!("Failed to create indexes on links collection: {}", e))?;

        Ok(())
    }

    /// Convert a `LinkEntity` into a MongoDB document.
    fn link_to_document(link: &LinkEntity) -> Result<Document> {
        let json =
            serde_json::to_value(link).map_err(|e| anyhow!("Failed to serialize link: {}", e))?;
        json_to_document(json)
    }

    /// Convert a MongoDB document back into a `LinkEntity`.
    fn document_to_link(doc: Document) -> Result<LinkEntity> {
        let json = document_to_json(doc);
        serde_json::from_value(json)
            .map_err(|e| anyhow!("Failed to deserialize link from document: {}", e))
    }
}

#[async_trait]
impl LinkService for MongoLinkService {
    /// Insert a new link into the `links` collection.
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let doc = Self::link_to_document(&link)?;
        let id_bson = uuid_bson(&link.id);

        self.collection()
            .insert_one(doc)
            .await
            .map_err(|e| anyhow!("Failed to create link: {}", e))?;

        let result = self
            .collection()
            .find_one(doc! { "_id": id_bson })
            .await
            .map_err(|e| anyhow!("Failed to read back created link: {}", e))?
            .ok_or_else(|| anyhow!("Link not found after insert"))?;

        Self::document_to_link(result)
    }

    /// Fetch a link by UUID.
    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let doc = self
            .collection()
            .find_one(doc! { "_id": uuid_bson(id) })
            .await
            .map_err(|e| anyhow!("Failed to get link: {}", e))?;

        match doc {
            Some(d) => Ok(Some(Self::document_to_link(d)?)),
            None => Ok(None),
        }
    }

    /// List all links, ordered by creation time (newest first).
    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let cursor = self
            .collection()
            .find(doc! {})
            .sort(doc! { "created_at": -1 })
            .await
            .map_err(|e| anyhow!("Failed to list links: {}", e))?;

        let docs: Vec<Document> = cursor
            .try_collect()
            .await
            .map_err(|e| anyhow!("Failed to collect links: {}", e))?;

        docs.into_iter().map(Self::document_to_link).collect()
    }

    /// Find links by source entity, with optional link_type filter.
    ///
    /// **Note:** `target_type` is currently ignored because `LinkEntity` does not
    /// carry entity-type metadata — matching `InMemoryLinkService` behavior.
    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let mut filter = doc! { "source_id": uuid_bson(source_id) };
        if let Some(lt) = link_type {
            filter.insert("link_type", lt);
        }

        let cursor = self
            .collection()
            .find(filter)
            .sort(doc! { "created_at": -1 })
            .await
            .map_err(|e| anyhow!("Failed to find links by source: {}", e))?;

        let docs: Vec<Document> = cursor
            .try_collect()
            .await
            .map_err(|e| anyhow!("Failed to collect links: {}", e))?;

        docs.into_iter().map(Self::document_to_link).collect()
    }

    /// Find links by target entity, with optional link_type filter.
    ///
    /// **Note:** `source_type` is currently ignored because `LinkEntity` does not
    /// carry entity-type metadata — matching `InMemoryLinkService` behavior.
    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let mut filter = doc! { "target_id": uuid_bson(target_id) };
        if let Some(lt) = link_type {
            filter.insert("link_type", lt);
        }

        let cursor = self
            .collection()
            .find(filter)
            .sort(doc! { "created_at": -1 })
            .await
            .map_err(|e| anyhow!("Failed to find links by target: {}", e))?;

        let docs: Vec<Document> = cursor
            .try_collect()
            .await
            .map_err(|e| anyhow!("Failed to collect links: {}", e))?;

        docs.into_iter().map(Self::document_to_link).collect()
    }

    /// Update a link's fields.
    ///
    /// Returns `Err` if the link does not exist.
    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        let doc = Self::link_to_document(&link)?;
        let id_bson = uuid_bson(id);

        let result = self
            .collection()
            .replace_one(doc! { "_id": &id_bson }, doc)
            .await
            .map_err(|e| anyhow!("Failed to update link: {}", e))?;

        if result.matched_count == 0 {
            return Err(anyhow!("Link not found: {}", id));
        }

        let updated = self
            .collection()
            .find_one(doc! { "_id": id_bson })
            .await
            .map_err(|e| anyhow!("Failed to read back updated link: {}", e))?
            .ok_or_else(|| anyhow!("Link not found after update"))?;

        Self::document_to_link(updated)
    }

    /// Delete a link by UUID.
    ///
    /// Silently succeeds if the link does not exist (idempotent).
    async fn delete(&self, id: &Uuid) -> Result<()> {
        self.collection()
            .delete_one(doc! { "_id": uuid_bson(id) })
            .await
            .map_err(|e| anyhow!("Failed to delete link: {}", e))?;

        Ok(())
    }

    /// Delete all links involving a specific entity (as source OR target).
    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let id_bson = uuid_bson(entity_id);
        self.collection()
            .delete_many(doc! {
                "$or": [
                    { "source_id": id_bson.clone() },
                    { "target_id": id_bson }
                ]
            })
            .await
            .map_err(|e| anyhow!("Failed to delete links by entity: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "mongodb_backend")]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // json_to_document
    // -----------------------------------------------------------------------

    #[test]
    fn json_to_document_renames_id_to_underscore_id() {
        let input = json!({"id": "abc", "name": "test"});
        let doc = json_to_document(input).unwrap();

        assert!(doc.contains_key("_id"), "document should contain _id");
        assert!(!doc.contains_key("id"), "document should not contain id");
        assert_eq!(doc.get_str("_id").unwrap(), "abc");
    }

    #[test]
    fn json_to_document_preserves_other_fields() {
        let input = json!({"id": "abc", "name": "test", "age": 42});
        let doc = json_to_document(input).unwrap();

        assert_eq!(doc.get_str("name").unwrap(), "test");
        // serde_json::json!(42) serializes to i64 in BSON
        assert_eq!(doc.get_i64("age").unwrap(), 42);
    }

    #[test]
    fn json_to_document_non_object_returns_error() {
        let input = json!("string");
        let result = json_to_document(input);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("non-object"),
            "error should mention non-object, got: {err_msg}"
        );
    }

    #[test]
    fn json_to_document_nested_objects() {
        let input = json!({"id": "x", "data": {"nested": true}});
        let doc = json_to_document(input).unwrap();

        assert_eq!(doc.get_str("_id").unwrap(), "x");
        let nested = doc.get_document("data").unwrap();
        assert_eq!(nested.get_bool("nested").unwrap(), true);
    }

    // -----------------------------------------------------------------------
    // document_to_json
    // -----------------------------------------------------------------------

    #[test]
    fn document_to_json_renames_underscore_id_to_id() {
        let doc = doc! { "_id": "abc", "name": "test" };
        let json = document_to_json(doc);

        assert_eq!(json["id"], "abc");
        assert!(json.get("_id").is_none(), "json should not contain _id");
    }

    #[test]
    fn document_to_json_preserves_fields() {
        let doc = doc! { "_id": "abc", "name": "test", "age": 42 };
        let json = document_to_json(doc);

        assert_eq!(json["name"], "test");
        assert_eq!(json["age"], 42);
    }

    // -----------------------------------------------------------------------
    // roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn json_document_roundtrip() {
        let original = json!({"id": "round", "name": "trip"});
        let doc = json_to_document(original.clone()).unwrap();

        // After json_to_document: "id" became "_id"
        assert!(doc.contains_key("_id"));
        assert!(!doc.contains_key("id"));

        // After document_to_json: "_id" becomes "id" again
        let back = document_to_json(doc);
        assert_eq!(back["id"], "round");
        assert_eq!(back["name"], "trip");
        assert!(back.get("_id").is_none());
    }

    #[test]
    fn json_document_roundtrip_with_nested() {
        let original = json!({
            "id": "nested-rt",
            "payload": {
                "items": [1, 2, 3],
                "meta": {"key": "value"}
            }
        });
        let doc = json_to_document(original).unwrap();
        let back = document_to_json(doc);

        assert_eq!(back["id"], "nested-rt");
        assert_eq!(back["payload"]["items"], json!([1, 2, 3]));
        assert_eq!(back["payload"]["meta"]["key"], "value");
    }

    // -----------------------------------------------------------------------
    // uuid_bson
    // -----------------------------------------------------------------------

    #[test]
    fn uuid_bson_returns_string() {
        let id = Uuid::new_v4();
        let bson = uuid_bson(&id);

        match bson {
            Bson::String(s) => assert_eq!(s, id.to_string()),
            other => panic!("expected Bson::String, got: {other:?}"),
        }
    }
}
