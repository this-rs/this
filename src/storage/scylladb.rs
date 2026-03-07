//! ScyllaDB storage backend using the scylla-rust CQL driver.
//!
//! Provides `ScyllaDataService<T>` and `ScyllaLinkService` implementations
//! backed by a ScyllaDB/Cassandra cluster via `scylla::Session`.
//!
//! # Feature flag
//!
//! This module is gated behind the `scylladb` feature flag:
//! ```toml
//! [dependencies]
//! this-rs = { version = "0.0.7", features = ["scylladb"] }
//! ```
//!
//! # Storage model
//!
//! Entities are stored in an `entities` table with `(entity_type, id)` as
//! composite primary key. A `entity_data` column holds the full JSON string for
//! reliable deserialization. All scalar fields are also stored as individual
//! columns for CQL-level querying.
//!
//! Links are stored in a single `links` table with `id` as primary key.
//! Secondary indexes on `source_id` and `target_id` enable efficient
//! `find_by_source` and `find_by_target` queries.

use crate::core::field::FieldValue;
use crate::core::link::LinkEntity;
use crate::core::module::{EntityCreator, EntityFetcher};
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use scylla::client::session::Session;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Schema management
// ---------------------------------------------------------------------------

/// Default keyspace name used when none is specified.
const DEFAULT_KEYSPACE: &str = "this_rs";

/// Create the keyspace and tables if they don't exist.
///
/// This is idempotent — safe to call on every startup.
pub async fn ensure_schema(session: &Session, keyspace: &str) -> Result<()> {
    // Create keyspace with SimpleStrategy (suitable for dev/single-node).
    // Production deployments should pre-create the keyspace with NetworkTopologyStrategy.
    let create_ks = format!(
        "CREATE KEYSPACE IF NOT EXISTS {} WITH replication = \
         {{'class': 'SimpleStrategy', 'replication_factor': 1}}",
        keyspace
    );
    session
        .query_unpaged(create_ks, ())
        .await
        .map_err(|e| anyhow!("Failed to create keyspace: {}", e))?;

    // Entities table: partition by entity_type, cluster by id
    let create_entities = format!(
        "CREATE TABLE IF NOT EXISTS {}.entities (\
            entity_type TEXT, \
            id TEXT, \
            name TEXT, \
            status TEXT, \
            entity_data TEXT, \
            created_at TEXT, \
            updated_at TEXT, \
            PRIMARY KEY ((entity_type), id)\
        )",
        keyspace
    );
    session
        .query_unpaged(create_entities, ())
        .await
        .map_err(|e| anyhow!("Failed to create entities table: {}", e))?;

    // Links table: id as primary key
    let create_links = format!(
        "CREATE TABLE IF NOT EXISTS {}.links (\
            id TEXT PRIMARY KEY, \
            entity_type TEXT, \
            source_id TEXT, \
            target_id TEXT, \
            link_type TEXT, \
            source_type TEXT, \
            target_type TEXT, \
            status TEXT, \
            entity_data TEXT, \
            created_at TEXT, \
            updated_at TEXT\
        )",
        keyspace
    );
    session
        .query_unpaged(create_links, ())
        .await
        .map_err(|e| anyhow!("Failed to create links table: {}", e))?;

    // Secondary indexes for efficient link queries
    let idx_source = format!(
        "CREATE INDEX IF NOT EXISTS ON {}.links (source_id)",
        keyspace
    );
    let idx_target = format!(
        "CREATE INDEX IF NOT EXISTS ON {}.links (target_id)",
        keyspace
    );
    let idx_name = format!("CREATE INDEX IF NOT EXISTS ON {}.entities (name)", keyspace);

    for idx in [&idx_source, &idx_target, &idx_name] {
        session
            .query_unpaged(idx.clone(), ())
            .await
            .map_err(|e| anyhow!("Failed to create index: {}", e))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// ScyllaDataService<T>
// ---------------------------------------------------------------------------

/// Generic data storage service backed by ScyllaDB.
///
/// Each entity type is stored in the `entities` table, partitioned by
/// `entity_type` with `id` as clustering key.
///
/// # Example
///
/// ```rust,ignore
/// use scylla::SessionBuilder;
/// use this::storage::ScyllaDataService;
///
/// let session = SessionBuilder::new().known_node("127.0.0.1:9042").build().await?;
/// let service = ScyllaDataService::<MyEntity>::new(session, "my_keyspace");
/// ```
#[derive(Clone)]
pub struct ScyllaDataService<T> {
    session: Arc<Session>,
    keyspace: String,
    _marker: std::marker::PhantomData<T>,
}

impl<T> ScyllaDataService<T> {
    pub fn new(session: Arc<Session>, keyspace: impl Into<String>) -> Self {
        Self {
            session,
            keyspace: keyspace.into(),
            _marker: std::marker::PhantomData,
        }
    }

    /// Create with the default keyspace name.
    pub fn with_default_keyspace(session: Arc<Session>) -> Self {
        Self::new(session, DEFAULT_KEYSPACE)
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
}

impl<T: Data + Serialize + DeserializeOwned> ScyllaDataService<T> {
    fn entity_type_name() -> &'static str {
        T::resource_name_singular()
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> DataService<T> for ScyllaDataService<T> {
    async fn create(&self, entity: T) -> Result<T> {
        let json_str = serde_json::to_string(&entity)
            .map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;
        let json_val: serde_json::Value = serde_json::to_value(&entity)
            .map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;

        let id = entity.id().to_string();
        let name = json_val
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = json_val
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let created_at = json_val
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let updated_at = json_val
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let entity_type = Self::entity_type_name().to_string();

        let cql = format!(
            "INSERT INTO {}.entities (entity_type, id, name, status, entity_data, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            self.keyspace
        );

        self.session
            .query_unpaged(
                cql,
                (
                    &entity_type,
                    &id,
                    &name,
                    &status,
                    &json_str,
                    &created_at,
                    &updated_at,
                ),
            )
            .await
            .map_err(|e| anyhow!("Failed to create entity: {}", e))?;

        Ok(entity)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let cql = format!(
            "SELECT entity_data FROM {}.entities WHERE entity_type = ? AND id = ?",
            self.keyspace
        );

        let result = self
            .session
            .query_unpaged(cql, (Self::entity_type_name(), id.to_string().as_str()))
            .await
            .map_err(|e| anyhow!("Failed to get entity: {}", e))?;

        let rows_result = result
            .into_rows_result()
            .map_err(|e| anyhow!("Failed to parse result: {}", e))?;

        let rows: Vec<(String,)> = rows_result
            .rows()
            .map_err(|e| anyhow!("Failed to deserialize rows: {}", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to collect rows: {}", e))?;

        match rows.first() {
            Some((data,)) => {
                let entity: T = serde_json::from_str(data)
                    .map_err(|e| anyhow!("Failed to deserialize entity: {}", e))?;
                Ok(Some(entity))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<T>> {
        let cql = format!(
            "SELECT entity_data FROM {}.entities WHERE entity_type = ?",
            self.keyspace
        );

        let result = self
            .session
            .query_unpaged(cql, (Self::entity_type_name(),))
            .await
            .map_err(|e| anyhow!("Failed to list entities: {}", e))?;

        let rows_result = result
            .into_rows_result()
            .map_err(|e| anyhow!("Failed to parse result: {}", e))?;

        let rows: Vec<(String,)> = rows_result
            .rows()
            .map_err(|e| anyhow!("Failed to deserialize rows: {}", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to collect rows: {}", e))?;

        let mut entities = Vec::new();
        for (data,) in &rows {
            let entity: T = serde_json::from_str(data)
                .map_err(|e| anyhow!("Failed to deserialize entity: {}", e))?;
            entities.push(entity);
        }

        // Sort by created_at DESC (CQL doesn't support ORDER BY on non-clustering columns)
        entities.sort_by_key(|b| std::cmp::Reverse(b.created_at()));

        Ok(entities)
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        // Verify entity exists first
        let existing = self.get(id).await?;
        if existing.is_none() {
            return Err(anyhow!("Entity not found: {}", id));
        }

        let json_str = serde_json::to_string(&entity)
            .map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;
        let json_val: serde_json::Value = serde_json::to_value(&entity)
            .map_err(|e| anyhow!("Failed to serialize entity: {}", e))?;

        let name = json_val
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = json_val
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let updated_at = json_val
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let cql = format!(
            "UPDATE {}.entities SET name = ?, status = ?, entity_data = ?, updated_at = ? \
             WHERE entity_type = ? AND id = ?",
            self.keyspace
        );

        self.session
            .query_unpaged(
                cql,
                (
                    &name,
                    &status,
                    &json_str,
                    &updated_at,
                    Self::entity_type_name(),
                    id.to_string().as_str(),
                ),
            )
            .await
            .map_err(|e| anyhow!("Failed to update entity: {}", e))?;

        Ok(entity)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let cql = format!(
            "DELETE FROM {}.entities WHERE entity_type = ? AND id = ?",
            self.keyspace
        );

        self.session
            .query_unpaged(cql, (Self::entity_type_name(), id.to_string().as_str()))
            .await
            .map_err(|e| anyhow!("Failed to delete entity: {}", e))?;

        Ok(())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        // For "name" field, we can use the secondary index directly
        if field == "name" {
            let cql = format!(
                "SELECT entity_data FROM {}.entities WHERE entity_type = ? AND name = ? ALLOW FILTERING",
                self.keyspace
            );

            let result = self
                .session
                .query_unpaged(cql, (Self::entity_type_name(), value))
                .await
                .map_err(|e| anyhow!("Failed to search entities: {}", e))?;

            let rows_result = result
                .into_rows_result()
                .map_err(|e| anyhow!("Failed to parse result: {}", e))?;

            let rows: Vec<(String,)> = rows_result
                .rows()
                .map_err(|e| anyhow!("Failed to deserialize rows: {}", e))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| anyhow!("Failed to collect rows: {}", e))?;

            let mut entities = Vec::new();
            for (data,) in &rows {
                let entity: T = serde_json::from_str(data)
                    .map_err(|e| anyhow!("Failed to deserialize entity: {}", e))?;
                entities.push(entity);
            }
            return Ok(entities);
        }

        // For other fields, load all entities and filter client-side via entity_data JSON.
        // ScyllaDB doesn't support arbitrary field queries efficiently.
        let all = self.list().await?;
        let results = all
            .into_iter()
            .filter(|entity| {
                entity.field_value(field).is_some_and(|fv| match &fv {
                    FieldValue::String(s) => s == value,
                    FieldValue::Integer(i) => i.to_string() == value,
                    FieldValue::Float(f) => f.to_string() == value,
                    FieldValue::Boolean(b) => b.to_string() == value,
                    FieldValue::Uuid(u) => u.to_string() == value,
                    FieldValue::DateTime(dt) => dt.to_rfc3339() == value,
                    FieldValue::Null => false,
                })
            })
            .collect();

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// JSON number normalization (protobuf Struct sends ALL numbers as f64)
// ---------------------------------------------------------------------------

/// Recursively walk a `serde_json::Value` and convert any `f64` that has no
/// fractional part (e.g. `3549883.0`) into an `i64`.  This is necessary
/// because `google.protobuf.Struct` only has `number_value` (double) — there
/// is no integer type.  Without this normalisation, `serde_json::from_value`
/// rejects `3549883.0` when the target Rust field is `i64` / `Option<i64>`.
fn normalize_json_numbers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                // Only convert if the value has no fractional part AND fits in i64
                if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    #[allow(clippy::cast_possible_truncation)]
                    let i = f as i64;
                    if let Some(int_val) = serde_json::Number::from_f64(i as f64) {
                        // Use from_i64 which is infallible for valid i64
                        *n = serde_json::Number::from(i);
                        let _ = int_val; // suppress unused warning
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                normalize_json_numbers(item);
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values_mut() {
                normalize_json_numbers(val);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Generic EntityFetcher / EntityCreator for ScyllaDataService<T>
// ---------------------------------------------------------------------------

#[async_trait]
#[allow(clippy::cast_sign_loss)]
impl<T: Data + Serialize + DeserializeOwned> EntityFetcher for ScyllaDataService<T> {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let entity = DataService::get(self, entity_id)
            .await?
            .ok_or_else(|| anyhow!("{} not found: {}", Self::entity_type_name(), entity_id))?;
        serde_json::to_value(entity).map_err(|e| anyhow!("Failed to serialize entity: {}", e))
    }

    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        let all = DataService::list(self).await?;
        let offset = offset.unwrap_or(0).max(0) as usize;
        let limit = limit.unwrap_or(50).max(0) as usize;

        all.into_iter()
            .skip(offset)
            .take(limit)
            .map(|e| serde_json::to_value(e).map_err(|err| anyhow!("serialize: {}", err)))
            .collect()
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> EntityCreator for ScyllaDataService<T> {
    async fn create_from_json(&self, mut data: serde_json::Value) -> Result<serde_json::Value> {
        // Inject system fields if missing so the entity can be deserialized.
        // The macro-generated structs use #[serde(rename = "type")] for entity_type.
        if let Some(obj) = data.as_object_mut() {
            if !obj.contains_key("id") {
                obj.insert(
                    "id".to_string(),
                    serde_json::to_value(Uuid::new_v4())?,
                );
            }
            if !obj.contains_key("type") {
                obj.insert(
                    "type".to_string(),
                    serde_json::Value::String(Self::entity_type_name().to_string()),
                );
            }
            let now = chrono::Utc::now();
            if !obj.contains_key("created_at") {
                obj.insert("created_at".to_string(), serde_json::to_value(now)?);
            }
            if !obj.contains_key("updated_at") {
                obj.insert("updated_at".to_string(), serde_json::to_value(now)?);
            }
            if !obj.contains_key("status") {
                obj.insert(
                    "status".to_string(),
                    serde_json::Value::String("active".to_string()),
                );
            }
        }

        // Normalise floats that are actually integers (protobuf Struct quirk)
        normalize_json_numbers(&mut data);

        let entity: T = serde_json::from_value(data)
            .map_err(|e| anyhow!("Failed to deserialize {}: {}", Self::entity_type_name(), e))?;
        let created = DataService::create(self, entity).await?;
        serde_json::to_value(created).map_err(|e| anyhow!("Failed to serialize: {}", e))
    }

    async fn update_from_json(
        &self,
        entity_id: &Uuid,
        data: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let existing = DataService::get(self, entity_id)
            .await?
            .ok_or_else(|| anyhow!("{} not found: {}", Self::entity_type_name(), entity_id))?;

        let mut existing_json = serde_json::to_value(&existing)?;

        // Merge update fields into existing entity
        if let (Some(existing_obj), Some(update_obj)) =
            (existing_json.as_object_mut(), data.as_object())
        {
            for (key, value) in update_obj {
                existing_obj.insert(key.clone(), value.clone());
            }
            // Always bump updated_at
            existing_obj.insert(
                "updated_at".to_string(),
                serde_json::to_value(chrono::Utc::now())?,
            );
        }

        // Normalise floats that are actually integers (protobuf Struct quirk)
        normalize_json_numbers(&mut existing_json);

        let updated: T = serde_json::from_value(existing_json)
            .map_err(|e| anyhow!("Failed to deserialize {}: {}", Self::entity_type_name(), e))?;
        let result = DataService::update(self, entity_id, updated).await?;
        serde_json::to_value(result).map_err(|e| anyhow!("Failed to serialize: {}", e))
    }

    async fn delete(&self, entity_id: &Uuid) -> Result<()> {
        DataService::delete(self, entity_id).await
    }
}

// ---------------------------------------------------------------------------
// ScyllaLinkService
// ---------------------------------------------------------------------------

/// Link storage service backed by ScyllaDB.
///
/// Links are stored in a single `links` table with secondary indexes on
/// `source_id` and `target_id` for efficient directional queries.
///
/// # Example
///
/// ```rust,ignore
/// use scylla::SessionBuilder;
/// use this::storage::ScyllaLinkService;
///
/// let session = SessionBuilder::new().known_node("127.0.0.1:9042").build().await?;
/// let service = ScyllaLinkService::new(session, "my_keyspace");
/// ```
#[derive(Clone)]
pub struct ScyllaLinkService {
    session: Arc<Session>,
    keyspace: String,
}

impl ScyllaLinkService {
    pub fn new(session: Arc<Session>, keyspace: impl Into<String>) -> Self {
        Self {
            session,
            keyspace: keyspace.into(),
        }
    }

    /// Create with the default keyspace name.
    pub fn with_default_keyspace(session: Arc<Session>) -> Self {
        Self::new(session, DEFAULT_KEYSPACE)
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Parse a link from its entity_data JSON string.
    fn parse_link(data: &str) -> Result<LinkEntity> {
        serde_json::from_str(data).map_err(|e| anyhow!("Failed to deserialize link: {}", e))
    }

    /// Collect links from a query result.
    async fn collect_links(
        &self,
        cql: String,
        values: impl scylla::serialize::row::SerializeRow,
    ) -> Result<Vec<LinkEntity>> {
        let result = self
            .session
            .query_unpaged(cql, values)
            .await
            .map_err(|e| anyhow!("Failed to query links: {}", e))?;

        let rows_result = result
            .into_rows_result()
            .map_err(|e| anyhow!("Failed to parse result: {}", e))?;

        let rows: Vec<(String,)> = rows_result
            .rows()
            .map_err(|e| anyhow!("Failed to deserialize rows: {}", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to collect rows: {}", e))?;

        let mut links = Vec::new();
        for (data,) in &rows {
            links.push(Self::parse_link(data)?);
        }

        // Sort by created_at DESC
        links.sort_by_key(|b| std::cmp::Reverse(b.created_at));

        Ok(links)
    }
}

#[async_trait]
impl LinkService for ScyllaLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let json_str =
            serde_json::to_string(&link).map_err(|e| anyhow!("Failed to serialize link: {}", e))?;

        let cql = format!(
            "INSERT INTO {}.links (id, entity_type, source_id, target_id, link_type, \
             source_type, target_type, status, entity_data, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            self.keyspace
        );

        let source_type = link.entity_type.clone();
        let target_type = "".to_string(); // LinkEntity doesn't store source/target types separately

        self.session
            .query_unpaged(
                cql,
                (
                    link.id.to_string().as_str(),
                    &link.entity_type,
                    link.source_id.to_string().as_str(),
                    link.target_id.to_string().as_str(),
                    &link.link_type,
                    &source_type,
                    &target_type,
                    &link.status,
                    &json_str,
                    link.created_at.to_rfc3339().as_str(),
                    link.updated_at.to_rfc3339().as_str(),
                ),
            )
            .await
            .map_err(|e| anyhow!("Failed to create link: {}", e))?;

        Ok(link)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let cql = format!(
            "SELECT entity_data FROM {}.links WHERE id = ?",
            self.keyspace
        );

        let result = self
            .session
            .query_unpaged(cql, (id.to_string().as_str(),))
            .await
            .map_err(|e| anyhow!("Failed to get link: {}", e))?;

        let rows_result = result
            .into_rows_result()
            .map_err(|e| anyhow!("Failed to parse result: {}", e))?;

        let rows: Vec<(String,)> = rows_result
            .rows()
            .map_err(|e| anyhow!("Failed to deserialize rows: {}", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to collect rows: {}", e))?;

        match rows.first() {
            Some((data,)) => Ok(Some(Self::parse_link(data)?)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let cql = format!("SELECT entity_data FROM {}.links", self.keyspace);
        self.collect_links(cql, ()).await
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let cql = format!(
            "SELECT entity_data FROM {}.links WHERE source_id = ?",
            self.keyspace
        );

        let mut links = self
            .collect_links(cql, (source_id.to_string().as_str(),))
            .await?;

        // Apply optional filters client-side
        if let Some(lt) = link_type {
            links.retain(|l| l.link_type == lt);
        }

        Ok(links)
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let cql = format!(
            "SELECT entity_data FROM {}.links WHERE target_id = ?",
            self.keyspace
        );

        let mut links = self
            .collect_links(cql, (target_id.to_string().as_str(),))
            .await?;

        if let Some(lt) = link_type {
            links.retain(|l| l.link_type == lt);
        }

        Ok(links)
    }

    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        // Verify link exists
        let existing = self.get(id).await?;
        if existing.is_none() {
            return Err(anyhow!("Link not found: {}", id));
        }

        let json_str =
            serde_json::to_string(&link).map_err(|e| anyhow!("Failed to serialize link: {}", e))?;

        let cql = format!(
            "UPDATE {}.links SET entity_type = ?, source_id = ?, target_id = ?, \
             link_type = ?, status = ?, entity_data = ?, updated_at = ? WHERE id = ?",
            self.keyspace
        );

        self.session
            .query_unpaged(
                cql,
                (
                    &link.entity_type,
                    link.source_id.to_string().as_str(),
                    link.target_id.to_string().as_str(),
                    &link.link_type,
                    &link.status,
                    &json_str,
                    link.updated_at.to_rfc3339().as_str(),
                    id.to_string().as_str(),
                ),
            )
            .await
            .map_err(|e| anyhow!("Failed to update link: {}", e))?;

        Ok(link)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let cql = format!("DELETE FROM {}.links WHERE id = ?", self.keyspace);

        self.session
            .query_unpaged(cql, (id.to_string().as_str(),))
            .await
            .map_err(|e| anyhow!("Failed to delete link: {}", e))?;

        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let eid = entity_id.to_string();

        // Find all links where entity is source
        let source_cql = format!("SELECT id FROM {}.links WHERE source_id = ?", self.keyspace);
        let source_result = self
            .session
            .query_unpaged(source_cql, (eid.as_str(),))
            .await
            .map_err(|e| anyhow!("Failed to find source links: {}", e))?;

        // Find all links where entity is target
        let target_cql = format!("SELECT id FROM {}.links WHERE target_id = ?", self.keyspace);
        let target_result = self
            .session
            .query_unpaged(target_cql, (eid.as_str(),))
            .await
            .map_err(|e| anyhow!("Failed to find target links: {}", e))?;

        // Collect all link IDs to delete
        let mut ids_to_delete = Vec::new();

        let source_rows = source_result
            .into_rows_result()
            .map_err(|e| anyhow!("Failed to parse result: {}", e))?;
        let rows: Vec<(String,)> = source_rows
            .rows()
            .map_err(|e| anyhow!("Failed to deserialize: {}", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to collect: {}", e))?;
        for (id,) in &rows {
            ids_to_delete.push(id.clone());
        }

        let target_rows = target_result
            .into_rows_result()
            .map_err(|e| anyhow!("Failed to parse result: {}", e))?;
        let rows: Vec<(String,)> = target_rows
            .rows()
            .map_err(|e| anyhow!("Failed to deserialize: {}", e))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to collect: {}", e))?;
        for (id,) in &rows {
            if !ids_to_delete.contains(id) {
                ids_to_delete.push(id.clone());
            }
        }

        // Delete each link
        let delete_cql = format!("DELETE FROM {}.links WHERE id = ?", self.keyspace);
        for link_id in &ids_to_delete {
            self.session
                .query_unpaged(delete_cql.clone(), (link_id.as_str(),))
                .await
                .map_err(|e| anyhow!("Failed to delete link {}: {}", link_id, e))?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "scylladb")]
#[allow(dead_code)]
mod tests {
    use super::*;
    use crate::core::link::LinkEntity;
    use serde_json::json;
    use uuid::Uuid;

    // A lightweight entity for testing field_value / search-filter logic.
    crate::impl_data_entity!(TestWidget, "test_widget", ["name"], {
        weight: f64,
    });

    // -----------------------------------------------------------------------
    // parse_link helpers
    // -----------------------------------------------------------------------

    fn make_link(metadata: Option<serde_json::Value>) -> LinkEntity {
        LinkEntity::new("owns", Uuid::new_v4(), Uuid::new_v4(), metadata)
    }

    #[test]
    fn parse_link_valid_json() {
        let link = make_link(None);
        let json_str = serde_json::to_string(&link).expect("serialize");
        let parsed = ScyllaLinkService::parse_link(&json_str).expect("parse_link should succeed");

        assert_eq!(parsed.id, link.id);
        assert_eq!(parsed.link_type, "owns");
        assert_eq!(parsed.source_id, link.source_id);
        assert_eq!(parsed.target_id, link.target_id);
        assert_eq!(parsed.status, link.status);
    }

    #[test]
    fn parse_link_invalid_json() {
        let result = ScyllaLinkService::parse_link("not json");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Failed to deserialize link"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn parse_link_empty_object() {
        let result = ScyllaLinkService::parse_link("{}");
        assert!(
            result.is_err(),
            "empty JSON object should fail due to missing required fields"
        );
    }

    #[test]
    fn parse_link_with_metadata() {
        let meta = json!({"key": "val", "nested": {"a": 1}});
        let link = make_link(Some(meta.clone()));
        let json_str = serde_json::to_string(&link).expect("serialize");
        let parsed = ScyllaLinkService::parse_link(&json_str).expect("parse_link should succeed");

        assert_eq!(parsed.metadata, Some(meta));
    }

    #[test]
    fn parse_link_with_null_metadata() {
        let link = make_link(None);
        let json_str = serde_json::to_string(&link).expect("serialize");
        let parsed = ScyllaLinkService::parse_link(&json_str).expect("parse_link should succeed");

        assert_eq!(parsed.metadata, None);
        // Verify the rest of the entity survived the roundtrip.
        assert_eq!(parsed.id, link.id);
        assert_eq!(parsed.source_id, link.source_id);
        assert_eq!(parsed.target_id, link.target_id);
    }

    // -----------------------------------------------------------------------
    // Search field-value matching (mirrors the client-side filter in search())
    // -----------------------------------------------------------------------

    #[test]
    fn search_field_value_string_matching() {
        let widget = TestWidget::new("sprocket".into(), "active".into(), 3.5);

        let fv = widget.field_value("name").expect("name field should exist");
        assert_eq!(fv, FieldValue::String("sprocket".to_string()));

        // Simulate the filter predicate from ScyllaDataService::search
        let matches = match &fv {
            FieldValue::String(s) => s == "sprocket",
            _ => false,
        };
        assert!(matches, "FieldValue::String should match the search value");
    }

    #[test]
    fn search_field_value_integer_matching() {
        // FieldValue::Integer comparison uses to_string() in the search filter.
        let fv = FieldValue::Integer(42);
        let matches = match &fv {
            FieldValue::Integer(i) => i.to_string() == "42",
            _ => false,
        };
        assert!(
            matches,
            "FieldValue::Integer(42).to_string() should equal \"42\""
        );

        // Negative case
        let no_match = match &fv {
            FieldValue::Integer(i) => i.to_string() == "99",
            _ => false,
        };
        assert!(!no_match, "FieldValue::Integer(42) should not match \"99\"");
    }

    // -----------------------------------------------------------------------
    // Entity JSON serialization roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn entity_json_serialization_roundtrip() {
        let widget = TestWidget::new("gear".into(), "active".into(), 7.25);
        let json_str = serde_json::to_string(&widget).expect("serialize should succeed");
        let restored: TestWidget =
            serde_json::from_str(&json_str).expect("deserialize should succeed");

        assert_eq!(restored.id, widget.id);
        assert_eq!(restored.name, "gear");
        assert_eq!(restored.status, "active");
        assert_eq!(restored.entity_type, "test_widget");
        assert!((restored.weight - 7.25).abs() < f64::EPSILON);
    }
}
