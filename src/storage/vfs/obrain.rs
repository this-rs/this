//! `ObrainVfs`: VFS files as nodes of an `ObrainStore` (feature `obrain`).
//!
//! One node per file, label [`FILE_LABEL`], properties `_path`, `_content` (lower-case
//! hex), `_etag`, `_size`, `_modified` (RFC 3339), all strings: strings are the property
//! kind whose persistence across `ObrainStore::open` is already proven by the store's own
//! tests (`test_persistence_across_reopen_with_rebuilt_index`); `Value::Bytes` is not.
//!
//! No `path -> NodeId` index is kept in memory. `ObrainStore`'s `uuid -> NodeId` indexes
//! do NOT survive `open()` and must be rebuilt by hand (`rebuild_index_for_label`), and
//! they are cleared on every space switch (`use_graph`). A VFS cache would carry the
//! same trap; every operation therefore resolves the path by scanning the nodes of the
//! label. Module trees are small (a few files per version), the scan is the price of
//! being correct after a reopen without any rebuild step. Every mutation ends with
//! `commit_wal()` (durable once `Ok` is returned).
//!
//! Space: the files live in the ROOT graph of the database (`ObrainDB::graph_store_mut`),
//! whatever space the `ObrainStore` currently has active. `use_graph` is a store-wide
//! switch made by the entity layer (tenants, spaces); a VFS that followed it would see
//! its modules vanish, or another tenant's, on every switch.
//!
//! Creation is not atomic in obrain (node, then properties): `_path` is set LAST, so a
//! node interrupted half-way has no path and is ignored by `scan`.

use super::memory::{check_shape, list_flat};
use super::{
    Precondition, Vfs, VfsCaps, VfsEntry, VfsError, VfsMeta, VfsPath, check_precondition, etag_of,
};
use crate::storage::obrain::ObrainStore;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use obrain_common::types::{NodeId, Value as ObrainValue};
use obrain_core::graph::GraphStoreMut;
use std::sync::{Arc, Mutex};

/// Label of the VFS file nodes.
pub const FILE_LABEL: &str = "_vfs_file";

/// VFS over an `ObrainStore` (shares the store's active space).
pub struct ObrainVfs {
    store: Arc<ObrainStore>,
    /// Serializes check-then-write in this process.
    lock: Mutex<()>,
}

impl std::fmt::Debug for ObrainVfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObrainVfs").finish_non_exhaustive()
    }
}

fn backend(msg: impl std::fmt::Display) -> VfsError {
    VfsError::Backend(anyhow::anyhow!("obrain vfs: {msg}"))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Result<Vec<u8>, VfsError> {
    if !s.len().is_multiple_of(2) {
        return Err(backend("odd hex length"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(backend))
        .collect()
}

struct Row {
    id: NodeId,
    path: VfsPath,
    meta: VfsMeta,
}

impl ObrainVfs {
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self {
            store,
            lock: Mutex::new(()),
        }
    }

    /// Root graph of the database, independent of the store's active space.
    fn graph(&self) -> Arc<dyn GraphStoreMut> {
        self.store.db().graph_store_mut()
    }

    /// Every file node of the root graph. Nodes whose properties do not parse are
    /// skipped (never served), not fatal.
    fn scan(&self) -> Vec<Row> {
        let graph = self.graph();
        let mut out = Vec::new();
        for id in graph.nodes_by_label(FILE_LABEL) {
            let Some(node) = graph.get_node(id) else {
                continue;
            };
            let s = |k: &str| {
                node.get_property(k)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            let (Some(path), Some(etag), Some(size), Some(modified)) =
                (s("_path"), s("_etag"), s("_size"), s("_modified"))
            else {
                continue;
            };
            let (Ok(path), Ok(size), Ok(modified)) = (
                VfsPath::parse(&path),
                size.parse::<u64>(),
                DateTime::parse_from_rfc3339(&modified),
            ) else {
                continue;
            };
            out.push(Row {
                id,
                path,
                meta: VfsMeta {
                    size,
                    etag,
                    modified: modified.with_timezone(&Utc),
                },
            });
        }
        out
    }

    fn find(&self, path: &VfsPath) -> Option<Row> {
        self.scan().into_iter().find(|r| &r.path == path)
    }

    fn commit(&self) -> Result<(), VfsError> {
        self.store.commit_wal().map_err(VfsError::Backend)
    }

    fn guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, VfsError> {
        self.lock.lock().map_err(|_| backend("lock poisoned"))
    }
}

#[async_trait]
impl Vfs for ObrainVfs {
    async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        // Under the write lock: an update sets `_content` then `_etag`, a reader in
        // between would see a mismatch and report a spurious integrity error.
        let _g = self.guard()?;
        let row = self
            .find(path)
            .ok_or_else(|| VfsError::NotFound(path.clone()))?;
        let node = self
            .graph()
            .get_node(row.id)
            .ok_or_else(|| VfsError::NotFound(path.clone()))?;
        let hex = node
            .get_property("_content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| backend(format!("{path}: missing _content")))?;
        let bytes = from_hex(hex)?;
        // Integrity: the stored etag must match the stored content.
        if etag_of(&bytes) != row.meta.etag {
            return Err(backend(format!("{path}: content does not match its etag")));
        }
        Ok(bytes)
    }

    async fn write(
        &self,
        path: &VfsPath,
        bytes: &[u8],
        pre: Precondition,
    ) -> Result<VfsMeta, VfsError> {
        if self.store.is_read_only() {
            return Err(VfsError::ReadOnly);
        }
        self.store.check_write().map_err(VfsError::Backend)?;
        let _g = self.guard()?;
        let rows = self.scan();
        let current = rows.iter().find(|r| &r.path == path);
        check_precondition(path, current.map(|r| r.meta.etag.as_str()), &pre)?;
        if current.is_none() {
            check_shape(path, rows.iter().map(|r| &r.path))?;
        }
        let meta = VfsMeta {
            size: bytes.len() as u64,
            etag: etag_of(bytes),
            modified: Utc::now(),
        };
        let hex = to_hex(bytes);
        let size = meta.size.to_string();
        let modified = meta.modified.to_rfc3339();
        let props = |k: &'static str, v: &str| (k, ObrainValue::from(v));
        match current {
            Some(row) => {
                let graph = self.graph();
                for (k, v) in [
                    props("_content", &hex),
                    props("_etag", &meta.etag),
                    props("_size", &size),
                    props("_modified", &modified),
                ] {
                    graph.set_node_property(row.id, k, v);
                }
            }
            None => {
                let graph = self.graph();
                let id = graph.create_node(&[FILE_LABEL]);
                // `_path` last: an interrupted creation leaves a node `scan` ignores.
                for (k, v) in [
                    props("_content", &hex),
                    props("_etag", &meta.etag),
                    props("_size", &size),
                    props("_modified", &modified),
                    props("_path", path.as_str()),
                ] {
                    graph.set_node_property(id, k, v);
                }
            }
        }
        self.commit()?;
        Ok(meta)
    }

    async fn stat(&self, path: &VfsPath) -> Result<Option<VfsMeta>, VfsError> {
        Ok(self.find(path).map(|r| r.meta))
    }

    async fn list(&self, dir: &VfsPath) -> Result<Vec<VfsEntry>, VfsError> {
        let rows = self.scan();
        Ok(list_flat(dir, rows.iter().map(|r| (&r.path, &r.meta))))
    }

    async fn delete(&self, path: &VfsPath, pre: Precondition) -> Result<(), VfsError> {
        if self.store.is_read_only() {
            return Err(VfsError::ReadOnly);
        }
        self.store.check_write().map_err(VfsError::Backend)?;
        let _g = self.guard()?;
        let current = self.find(path);
        if current.is_none() && pre != Precondition::IfNoneMatch {
            return Err(VfsError::NotFound(path.clone()));
        }
        check_precondition(path, current.as_ref().map(|r| r.meta.etag.as_str()), &pre)?;
        if let Some(row) = current {
            self.graph().delete_node(row.id);
            self.commit()?;
        }
        Ok(())
    }

    fn caps(&self) -> VfsCaps {
        VfsCaps {
            read_only: self.store.is_read_only(),
            versioned: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::vfs::conformance;

    #[tokio::test]
    async fn obrain_vfs_conformance() {
        let v = ObrainVfs::new(ObrainStore::new_in_memory());
        conformance::run_all(&v).await;
    }

    #[tokio::test]
    async fn obrain_vfs_survives_reopen_without_rebuild() {
        let dir = std::env::temp_dir().join(format!("this_obrain_vfs_{}", uuid::Uuid::new_v4()));
        let p = VfsPath::parse("/modules/billing/1.0.0/descriptor.json").unwrap();
        let q = VfsPath::parse("/modules/billing/1.0.0/i18n/fr.json").unwrap();
        let etag = {
            let v = ObrainVfs::new(ObrainStore::open(&dir).unwrap());
            v.write(&q, b"{\"fr\":1}", Precondition::None)
                .await
                .unwrap();
            let m = v
                .write(&p, b"{\"a\":1}", Precondition::IfNoneMatch)
                .await
                .unwrap();
            // Update and delete must be durable too.
            v.write(&q, b"{\"fr\":2}", Precondition::None)
                .await
                .unwrap();
            m.etag
        };
        // Reopen: NO rebuild call, the VFS keeps no in-memory index.
        let v = ObrainVfs::new(ObrainStore::open(&dir).unwrap());
        assert_eq!(v.stat(&p).await.unwrap().unwrap().etag, etag);
        assert_eq!(v.read(&p).await.unwrap(), b"{\"a\":1}");
        assert_eq!(v.read(&q).await.unwrap(), b"{\"fr\":2}");
        assert!(matches!(
            v.write(&p, b"x", Precondition::IfNoneMatch).await,
            Err(VfsError::PreconditionFailed { .. })
        ));
        v.delete(&q, Precondition::None).await.unwrap();
        drop(v);
        let v = ObrainVfs::new(ObrainStore::open(&dir).unwrap());
        assert!(v.stat(&q).await.unwrap().is_none(), "delete durable");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn obrain_vfs_ignores_the_active_space() {
        // The entity layer switches spaces (`use_graph`); the module files must not
        // follow, neither for reads nor for writes.
        let store = ObrainStore::new_in_memory();
        let v = ObrainVfs::new(store.clone());
        let a = VfsPath::parse("/modules/billing/1.0.0/descriptor.json").unwrap();
        let b = VfsPath::parse("/modules/billing/1.0.0/i18n/fr.json").unwrap();
        v.write(&a, b"root", Precondition::None).await.unwrap();
        store.create_space("tenant_a").unwrap();
        store.use_graph("tenant_a");
        assert_eq!(store.space().as_deref(), Some("tenant_a"));
        assert_eq!(
            v.read(&a).await.unwrap(),
            b"root",
            "read while a space is active"
        );
        v.write(&b, b"fr", Precondition::IfNoneMatch).await.unwrap();
        // The tenant space holds no VFS node.
        assert!(
            store.active_store().nodes_by_label(FILE_LABEL).is_empty(),
            "VFS nodes leaked into the tenant space"
        );
        store.reset_graph();
        assert_eq!(v.read(&b).await.unwrap(), b"fr");
        assert_eq!(
            v.list(&VfsPath::parse("/modules/billing/1.0.0").unwrap())
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn obrain_module_store_survives_reopen() {
        use crate::descriptor::ModuleDescriptor;
        use crate::storage::vfs::{ModuleStore as _, VfsModuleStore};
        let dir = std::env::temp_dir().join(format!("this_obrain_ms_{}", uuid::Uuid::new_v4()));
        {
            let vfs = Arc::new(ObrainVfs::new(ObrainStore::open(&dir).unwrap()));
            let store = VfsModuleStore::new(vfs);
            store
                .put(&ModuleDescriptor::new("billing", "1.0.0"), &[])
                .await
                .unwrap();
            store
                .put(&ModuleDescriptor::new("billing", "1.1.0"), &[])
                .await
                .unwrap();
            store.yank("billing", "1.1.0").await.unwrap();
        }
        let vfs = Arc::new(ObrainVfs::new(ObrainStore::open(&dir).unwrap()));
        let store = VfsModuleStore::new(vfs);
        assert_eq!(store.get("billing", None).await.unwrap().version, "1.0.0");
        assert_eq!(store.list().await.unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
