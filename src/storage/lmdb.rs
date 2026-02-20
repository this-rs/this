//! LMDB storage backend using heed (memory-mapped B-tree).
//!
//! LMDB is an embedded key-value store — no external server required.
//! All operations are synchronous (memory-mapped I/O) and are wrapped in
//! `tokio::task::spawn_blocking` for async compatibility.
//!
//! # Architecture
//!
//! - `LmdbDataService<T>` — stores entities keyed by UUID string
//! - `LmdbLinkService` — stores links keyed by UUID string, with secondary
//!   index databases for `find_by_source` and `find_by_target`
//!
//! # Databases (named LMDB sub-databases)
//!
//! - `entities` — primary entity store (JSON-encoded values)
//! - `links` — primary link store (JSON-encoded values)
//! - `links_by_source` — composite key `{source_uuid}:{link_uuid}` → empty
//! - `links_by_target` — composite key `{target_uuid}:{link_uuid}` → empty
//!
//! # Serialization
//!
//! Values are stored as JSON bytes via `serde_json`. This is necessary because
//! `LinkEntity` contains `serde_json::Value` (the `metadata` field) which
//! cannot be round-tripped through bincode's binary format. JSON is universally
//! compatible with all serde types and the overhead is negligible with LMDB's
//! memory-mapped I/O.
//!
//! # Feature flag
//!
//! Enable with `--features lmdb`. Requires the `heed` crate.

use crate::core::field::FieldValue;
use crate::core::link::LinkEntity;
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvOpenOptions};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

/// Encode a value as JSON bytes for LMDB storage.
fn lmdb_encode<T: serde::Serialize>(item: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(item).map_err(|e| anyhow!("lmdb encode: {}", e))
}

/// Decode a value from JSON bytes.
fn lmdb_decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| anyhow!("lmdb decode: {}", e))
}

// ---------------------------------------------------------------------------
// LmdbDataService
// ---------------------------------------------------------------------------

/// LMDB-backed implementation of `DataService<T>`.
///
/// Stores entities as JSON blobs keyed by their UUID string.
/// The `Env` is wrapped in an `Arc` for cheap cloning across async tasks.
///
/// # Example
///
/// ```rust,ignore
/// use this::storage::LmdbDataService;
///
/// let service = LmdbDataService::<MyEntity>::open("/tmp/my-lmdb")?;
/// let entity = service.create(my_entity).await?;
/// ```
pub struct LmdbDataService<T: Data> {
    env: Arc<Env>,
    db: Database<Str, Bytes>,
    _marker: PhantomData<T>,
}

impl<T: Data> LmdbDataService<T> {
    /// Open (or create) an LMDB environment at `path` and initialise the
    /// `entities` named database.
    ///
    /// The map size defaults to 256 MB which is plenty for typical use-cases.
    /// LMDB will not actually allocate that much — it is a virtual address
    /// space reservation.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(path.as_ref())?;

        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(256 * 1024 * 1024)
                .max_dbs(10)
                .max_readers(126)
                .open(path.as_ref())?
        };

        let mut wtxn = env.write_txn()?;
        let db: Database<Str, Bytes> = env.create_database(&mut wtxn, Some("entities"))?;
        wtxn.commit()?;

        Ok(Self {
            env: Arc::new(env),
            db,
            _marker: PhantomData,
        })
    }
}

impl<T: Data> Clone for LmdbDataService<T> {
    fn clone(&self) -> Self {
        Self {
            env: Arc::clone(&self.env),
            db: self.db,
            _marker: PhantomData,
        }
    }
}

#[async_trait]
impl<T: Data + serde::Serialize + serde::de::DeserializeOwned> DataService<T>
    for LmdbDataService<T>
{
    async fn create(&self, entity: T) -> Result<T> {
        let env = self.env.clone();
        let db = self.db;
        let key = entity.id().to_string();
        let bytes = lmdb_encode(&entity)?;

        tokio::task::spawn_blocking(move || {
            let mut wtxn = env.write_txn()?;
            db.put(&mut wtxn, &key, &bytes)?;
            wtxn.commit()?;
            Ok(entity)
        })
        .await?
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let env = self.env.clone();
        let db = self.db;
        let key = id.to_string();

        tokio::task::spawn_blocking(move || {
            let rtxn = env.read_txn()?;
            match db.get(&rtxn, &key)? {
                Some(bytes) => Ok(Some(lmdb_decode(bytes)?)),
                None => Ok(None),
            }
        })
        .await?
    }

    async fn list(&self) -> Result<Vec<T>> {
        let env = self.env.clone();
        let db = self.db;

        tokio::task::spawn_blocking(move || {
            let rtxn = env.read_txn()?;
            let mut results = Vec::new();
            for item in db.iter(&rtxn)? {
                let (_key, bytes) = item?;
                results.push(lmdb_decode(bytes)?);
            }
            Ok(results)
        })
        .await?
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        let env = self.env.clone();
        let db = self.db;
        let key = id.to_string();
        let bytes = lmdb_encode(&entity)?;

        tokio::task::spawn_blocking(move || {
            let mut wtxn = env.write_txn()?;
            // Verify entity exists
            if db.get(&wtxn, &key)?.is_none() {
                return Err(anyhow!("Entity not found: {}", key));
            }
            db.put(&mut wtxn, &key, &bytes)?;
            wtxn.commit()?;
            Ok(entity)
        })
        .await?
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let env = self.env.clone();
        let db = self.db;
        let key = id.to_string();

        tokio::task::spawn_blocking(move || {
            let mut wtxn = env.write_txn()?;
            db.delete(&mut wtxn, &key)?;
            wtxn.commit()?;
            Ok(())
        })
        .await?
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        let env = self.env.clone();
        let db = self.db;
        let field = field.to_owned();
        let value = value.to_owned();

        tokio::task::spawn_blocking(move || {
            let rtxn = env.read_txn()?;
            let mut results = Vec::new();
            for item in db.iter(&rtxn)? {
                let (_key, bytes) = item?;
                let entity: T = lmdb_decode(bytes)?;
                if entity.field_value(&field).is_some_and(|fv| match &fv {
                    FieldValue::String(s) => s == &value,
                    FieldValue::Integer(i) => i.to_string() == value,
                    FieldValue::Float(f) => f.to_string() == value,
                    FieldValue::Boolean(b) => b.to_string() == value,
                    FieldValue::Uuid(u) => u.to_string() == value,
                    FieldValue::DateTime(dt) => dt.to_rfc3339() == value,
                    FieldValue::Null => false,
                }) {
                    results.push(entity);
                }
            }
            Ok(results)
        })
        .await?
    }
}

// ---------------------------------------------------------------------------
// LmdbLinkService
// ---------------------------------------------------------------------------

/// LMDB-backed implementation of `LinkService`.
///
/// Uses three named databases:
/// - `links` — primary store, keyed by link UUID string
/// - `links_by_source` — secondary index: `{source_uuid}:{link_uuid}` → empty
/// - `links_by_target` — secondary index: `{target_uuid}:{link_uuid}` → empty
///
/// The secondary indexes enable efficient `find_by_source` and `find_by_target`
/// via LMDB prefix iteration.
#[derive(Clone)]
pub struct LmdbLinkService {
    env: Arc<Env>,
    links_db: Database<Str, Bytes>,
    by_source_db: Database<Str, Bytes>,
    by_target_db: Database<Str, Bytes>,
}

impl LmdbLinkService {
    /// Open (or create) an LMDB environment at `path` and initialise the
    /// link databases (primary + secondary indexes).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(path.as_ref())?;

        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(256 * 1024 * 1024)
                .max_dbs(10)
                .max_readers(126)
                .open(path.as_ref())?
        };

        let mut wtxn = env.write_txn()?;
        let links_db: Database<Str, Bytes> =
            env.create_database(&mut wtxn, Some("links"))?;
        let by_source_db: Database<Str, Bytes> =
            env.create_database(&mut wtxn, Some("links_by_source"))?;
        let by_target_db: Database<Str, Bytes> =
            env.create_database(&mut wtxn, Some("links_by_target"))?;
        wtxn.commit()?;

        Ok(Self {
            env: Arc::new(env),
            links_db,
            by_source_db,
            by_target_db,
        })
    }
}

/// Composite key for secondary indexes: `{prefix_uuid}:{link_uuid}`.
fn composite_key(prefix: &Uuid, link_id: &Uuid) -> String {
    format!("{}:{}", prefix, link_id)
}

#[async_trait]
impl LinkService for LmdbLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let env = self.env.clone();
        let links_db = self.links_db;
        let by_source_db = self.by_source_db;
        let by_target_db = self.by_target_db;
        let bytes = lmdb_encode(&link)?;
        let key = link.id.to_string();
        let source_key = composite_key(&link.source_id, &link.id);
        let target_key = composite_key(&link.target_id, &link.id);

        tokio::task::spawn_blocking(move || {
            let mut wtxn = env.write_txn()?;
            links_db.put(&mut wtxn, &key, &bytes)?;
            by_source_db.put(&mut wtxn, &source_key, &[])?;
            by_target_db.put(&mut wtxn, &target_key, &[])?;
            wtxn.commit()?;
            Ok(link)
        })
        .await?
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let env = self.env.clone();
        let links_db = self.links_db;
        let key = id.to_string();

        tokio::task::spawn_blocking(move || {
            let rtxn = env.read_txn()?;
            match links_db.get(&rtxn, &key)? {
                Some(bytes) => Ok(Some(lmdb_decode(bytes)?)),
                None => Ok(None),
            }
        })
        .await?
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let env = self.env.clone();
        let links_db = self.links_db;

        tokio::task::spawn_blocking(move || {
            let rtxn = env.read_txn()?;
            let mut results = Vec::new();
            for item in links_db.iter(&rtxn)? {
                let (_key, bytes) = item?;
                results.push(lmdb_decode(bytes)?);
            }
            Ok(results)
        })
        .await?
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let env = self.env.clone();
        let links_db = self.links_db;
        let by_source_db = self.by_source_db;
        let prefix = format!("{}:", source_id);
        let link_type = link_type.map(|s| s.to_owned());

        tokio::task::spawn_blocking(move || {
            let rtxn = env.read_txn()?;
            let mut results = Vec::new();
            for item in by_source_db.prefix_iter(&rtxn, &prefix)? {
                let (composite, _) = item?;
                // Extract link_id from "source_uuid:link_uuid"
                let link_id = &composite[prefix.len()..];
                if let Some(bytes) = links_db.get(&rtxn, link_id)? {
                    let link: LinkEntity = lmdb_decode(bytes)?;
                    if link_type.as_deref().is_none_or(|lt| link.link_type == lt) {
                        results.push(link);
                    }
                }
            }
            Ok(results)
        })
        .await?
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let env = self.env.clone();
        let links_db = self.links_db;
        let by_target_db = self.by_target_db;
        let prefix = format!("{}:", target_id);
        let link_type = link_type.map(|s| s.to_owned());

        tokio::task::spawn_blocking(move || {
            let rtxn = env.read_txn()?;
            let mut results = Vec::new();
            for item in by_target_db.prefix_iter(&rtxn, &prefix)? {
                let (composite, _) = item?;
                let link_id = &composite[prefix.len()..];
                if let Some(bytes) = links_db.get(&rtxn, link_id)? {
                    let link: LinkEntity = lmdb_decode(bytes)?;
                    if link_type.as_deref().is_none_or(|lt| link.link_type == lt) {
                        results.push(link);
                    }
                }
            }
            Ok(results)
        })
        .await?
    }

    async fn update(&self, id: &Uuid, updated_link: LinkEntity) -> Result<LinkEntity> {
        let env = self.env.clone();
        let links_db = self.links_db;
        let by_source_db = self.by_source_db;
        let by_target_db = self.by_target_db;
        let key = id.to_string();
        let new_bytes = lmdb_encode(&updated_link)?;

        tokio::task::spawn_blocking(move || {
            let mut wtxn = env.write_txn()?;

            // Get old link to clean up old secondary index entries
            let old_bytes = links_db
                .get(&wtxn, &key)?
                .ok_or_else(|| anyhow!("Link not found: {}", key))?;
            let old_link: LinkEntity = lmdb_decode(old_bytes)?;

            // Remove old secondary indexes
            let old_source_key = composite_key(&old_link.source_id, &old_link.id);
            let old_target_key = composite_key(&old_link.target_id, &old_link.id);
            by_source_db.delete(&mut wtxn, &old_source_key)?;
            by_target_db.delete(&mut wtxn, &old_target_key)?;

            // Write updated link + new secondary indexes
            links_db.put(&mut wtxn, &key, &new_bytes)?;
            let new_source_key = composite_key(&updated_link.source_id, &updated_link.id);
            let new_target_key = composite_key(&updated_link.target_id, &updated_link.id);
            by_source_db.put(&mut wtxn, &new_source_key, &[])?;
            by_target_db.put(&mut wtxn, &new_target_key, &[])?;

            wtxn.commit()?;
            Ok(updated_link)
        })
        .await?
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let env = self.env.clone();
        let links_db = self.links_db;
        let by_source_db = self.by_source_db;
        let by_target_db = self.by_target_db;
        let key = id.to_string();

        tokio::task::spawn_blocking(move || {
            let mut wtxn = env.write_txn()?;

            // Get link to clean up secondary indexes
            if let Some(bytes) = links_db.get(&wtxn, &key)? {
                let link: LinkEntity = lmdb_decode(bytes)?;
                let source_key = composite_key(&link.source_id, &link.id);
                let target_key = composite_key(&link.target_id, &link.id);
                by_source_db.delete(&mut wtxn, &source_key)?;
                by_target_db.delete(&mut wtxn, &target_key)?;
            }

            links_db.delete(&mut wtxn, &key)?;
            wtxn.commit()?;
            Ok(())
        })
        .await?
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let env = self.env.clone();
        let links_db = self.links_db;
        let by_source_db = self.by_source_db;
        let by_target_db = self.by_target_db;
        let entity_id = *entity_id;

        tokio::task::spawn_blocking(move || {
            let mut wtxn = env.write_txn()?;

            // Collect all link IDs where entity is source or target
            let mut to_delete = Vec::new();

            // Find via source index
            let source_prefix = format!("{}:", entity_id);
            for item in by_source_db.prefix_iter(&wtxn, &source_prefix)? {
                let (composite, _) = item?;
                let link_id = composite[source_prefix.len()..].to_string();
                to_delete.push(link_id);
            }

            // Find via target index
            let target_prefix = format!("{}:", entity_id);
            for item in by_target_db.prefix_iter(&wtxn, &target_prefix)? {
                let (composite, _) = item?;
                let link_id = composite[target_prefix.len()..].to_string();
                if !to_delete.contains(&link_id) {
                    to_delete.push(link_id);
                }
            }

            // Delete each link + its index entries
            for link_id_str in &to_delete {
                if let Some(bytes) = links_db.get(&wtxn, link_id_str.as_str())? {
                    let link: LinkEntity = lmdb_decode(bytes)?;
                    let source_key = composite_key(&link.source_id, &link.id);
                    let target_key = composite_key(&link.target_id, &link.id);
                    by_source_db.delete(&mut wtxn, &source_key)?;
                    by_target_db.delete(&mut wtxn, &target_key)?;
                }
                links_db.delete(&mut wtxn, link_id_str.as_str())?;
            }

            wtxn.commit()?;
            Ok(())
        })
        .await?
    }
}
