//! MySQL storage backend using sqlx.
//!
//! Provides `MysqlDataService<T>` and `MysqlLinkService` implementations
//! backed by a MySQL database via `sqlx::MySqlPool`.
//!
//! # Feature flag
//!
//! This module is gated behind the `mysql` feature flag:
//! ```toml
//! [dependencies]
//! this-rs = { version = "0.0.7", features = ["mysql"] }
//! ```
//!
//! # Schema
//!
//! Entities are stored in a shared `entities` table with common columns
//! (id, entity_type, name, status, timestamps) and a JSON `data` column
//! for type-specific fields. Links are stored in a `links` table.
//!
//! # Differences from PostgreSQL backend
//!
//! - UUID stored as `CHAR(36)` (not native UUID type)
//! - JSON column instead of JSONB (no GIN index)
//! - `?` placeholders instead of `$1`, `$2`
//! - No `RETURNING *` — uses SELECT after INSERT/UPDATE
//! - `JSON_EXTRACT(data, '$.field')` instead of `data->>field`
//! - `DATETIME(6)` instead of `TIMESTAMPTZ`

use crate::core::link::LinkEntity;
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::MySqlPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Schema management
// ---------------------------------------------------------------------------

/// Apply the required tables and indexes (idempotent).
///
/// This creates:
/// - `entities` table with common columns + JSON data column
/// - `links` table with indexed source/target columns
///
/// Safe to call on every startup.
pub async fn ensure_schema(pool: &MySqlPool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS entities (
            id CHAR(36) NOT NULL PRIMARY KEY,
            entity_type VARCHAR(255) NOT NULL,
            name VARCHAR(255) NOT NULL DEFAULT '',
            status VARCHAR(50) NOT NULL DEFAULT '',
            tenant_id CHAR(36) NULL,
            data JSON,
            created_at DATETIME(6) NOT NULL,
            updated_at DATETIME(6) NOT NULL,
            deleted_at DATETIME(6) NULL,
            INDEX idx_entity_type (entity_type),
            INDEX idx_name (name)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| anyhow!("Failed to create entities table: {}", e))?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS links (
            id CHAR(36) NOT NULL PRIMARY KEY,
            entity_type VARCHAR(255) NOT NULL DEFAULT '',
            link_type VARCHAR(255) NOT NULL,
            source_id CHAR(36) NOT NULL,
            target_id CHAR(36) NOT NULL,
            source_type VARCHAR(255) NULL,
            target_type VARCHAR(255) NULL,
            status VARCHAR(50) NOT NULL DEFAULT '',
            tenant_id CHAR(36) NULL,
            metadata JSON,
            created_at DATETIME(6) NOT NULL,
            updated_at DATETIME(6) NOT NULL,
            deleted_at DATETIME(6) NULL,
            INDEX idx_source (source_id, link_type),
            INDEX idx_target (target_id, link_type)
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| anyhow!("Failed to create links table: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Common field definitions
// ---------------------------------------------------------------------------

/// Common entity fields stored in dedicated columns (excluded from JSON data).
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
const SEARCHABLE_COLUMNS: &[&str] = &["name", "status"];

// ---------------------------------------------------------------------------
// MysqlDataService<T>
// ---------------------------------------------------------------------------

/// Generic data storage service backed by MySQL.
///
/// Stores entities in a shared `entities` table with common columns
/// (id, entity_type, name, status, timestamps) and a JSON `data`
/// column for type-specific fields.
///
/// # Example
///
/// ```rust,ignore
/// use sqlx::MySqlPool;
/// use this::storage::MysqlDataService;
///
/// let pool = MySqlPool::connect("mysql://root:password@localhost/mydb").await?;
/// let service = MysqlDataService::<MyEntity>::new(pool);
/// let entity = service.create(my_entity).await?;
/// ```
#[derive(Clone, Debug)]
pub struct MysqlDataService<T> {
    pool: MySqlPool,
    _marker: std::marker::PhantomData<T>,
}

impl<T> MysqlDataService<T> {
    pub fn new(pool: MySqlPool) -> Self {
        Self {
            pool,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }
}

impl<T: Data + Serialize + DeserializeOwned> MysqlDataService<T> {
    fn entity_type_name() -> &'static str {
        T::resource_name_singular()
    }

    /// Convert a domain entity into column values for INSERT/UPDATE.
    ///
    /// Serializes the full entity to JSON, extracts common fields into
    /// dedicated columns, and stores remaining fields in the JSON `data` column.
    fn extract_data(entity: &T) -> Result<serde_json::Value> {
        let mut data = serde_json::to_value(entity)
            .map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;

        // Remove common fields from data (they're stored in dedicated columns)
        if let Some(obj) = data.as_object_mut() {
            for field in ENTITY_COMMON_FIELDS {
                obj.remove(*field);
            }
        }

        Ok(data)
    }

    /// Reconstruct a domain entity from a row's columns.
    ///
    /// Merges common columns back into the JSON data, then deserializes
    /// the combined JSON into the target type `T`.
    fn reconstruct_entity(
        id: String,
        entity_type: String,
        name: String,
        status: String,
        tenant_id: Option<String>,
        data: serde_json::Value,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        deleted_at: Option<DateTime<Utc>>,
    ) -> Result<T> {
        let mut json = if data.is_object() {
            data
        } else {
            serde_json::json!({})
        };

        if let Some(obj) = json.as_object_mut() {
            obj.insert("id".into(), serde_json::json!(id));
            if !obj.contains_key("entity_type") {
                obj.insert("entity_type".into(), serde_json::json!(entity_type));
            }
            if !obj.contains_key("type") {
                obj.insert("type".into(), serde_json::json!(entity_type));
            }
            obj.insert("name".into(), serde_json::json!(name));
            obj.insert("status".into(), serde_json::json!(status));
            obj.insert(
                "created_at".into(),
                serde_json::to_value(created_at)?,
            );
            obj.insert(
                "updated_at".into(),
                serde_json::to_value(updated_at)?,
            );
            obj.insert(
                "deleted_at".into(),
                serde_json::to_value(deleted_at)?,
            );
            if let Some(tid) = tenant_id {
                obj.insert("tenant_id".into(), serde_json::json!(tid));
            }
        }

        serde_json::from_value::<T>(json)
            .map_err(|e| anyhow!("Failed to deserialize entity from row: {}", e))
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> DataService<T> for MysqlDataService<T> {
    async fn create(&self, entity: T) -> Result<T> {
        let data = Self::extract_data(&entity)?;
        let id = entity.id().to_string();
        let entity_type = Self::entity_type_name().to_string();
        let name = entity.name().to_string();
        let status = entity.status().to_string();
        let tenant_id = entity.tenant_id().map(|u| u.to_string());
        let created_at = entity.created_at();
        let updated_at = entity.updated_at();
        let deleted_at = entity.deleted_at();

        sqlx::query(
            "INSERT INTO entities (id, entity_type, name, status, tenant_id, data, created_at, updated_at, deleted_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&entity_type)
        .bind(&name)
        .bind(&status)
        .bind(&tenant_id)
        .bind(&data)
        .bind(created_at)
        .bind(updated_at)
        .bind(deleted_at)
        .execute(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to create entity: {}", e))?;

        // MySQL doesn't support RETURNING — re-read the entity
        self.get(&entity.id())
            .await?
            .ok_or_else(|| anyhow!("Failed to read back created entity"))
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let row = sqlx::query_as::<_, (String, String, String, String, Option<String>, serde_json::Value, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>)>(
            "SELECT id, entity_type, name, status, tenant_id, data, created_at, updated_at, deleted_at \
             FROM entities WHERE id = ? AND entity_type = ?",
        )
        .bind(id.to_string())
        .bind(Self::entity_type_name())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to get entity: {}", e))?;

        match row {
            Some((id, etype, name, status, tid, data, cat, uat, dat)) => {
                Ok(Some(Self::reconstruct_entity(id, etype, name, status, tid, data, cat, uat, dat)?))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<T>> {
        let rows = sqlx::query_as::<_, (String, String, String, String, Option<String>, serde_json::Value, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>)>(
            "SELECT id, entity_type, name, status, tenant_id, data, created_at, updated_at, deleted_at \
             FROM entities WHERE entity_type = ? ORDER BY created_at DESC",
        )
        .bind(Self::entity_type_name())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to list entities: {}", e))?;

        rows.into_iter()
            .map(|(id, etype, name, status, tid, data, cat, uat, dat)| {
                Self::reconstruct_entity(id, etype, name, status, tid, data, cat, uat, dat)
            })
            .collect()
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        let data = Self::extract_data(&entity)?;
        let name = entity.name().to_string();
        let status = entity.status().to_string();
        let tenant_id = entity.tenant_id().map(|u| u.to_string());
        let updated_at = entity.updated_at();
        let deleted_at = entity.deleted_at();

        let result = sqlx::query(
            "UPDATE entities \
             SET name = ?, status = ?, tenant_id = ?, data = ?, updated_at = ?, deleted_at = ? \
             WHERE id = ? AND entity_type = ?",
        )
        .bind(&name)
        .bind(&status)
        .bind(&tenant_id)
        .bind(&data)
        .bind(updated_at)
        .bind(deleted_at)
        .bind(id.to_string())
        .bind(Self::entity_type_name())
        .execute(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to update entity: {}", e))?;

        if result.rows_affected() == 0 {
            return Err(anyhow!("Entity not found: {}", id));
        }

        // Re-read the entity
        self.get(id)
            .await?
            .ok_or_else(|| anyhow!("Failed to read back updated entity"))
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM entities WHERE id = ? AND entity_type = ?")
            .bind(id.to_string())
            .bind(Self::entity_type_name())
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to delete entity: {}", e))?;

        Ok(())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        let rows = if SEARCHABLE_COLUMNS.contains(&field) {
            // Direct column search (field name is whitelisted, safe to interpolate)
            let sql = format!(
                "SELECT id, entity_type, name, status, tenant_id, data, created_at, updated_at, deleted_at \
                 FROM entities WHERE entity_type = ? AND {} = ?",
                field
            );
            sqlx::query_as::<_, (String, String, String, String, Option<String>, serde_json::Value, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>)>(&sql)
                .bind(Self::entity_type_name())
                .bind(value)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| anyhow!("Failed to search entities: {}", e))?
        } else {
            // Search in JSON data column using JSON_EXTRACT
            // JSON_UNQUOTE(JSON_EXTRACT(data, '$.field')) returns the text value
            let json_path = format!("$.{}", field);
            sqlx::query_as::<_, (String, String, String, String, Option<String>, serde_json::Value, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>)>(
                "SELECT id, entity_type, name, status, tenant_id, data, created_at, updated_at, deleted_at \
                 FROM entities WHERE entity_type = ? AND JSON_UNQUOTE(JSON_EXTRACT(data, ?)) = ?",
            )
            .bind(Self::entity_type_name())
            .bind(&json_path)
            .bind(value)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to search entities by JSON field: {}", e))?
        };

        rows.into_iter()
            .map(|(id, etype, name, status, tid, data, cat, uat, dat)| {
                Self::reconstruct_entity(id, etype, name, status, tid, data, cat, uat, dat)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// MysqlLinkService
// ---------------------------------------------------------------------------

/// Link storage service backed by MySQL.
///
/// Stores links in a `links` table with indexed columns for
/// efficient source/target traversal queries.
///
/// # Example
///
/// ```rust,ignore
/// use sqlx::MySqlPool;
/// use this::storage::MysqlLinkService;
///
/// let pool = MySqlPool::connect("mysql://root:password@localhost/mydb").await?;
/// let service = MysqlLinkService::new(pool);
/// let link = service.create(my_link).await?;
/// ```
#[derive(Clone, Debug)]
pub struct MysqlLinkService {
    pool: MySqlPool,
}

impl MysqlLinkService {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    /// Parse a link row tuple into a LinkEntity.
    fn row_to_link(
        id: String,
        entity_type: String,
        link_type: String,
        source_id: String,
        target_id: String,
        _source_type: Option<String>,
        _target_type: Option<String>,
        status: String,
        tenant_id: Option<String>,
        metadata: serde_json::Value,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        deleted_at: Option<DateTime<Utc>>,
    ) -> Result<LinkEntity> {
        Ok(LinkEntity {
            id: id
                .parse()
                .map_err(|e| anyhow!("Invalid UUID for link id: {}", e))?,
            entity_type,
            created_at,
            updated_at,
            deleted_at,
            status,
            tenant_id: tenant_id
                .and_then(|t| t.parse().ok()),
            link_type,
            source_id: source_id
                .parse()
                .map_err(|e| anyhow!("Invalid UUID for source_id: {}", e))?,
            target_id: target_id
                .parse()
                .map_err(|e| anyhow!("Invalid UUID for target_id: {}", e))?,
            metadata: if metadata == serde_json::json!({}) {
                None
            } else {
                Some(metadata)
            },
        })
    }
}

type LinkTuple = (String, String, String, String, String, Option<String>, Option<String>, String, Option<String>, serde_json::Value, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>);

const LINK_SELECT: &str = "SELECT id, entity_type, link_type, source_id, target_id, source_type, target_type, status, tenant_id, metadata, created_at, updated_at, deleted_at FROM links";

#[async_trait]
impl LinkService for MysqlLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let metadata = link.metadata.clone().unwrap_or(serde_json::json!({}));

        sqlx::query(
            "INSERT INTO links (id, entity_type, link_type, source_id, target_id, source_type, target_type, status, tenant_id, metadata, created_at, updated_at, deleted_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(link.id.to_string())
        .bind(&link.entity_type)
        .bind(&link.link_type)
        .bind(link.source_id.to_string())
        .bind(link.target_id.to_string())
        .bind(None::<String>)  // source_type
        .bind(None::<String>)  // target_type
        .bind(&link.status)
        .bind(link.tenant_id.map(|u| u.to_string()))
        .bind(&metadata)
        .bind(link.created_at)
        .bind(link.updated_at)
        .bind(link.deleted_at)
        .execute(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to create link: {}", e))?;

        // Re-read
        self.get(&link.id)
            .await?
            .ok_or_else(|| anyhow!("Failed to read back created link"))
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let sql = format!("{} WHERE id = ?", LINK_SELECT);
        let row = sqlx::query_as::<_, LinkTuple>(&sql)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to get link: {}", e))?;

        match row {
            Some((id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)) => {
                Ok(Some(Self::row_to_link(id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)?))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let sql = format!("{} ORDER BY created_at DESC", LINK_SELECT);
        let rows = sqlx::query_as::<_, LinkTuple>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to list links: {}", e))?;

        rows.into_iter()
            .map(|(id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)| {
                Self::row_to_link(id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)
            })
            .collect()
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let mut sql = format!("{} WHERE source_id = ?", LINK_SELECT);
        if link_type.is_some() {
            sql.push_str(" AND link_type = ?");
        }
        sql.push_str(" ORDER BY created_at DESC");

        let mut query = sqlx::query_as::<_, LinkTuple>(&sql)
            .bind(source_id.to_string());

        if let Some(lt) = link_type {
            query = query.bind(lt);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to find links by source: {}", e))?;

        rows.into_iter()
            .map(|(id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)| {
                Self::row_to_link(id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)
            })
            .collect()
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let mut sql = format!("{} WHERE target_id = ?", LINK_SELECT);
        if link_type.is_some() {
            sql.push_str(" AND link_type = ?");
        }
        sql.push_str(" ORDER BY created_at DESC");

        let mut query = sqlx::query_as::<_, LinkTuple>(&sql)
            .bind(target_id.to_string());

        if let Some(lt) = link_type {
            query = query.bind(lt);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to find links by target: {}", e))?;

        rows.into_iter()
            .map(|(id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)| {
                Self::row_to_link(id, etype, lt, sid, tid, st, tt, status, tenant, meta, cat, uat, dat)
            })
            .collect()
    }

    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        let metadata = link.metadata.clone().unwrap_or(serde_json::json!({}));

        let result = sqlx::query(
            "UPDATE links \
             SET link_type = ?, source_id = ?, target_id = ?, status = ?, \
                 tenant_id = ?, metadata = ?, updated_at = ?, deleted_at = ? \
             WHERE id = ?",
        )
        .bind(&link.link_type)
        .bind(link.source_id.to_string())
        .bind(link.target_id.to_string())
        .bind(&link.status)
        .bind(link.tenant_id.map(|u| u.to_string()))
        .bind(&metadata)
        .bind(link.updated_at)
        .bind(link.deleted_at)
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| anyhow!("Failed to update link: {}", e))?;

        if result.rows_affected() == 0 {
            return Err(anyhow!("Link not found: {}", id));
        }

        self.get(id)
            .await?
            .ok_or_else(|| anyhow!("Failed to read back updated link"))
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM links WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to delete link: {}", e))?;

        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let eid = entity_id.to_string();
        sqlx::query("DELETE FROM links WHERE source_id = ? OR target_id = ?")
            .bind(&eid)
            .bind(&eid)
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to delete links by entity: {}", e))?;

        Ok(())
    }
}
