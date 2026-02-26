//! PostgreSQL storage backend using sqlx.
//!
//! Provides `PostgresDataService<T>` and `PostgresLinkService` implementations
//! backed by a PostgreSQL database via `sqlx::PgPool`.
//!
//! # Feature flag
//!
//! This module is gated behind the `postgres` feature flag:
//! ```toml
//! [dependencies]
//! this-rs = { version = "0.0.7", features = ["postgres"] }
//! ```
//!
//! # Schema
//!
//! Entities are stored in a shared `entities` table with common columns
//! (id, entity_type, name, status, timestamps) and a JSONB `data` column
//! for type-specific fields. See `migrations/001_create_entities.up.sql`.
//!
//! Links are stored in a `links` table with dedicated columns for
//! relationship traversal. See `migrations/002_create_links.up.sql`.
//!
//! # Entity type convention
//!
//! The `entity_type` column is populated from `T::resource_name_singular()`.
//! All query filters (get, list, update, delete, search) use this value
//! to scope operations to the correct entity type.

use crate::core::link::LinkEntity;
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// EntityRow — intermediate struct for DB row mapping
// ---------------------------------------------------------------------------

/// Database row representation for the `entities` table.
///
/// Maps 1:1 to the SQL schema. Type-specific fields are stored
/// in the JSONB `data` column; common fields have dedicated columns.
#[derive(Debug, FromRow)]
struct EntityRow {
    id: Uuid,
    entity_type: String,
    name: String,
    status: String,
    tenant_id: Option<Uuid>,
    data: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

/// Common entity fields stored in dedicated columns (excluded from JSONB data).
///
/// Note: `entity_type` and `type` are intentionally NOT in this list.
/// They are preserved in the JSONB `data` column so that the original
/// `entity_type()` value survives the round-trip (the SQL column uses
/// `resource_name_singular()` for query scoping, which may differ).
const ENTITY_COMMON_FIELDS: &[&str] = &[
    "id",
    "name",
    "status",
    "tenant_id",
    "created_at",
    "updated_at",
    "deleted_at",
];

/// Common entity fields that can be searched via direct SQL column comparison.
/// These field names are safe to interpolate into SQL because they are whitelisted.
const SEARCHABLE_COLUMNS: &[&str] = &["name", "status"];

// ---------------------------------------------------------------------------
// PostgresDataService<T>
// ---------------------------------------------------------------------------

/// Generic data storage service backed by PostgreSQL.
///
/// Stores entities in a shared `entities` table with common columns
/// (id, entity_type, name, status, timestamps) and a JSONB `data`
/// column for type-specific fields.
///
/// # Type bounds
///
/// `T` must implement:
/// - `Data` — entity trait hierarchy (Entity + Data)
/// - `Serialize` — for serializing entity → JSONB
/// - `DeserializeOwned` — for deserializing JSONB → entity
///
/// # Example
///
/// ```rust,ignore
/// use sqlx::PgPool;
/// use this::storage::PostgresDataService;
///
/// let pool = PgPool::connect("postgres://localhost/mydb").await?;
/// let service = PostgresDataService::<MyEntity>::new(pool);
/// let entity = service.create(my_entity).await?;
/// ```
#[derive(Clone, Debug)]
pub struct PostgresDataService<T> {
    pool: PgPool,
    _marker: std::marker::PhantomData<T>,
}

impl<T> PostgresDataService<T> {
    /// Create a new `PostgresDataService` with the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            _marker: std::marker::PhantomData,
        }
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

impl<T: Data + Serialize + DeserializeOwned> PostgresDataService<T> {
    /// Get the entity type string used for SQL filtering.
    ///
    /// Uses `T::resource_name_singular()` as the canonical type identifier.
    fn entity_type_name() -> &'static str {
        T::resource_name_singular()
    }

    /// Convert a domain entity into a database row.
    ///
    /// Serializes the full entity to JSON, extracts common fields into
    /// dedicated columns, and stores remaining fields in the JSONB `data` column.
    fn entity_to_row(entity: &T) -> Result<EntityRow> {
        // Serialize the full entity to JSON
        let mut data = serde_json::to_value(entity)
            .map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;

        // Remove common fields from data (they're stored in dedicated columns)
        if let Some(obj) = data.as_object_mut() {
            for field in ENTITY_COMMON_FIELDS {
                obj.remove(*field);
            }
        }

        Ok(EntityRow {
            id: entity.id(),
            entity_type: Self::entity_type_name().to_string(),
            name: entity.name().to_string(),
            status: entity.status().to_string(),
            tenant_id: entity.tenant_id(),
            data,
            created_at: entity.created_at(),
            updated_at: entity.updated_at(),
            deleted_at: entity.deleted_at(),
        })
    }

    /// Convert a database row back into a domain entity.
    ///
    /// Merges common columns back into the JSONB data, then deserializes
    /// the combined JSON into the target type `T`.
    fn row_to_entity(row: EntityRow) -> Result<T> {
        // Start with the JSONB data (custom fields)
        let mut json = if row.data.is_object() {
            row.data
        } else {
            serde_json::json!({})
        };

        // Merge common fields back into the JSON.
        // entity_type/type are already in the JSONB (preserved from the original entity),
        // so only inject them as fallback if missing.
        if let Some(obj) = json.as_object_mut() {
            obj.insert("id".into(), serde_json::to_value(row.id)?);
            // Only inject entity_type/type if not already present in JSONB
            // (the JSONB preserves the original value from the entity)
            if !obj.contains_key("entity_type") {
                obj.insert(
                    "entity_type".into(),
                    serde_json::to_value(&row.entity_type)?,
                );
            }
            if !obj.contains_key("type") {
                obj.insert("type".into(), serde_json::to_value(&row.entity_type)?);
            }
            obj.insert("name".into(), serde_json::to_value(&row.name)?);
            obj.insert("status".into(), serde_json::to_value(&row.status)?);
            obj.insert("created_at".into(), serde_json::to_value(row.created_at)?);
            obj.insert("updated_at".into(), serde_json::to_value(row.updated_at)?);
            obj.insert("deleted_at".into(), serde_json::to_value(row.deleted_at)?);
            if let Some(tid) = row.tenant_id {
                obj.insert("tenant_id".into(), serde_json::to_value(tid)?);
            }
        }

        serde_json::from_value::<T>(json)
            .map_err(|e| anyhow!("Failed to deserialize entity from row: {}", e))
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> DataService<T> for PostgresDataService<T> {
    /// Insert a new entity into the `entities` table.
    ///
    /// Returns the created entity as read back from the database.
    async fn create(&self, entity: T) -> Result<T> {
        let row = Self::entity_to_row(&entity)?;

        let result = sqlx::query_as::<_, EntityRow>(
            "INSERT INTO entities (id, entity_type, name, status, tenant_id, data, created_at, updated_at, deleted_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             RETURNING *",
        )
        .bind(row.id)
        .bind(&row.entity_type)
        .bind(&row.name)
        .bind(&row.status)
        .bind(row.tenant_id)
        .bind(&row.data)
        .bind(row.created_at)
        .bind(row.updated_at)
        .bind(row.deleted_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to create entity: {}", e))?;

        Self::row_to_entity(result)
    }

    /// Fetch an entity by UUID, scoped to entity type `T`.
    ///
    /// Returns `Ok(None)` if the entity does not exist.
    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let row = sqlx::query_as::<_, EntityRow>(
            "SELECT * FROM entities WHERE id = $1 AND entity_type = $2",
        )
        .bind(id)
        .bind(Self::entity_type_name())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to get entity: {}", e))?;

        match row {
            Some(r) => Ok(Some(Self::row_to_entity(r)?)),
            None => Ok(None),
        }
    }

    /// List all entities of type `T`, ordered by creation time (newest first).
    async fn list(&self) -> Result<Vec<T>> {
        let rows = sqlx::query_as::<_, EntityRow>(
            "SELECT * FROM entities WHERE entity_type = $1 ORDER BY created_at DESC",
        )
        .bind(Self::entity_type_name())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to list entities: {}", e))?;

        rows.into_iter().map(Self::row_to_entity).collect()
    }

    /// Update an existing entity.
    ///
    /// Returns `Err` if the entity does not exist (no row matched).
    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        let row = Self::entity_to_row(&entity)?;

        let result = sqlx::query_as::<_, EntityRow>(
            "UPDATE entities \
             SET name = $1, status = $2, tenant_id = $3, data = $4, updated_at = $5, deleted_at = $6 \
             WHERE id = $7 AND entity_type = $8 \
             RETURNING *",
        )
        .bind(&row.name)
        .bind(&row.status)
        .bind(row.tenant_id)
        .bind(&row.data)
        .bind(row.updated_at)
        .bind(row.deleted_at)
        .bind(id)
        .bind(Self::entity_type_name())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to update entity: {}", e))?;

        match result {
            Some(r) => Self::row_to_entity(r),
            None => Err(anyhow!("Entity not found: {}", id)),
        }
    }

    /// Delete an entity by UUID.
    ///
    /// Silently succeeds if the entity does not exist (idempotent).
    async fn delete(&self, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM entities WHERE id = $1 AND entity_type = $2")
            .bind(id)
            .bind(Self::entity_type_name())
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to delete entity: {}", e))?;

        Ok(())
    }

    /// Search entities by field value.
    ///
    /// For common fields (`name`, `status`), uses direct column comparison.
    /// For custom fields, uses JSONB text extraction (`data->>field = value`).
    /// All searches are scoped to entity type `T`.
    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        let rows = if SEARCHABLE_COLUMNS.contains(&field) {
            // Search by dedicated column (field name is whitelisted, safe to interpolate)
            let sql = format!(
                "SELECT * FROM entities WHERE entity_type = $1 AND {} = $2",
                field
            );
            sqlx::query_as::<_, EntityRow>(&sql)
                .bind(Self::entity_type_name())
                .bind(value)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| anyhow!("Failed to search entities: {}", e))?
        } else {
            // Search by JSONB field: data->>field_name returns text for comparison
            sqlx::query_as::<_, EntityRow>(
                "SELECT * FROM entities WHERE entity_type = $1 AND data->>$2 = $3",
            )
            .bind(Self::entity_type_name())
            .bind(field)
            .bind(value)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to search entities by JSONB field: {}", e))?
        };

        rows.into_iter().map(Self::row_to_entity).collect()
    }
}

// ---------------------------------------------------------------------------
// LinkRow — intermediate struct for DB row mapping
// ---------------------------------------------------------------------------

/// Database row representation for the `links` table.
#[derive(Debug, FromRow)]
struct LinkRow {
    id: Uuid,
    entity_type: String,
    link_type: String,
    source_id: Uuid,
    target_id: Uuid,
    source_type: Option<String>,
    target_type: Option<String>,
    status: String,
    tenant_id: Option<Uuid>,
    metadata: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl LinkRow {
    /// Convert a `LinkEntity` into a database row.
    fn from_link(link: &LinkEntity) -> Self {
        Self {
            id: link.id,
            entity_type: link.entity_type.clone(),
            link_type: link.link_type.clone(),
            source_id: link.source_id,
            target_id: link.target_id,
            source_type: None, // LinkEntity doesn't carry source_type
            target_type: None, // LinkEntity doesn't carry target_type
            status: link.status.clone(),
            tenant_id: link.tenant_id,
            metadata: link.metadata.clone().unwrap_or(serde_json::json!({})),
            created_at: link.created_at,
            updated_at: link.updated_at,
            deleted_at: link.deleted_at,
        }
    }

    /// Convert a database row back into a `LinkEntity`.
    fn into_link(self) -> LinkEntity {
        LinkEntity {
            id: self.id,
            entity_type: self.entity_type,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
            status: self.status,
            tenant_id: self.tenant_id,
            link_type: self.link_type,
            source_id: self.source_id,
            target_id: self.target_id,
            metadata: if self.metadata == serde_json::json!({}) {
                None
            } else {
                Some(self.metadata)
            },
        }
    }
}

// ---------------------------------------------------------------------------
// PostgresLinkService
// ---------------------------------------------------------------------------

/// Link storage service backed by PostgreSQL.
///
/// Stores links in a `links` table with indexed columns for
/// efficient source/target traversal queries.
///
/// # Example
///
/// ```rust,ignore
/// use sqlx::PgPool;
/// use this::storage::PostgresLinkService;
///
/// let pool = PgPool::connect("postgres://localhost/mydb").await?;
/// let service = PostgresLinkService::new(pool);
/// let link = service.create(my_link).await?;
/// ```
#[derive(Clone, Debug)]
pub struct PostgresLinkService {
    pool: PgPool,
}

impl PostgresLinkService {
    /// Create a new `PostgresLinkService` with the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl LinkService for PostgresLinkService {
    /// Insert a new link into the `links` table.
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let row = LinkRow::from_link(&link);

        let result = sqlx::query_as::<_, LinkRow>(
            "INSERT INTO links (id, entity_type, link_type, source_id, target_id, source_type, target_type, status, tenant_id, metadata, created_at, updated_at, deleted_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             RETURNING *",
        )
        .bind(row.id)
        .bind(&row.entity_type)
        .bind(&row.link_type)
        .bind(row.source_id)
        .bind(row.target_id)
        .bind(&row.source_type)
        .bind(&row.target_type)
        .bind(&row.status)
        .bind(row.tenant_id)
        .bind(&row.metadata)
        .bind(row.created_at)
        .bind(row.updated_at)
        .bind(row.deleted_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to create link: {}", e))?;

        Ok(result.into_link())
    }

    /// Fetch a link by UUID.
    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let row = sqlx::query_as::<_, LinkRow>("SELECT * FROM links WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to get link: {}", e))?;

        Ok(row.map(LinkRow::into_link))
    }

    /// List all links, ordered by creation time (newest first).
    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let rows = sqlx::query_as::<_, LinkRow>("SELECT * FROM links ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to list links: {}", e))?;

        Ok(rows.into_iter().map(LinkRow::into_link).collect())
    }

    /// Find links by source entity, with optional filters.
    ///
    /// Dynamically builds WHERE clauses for link_type filter.
    ///
    /// **Note:** `target_type` is currently ignored because `LinkEntity` does not
    /// carry entity-type metadata — the `target_type` column is always NULL.
    /// This matches the `InMemoryLinkService` behavior. When/if `LinkEntity`
    /// gains a `target_type` field, re-enable the SQL filter here.
    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let mut sql = String::from("SELECT * FROM links WHERE source_id = $1");

        if link_type.is_some() {
            sql.push_str(" AND link_type = $2");
        }
        sql.push_str(" ORDER BY created_at DESC");

        let mut query = sqlx::query_as::<_, LinkRow>(&sql).bind(source_id);

        if let Some(lt) = link_type {
            query = query.bind(lt);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to find links by source: {}", e))?;

        Ok(rows.into_iter().map(LinkRow::into_link).collect())
    }

    /// Find links by target entity, with optional filters.
    ///
    /// Dynamically builds WHERE clauses for link_type filter.
    ///
    /// **Note:** `source_type` is currently ignored because `LinkEntity` does not
    /// carry entity-type metadata — the `source_type` column is always NULL.
    /// This matches the `InMemoryLinkService` behavior. When/if `LinkEntity`
    /// gains a `source_type` field, re-enable the SQL filter here.
    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let mut sql = String::from("SELECT * FROM links WHERE target_id = $1");

        if link_type.is_some() {
            sql.push_str(" AND link_type = $2");
        }
        sql.push_str(" ORDER BY created_at DESC");

        let mut query = sqlx::query_as::<_, LinkRow>(&sql).bind(target_id);

        if let Some(lt) = link_type {
            query = query.bind(lt);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to find links by target: {}", e))?;

        Ok(rows.into_iter().map(LinkRow::into_link).collect())
    }

    /// Update a link's fields.
    ///
    /// Returns `Err` if the link does not exist.
    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        let row = LinkRow::from_link(&link);

        let result = sqlx::query_as::<_, LinkRow>(
            "UPDATE links \
             SET link_type = $1, source_id = $2, target_id = $3, status = $4, \
                 tenant_id = $5, metadata = $6, updated_at = $7, deleted_at = $8 \
             WHERE id = $9 \
             RETURNING *",
        )
        .bind(&row.link_type)
        .bind(row.source_id)
        .bind(row.target_id)
        .bind(&row.status)
        .bind(row.tenant_id)
        .bind(&row.metadata)
        .bind(row.updated_at)
        .bind(row.deleted_at)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to update link: {}", e))?;

        match result {
            Some(r) => Ok(r.into_link()),
            None => Err(anyhow!("Link not found: {}", id)),
        }
    }

    /// Delete a link by UUID.
    ///
    /// Silently succeeds if the link does not exist (idempotent).
    async fn delete(&self, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM links WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to delete link: {}", e))?;

        Ok(())
    }

    /// Delete all links involving a specific entity (as source OR target).
    ///
    /// Uses a single query with OR for efficiency.
    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM links WHERE source_id = $1 OR target_id = $1")
            .bind(entity_id)
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to delete links by entity: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "postgres")]
#[allow(dead_code)]
mod tests {
    use super::*;
    use serde_json::json;

    // Minimal test entity via the impl_data_entity! macro.
    crate::impl_data_entity!(TestOrder, "test_order", ["name"], {
        amount: f64,
    });

    // -----------------------------------------------------------------------
    // entity_to_row
    // -----------------------------------------------------------------------

    #[test]
    fn entity_to_row_strips_common_fields() {
        let order = TestOrder::new("Widget".into(), "active".into(), 42.5);
        let row = PostgresDataService::<TestOrder>::entity_to_row(&order).unwrap();

        let obj = row.data.as_object().expect("data should be a JSON object");
        // Common fields must NOT appear in the JSONB data column
        for field in ENTITY_COMMON_FIELDS {
            assert!(
                !obj.contains_key(*field),
                "data should not contain common field '{field}'"
            );
        }
        // Type-specific field must be preserved
        assert_eq!(obj.get("amount").and_then(|v| v.as_f64()), Some(42.5));
    }

    #[test]
    fn entity_to_row_preserves_entity_type() {
        let order = TestOrder::new("Gadget".into(), "active".into(), 10.0);
        let row = PostgresDataService::<TestOrder>::entity_to_row(&order).unwrap();

        assert_eq!(row.entity_type, "test_order");
    }

    // -----------------------------------------------------------------------
    // row_to_entity (roundtrip)
    // -----------------------------------------------------------------------

    #[test]
    fn row_to_entity_roundtrip() {
        let order = TestOrder::new("Roundtrip".into(), "pending".into(), 99.99);
        let original_id = order.id;
        let original_created = order.created_at;
        let original_updated = order.updated_at;

        let row = PostgresDataService::<TestOrder>::entity_to_row(&order).unwrap();
        let restored = PostgresDataService::<TestOrder>::row_to_entity(row).unwrap();

        assert_eq!(restored.id, original_id);
        assert_eq!(restored.name, "Roundtrip");
        assert_eq!(restored.status, "pending");
        assert_eq!(restored.amount, 99.99);
        assert_eq!(restored.created_at, original_created);
        assert_eq!(restored.updated_at, original_updated);
        assert!(restored.deleted_at.is_none());
    }

    #[test]
    fn row_to_entity_non_object_data_handled() {
        // When the JSONB `data` column is not an object (e.g. null),
        // row_to_entity should default to an empty object and still
        // succeed if the type-specific fields have defaults / are present.
        let now = Utc::now();
        let id = Uuid::new_v4();

        // Provide the required `amount` field so deserialization can succeed
        // even though the outer value started as null -> {}.
        let row_with_amount = EntityRow {
            id,
            entity_type: "test_order".into(),
            name: "NullData".into(),
            status: "active".into(),
            tenant_id: None,
            data: json!({ "amount": 7.5 }),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        let entity = PostgresDataService::<TestOrder>::row_to_entity(row_with_amount).unwrap();
        assert_eq!(entity.id, id);
        assert_eq!(entity.name, "NullData");

        // When data is truly null (missing custom fields), the fallback to {}
        // still happens but deserialization returns a descriptive error (not a panic).
        let row_null = EntityRow {
            id,
            entity_type: "test_order".into(),
            name: "NullData".into(),
            status: "active".into(),
            tenant_id: None,
            data: json!(null),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        let err = PostgresDataService::<TestOrder>::row_to_entity(row_null).unwrap_err();
        assert!(
            err.to_string().contains("deserialize"),
            "error should mention deserialization: {}",
            err
        );
    }

    #[test]
    fn row_to_entity_entity_type_fallback() {
        // When entity_type / type are NOT in the JSONB data, row_to_entity
        // should inject them from the row's entity_type column.
        let now = Utc::now();
        let id = Uuid::new_v4();

        let row = EntityRow {
            id,
            entity_type: "test_order".into(),
            name: "Fallback".into(),
            status: "active".into(),
            tenant_id: None,
            data: json!({ "amount": 1.0 }), // no entity_type / type key
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };

        let entity = PostgresDataService::<TestOrder>::row_to_entity(row).unwrap();
        assert_eq!(entity.entity_type, "test_order");
    }

    // -----------------------------------------------------------------------
    // LinkRow conversions
    // -----------------------------------------------------------------------

    fn make_link() -> LinkEntity {
        let now = Utc::now();
        LinkEntity {
            id: Uuid::new_v4(),
            entity_type: "ownership".into(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".into(),
            tenant_id: Some(Uuid::new_v4()),
            link_type: "owns".into(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: Some(json!({"priority": "high"})),
        }
    }

    #[test]
    fn link_row_from_link_preserves_fields() {
        let link = make_link();
        let row = LinkRow::from_link(&link);

        assert_eq!(row.id, link.id);
        assert_eq!(row.entity_type, link.entity_type);
        assert_eq!(row.link_type, link.link_type);
        assert_eq!(row.source_id, link.source_id);
        assert_eq!(row.target_id, link.target_id);
        assert_eq!(row.status, link.status);
        assert_eq!(row.tenant_id, link.tenant_id);
        assert_eq!(row.created_at, link.created_at);
        assert_eq!(row.updated_at, link.updated_at);
        assert_eq!(row.deleted_at, link.deleted_at);
        // metadata: Some({...}) -> stored as the inner value
        assert_eq!(row.metadata, json!({"priority": "high"}));
        // source_type / target_type are always None
        assert!(row.source_type.is_none());
        assert!(row.target_type.is_none());
    }

    #[test]
    fn link_row_into_link_roundtrip() {
        let original = make_link();
        let row = LinkRow::from_link(&original);
        let restored = row.into_link();

        assert_eq!(restored.id, original.id);
        assert_eq!(restored.entity_type, original.entity_type);
        assert_eq!(restored.link_type, original.link_type);
        assert_eq!(restored.source_id, original.source_id);
        assert_eq!(restored.target_id, original.target_id);
        assert_eq!(restored.status, original.status);
        assert_eq!(restored.tenant_id, original.tenant_id);
        assert_eq!(restored.created_at, original.created_at);
        assert_eq!(restored.updated_at, original.updated_at);
        assert_eq!(restored.deleted_at, original.deleted_at);
        assert_eq!(restored.metadata, original.metadata);
    }

    #[test]
    fn link_row_into_link_empty_metadata_becomes_none() {
        let mut link = make_link();
        link.metadata = None; // from_link will store json!({})

        let row = LinkRow::from_link(&link);
        assert_eq!(
            row.metadata,
            json!({}),
            "None metadata stored as empty object"
        );

        let restored = row.into_link();
        assert_eq!(restored.metadata, None, "empty object should become None");
    }

    #[test]
    fn link_row_into_link_with_metadata() {
        let mut link = make_link();
        link.metadata = Some(json!({"key": "val"}));

        let row = LinkRow::from_link(&link);
        let restored = row.into_link();

        assert_eq!(
            restored.metadata,
            Some(json!({"key": "val"})),
            "non-empty metadata should survive roundtrip"
        );
    }
}
