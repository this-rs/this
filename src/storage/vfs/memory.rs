//! `InMemoryVfs`: a `BTreeMap<VfsPath, file>` behind one lock. Reference implementation
//! of the `Vfs` semantics (tests, ephemeral hosts, caches in front of a slower backend).

use super::{
    Precondition, Vfs, VfsCaps, VfsEntry, VfsError, VfsMeta, VfsPath, check_precondition, etag_of,
};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
struct File {
    bytes: Arc<[u8]>,
    meta: VfsMeta,
}

/// In-memory VFS. Cheap to clone? No: share it with `Arc<InMemoryVfs>`.
#[derive(Default)]
pub struct InMemoryVfs {
    files: RwLock<BTreeMap<VfsPath, File>>,
    read_only: bool,
}

impl std::fmt::Debug for InMemoryVfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.files.read().map(|m| m.len()).unwrap_or(0);
        f.debug_struct("InMemoryVfs")
            .field("files", &n)
            .field("read_only", &self.read_only)
            .finish()
    }
}

impl InMemoryVfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// A read-only copy of the current content (snapshot handed to a sandbox, a preview…).
    pub fn snapshot_read_only(&self) -> Self {
        let files = self.files.read().map(|m| m.clone()).unwrap_or_default();
        Self {
            files: RwLock::new(files),
            read_only: true,
        }
    }
}

fn poisoned() -> VfsError {
    VfsError::Backend(anyhow::anyhow!("in-memory vfs lock poisoned"))
}

/// Shape rule shared by the backends that store a flat path -> file map: a file cannot
/// replace a directory (some file below it) nor live below a file.
pub(crate) fn check_shape<'a>(
    path: &VfsPath,
    mut existing: impl Iterator<Item = &'a VfsPath>,
) -> Result<(), VfsError> {
    if path.is_root() {
        return Err(VfsError::InvalidPath(path.to_string()));
    }
    if existing.any(|p| p.is_below(path) || path.is_below(p)) {
        return Err(VfsError::InvalidPath(path.to_string()));
    }
    Ok(())
}

/// Immediate children of `dir` from a flat file map (directories are implicit).
pub(crate) fn list_flat<'a>(
    dir: &VfsPath,
    files: impl Iterator<Item = (&'a VfsPath, &'a VfsMeta)>,
) -> Vec<VfsEntry> {
    let mut out: BTreeMap<VfsPath, VfsEntry> = BTreeMap::new();
    for (p, meta) in files {
        let Some(child) = p.child_of(dir) else {
            continue;
        };
        if &child == p {
            out.insert(
                child.clone(),
                VfsEntry::File {
                    path: child,
                    meta: meta.clone(),
                },
            );
        } else {
            out.entry(child.clone())
                .or_insert(VfsEntry::Dir { path: child });
        }
    }
    out.into_values().collect()
}

#[async_trait]
impl Vfs for InMemoryVfs {
    async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let files = self.files.read().map_err(|_| poisoned())?;
        files
            .get(path)
            .map(|f| f.bytes.to_vec())
            .ok_or_else(|| VfsError::NotFound(path.clone()))
    }

    async fn write(
        &self,
        path: &VfsPath,
        bytes: &[u8],
        pre: Precondition,
    ) -> Result<VfsMeta, VfsError> {
        if self.read_only {
            return Err(VfsError::ReadOnly);
        }
        let mut files = self.files.write().map_err(|_| poisoned())?;
        let current = files.get(path).map(|f| f.meta.etag.clone());
        check_precondition(path, current.as_deref(), &pre)?;
        if current.is_none() {
            check_shape(path, files.keys())?;
        }
        let meta = VfsMeta {
            size: bytes.len() as u64,
            etag: etag_of(bytes),
            modified: Utc::now(),
        };
        files.insert(
            path.clone(),
            File {
                bytes: Arc::from(bytes),
                meta: meta.clone(),
            },
        );
        Ok(meta)
    }

    async fn stat(&self, path: &VfsPath) -> Result<Option<VfsMeta>, VfsError> {
        let files = self.files.read().map_err(|_| poisoned())?;
        Ok(files.get(path).map(|f| f.meta.clone()))
    }

    async fn list(&self, dir: &VfsPath) -> Result<Vec<VfsEntry>, VfsError> {
        let files = self.files.read().map_err(|_| poisoned())?;
        Ok(list_flat(dir, files.iter().map(|(p, f)| (p, &f.meta))))
    }

    async fn delete(&self, path: &VfsPath, pre: Precondition) -> Result<(), VfsError> {
        if self.read_only {
            return Err(VfsError::ReadOnly);
        }
        let mut files = self.files.write().map_err(|_| poisoned())?;
        let current = files.get(path).map(|f| f.meta.etag.clone());
        if current.is_none() && pre != Precondition::IfNoneMatch {
            return Err(VfsError::NotFound(path.clone()));
        }
        check_precondition(path, current.as_deref(), &pre)?;
        files.remove(path);
        Ok(())
    }

    fn caps(&self) -> VfsCaps {
        VfsCaps {
            read_only: self.read_only,
            versioned: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::vfs::conformance;

    #[tokio::test]
    async fn vfs_conformance_read_write_roundtrip_memory() {
        conformance::read_write_roundtrip(&InMemoryVfs::new()).await;
    }
    #[tokio::test]
    async fn vfs_conformance_if_match_conflict_memory() {
        conformance::if_match_conflict(&InMemoryVfs::new()).await;
    }
    #[tokio::test]
    async fn vfs_conformance_if_none_match_create_only_memory() {
        conformance::if_none_match_create_only(&InMemoryVfs::new()).await;
    }
    #[tokio::test]
    async fn vfs_conformance_list_dir_memory() {
        conformance::list_dir(&InMemoryVfs::new()).await;
    }
    #[tokio::test]
    async fn vfs_conformance_delete_missing_not_found_memory() {
        conformance::delete_missing_not_found(&InMemoryVfs::new()).await;
    }
    #[tokio::test]
    async fn vfs_conformance_shape_conflicts_memory() {
        conformance::shape_conflicts(&InMemoryVfs::new()).await;
    }

    #[tokio::test]
    async fn memory_snapshot_is_read_only() {
        let v = InMemoryVfs::new();
        let p = VfsPath::parse("/a").unwrap();
        v.write(&p, b"x", Precondition::None).await.unwrap();
        let s = v.snapshot_read_only();
        assert!(s.caps().read_only);
        assert_eq!(s.read(&p).await.unwrap(), b"x");
        assert!(matches!(
            s.write(&p, b"y", Precondition::None).await,
            Err(VfsError::ReadOnly)
        ));
    }
}
