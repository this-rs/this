//! `LocalFsVfs`: a sandboxed directory of the local file system.
//!
//! - Sandbox: `VfsPath` already refuses `..`, `.`, `\` and NUL; on top of that every
//!   component under the root is checked with `symlink_metadata` and a symlink is refused
//!   (`InvalidPath`), so a link planted in the tree cannot lead outside the root.
//! - Atomic writes: temp file in the same directory (`.vfs-tmp-<uuid>`), fsync, rename.
//!   Temp names are hidden from `list` and refused as user paths.
//! - Implicit directories: empty parents are pruned after a delete, so `list` behaves like
//!   the flat backends (a directory exists while a file exists below it).
//! - Preconditions are checked under an in-process lock. Two PROCESSES writing the same
//!   tree are not serialized (no advisory lock): one writer process per root.

use super::{
    Precondition, Vfs, VfsCaps, VfsEntry, VfsError, VfsMeta, VfsPath, check_precondition, etag_of,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const TMP_PREFIX: &str = ".vfs-tmp-";

#[derive(Debug)]
struct Inner {
    root: PathBuf,
    lock: Mutex<()>,
    read_only: bool,
}

/// VFS rooted at a local directory.
#[derive(Debug, Clone)]
pub struct LocalFsVfs {
    inner: Arc<Inner>,
}

fn backend(e: impl Into<anyhow::Error>) -> VfsError {
    VfsError::Backend(e.into())
}

impl LocalFsVfs {
    /// Open (and create) `root`. The root is canonicalized once: later checks compare
    /// against the real directory, not a path that could be swapped.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VfsError> {
        fs::create_dir_all(root.as_ref()).map_err(backend)?;
        let root = fs::canonicalize(root.as_ref()).map_err(backend)?;
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                lock: Mutex::new(()),
                read_only: false,
            }),
        })
    }

    /// Same tree, refusing writes.
    pub fn open_read_only(root: impl AsRef<Path>) -> Result<Self, VfsError> {
        let root = fs::canonicalize(root.as_ref()).map_err(backend)?;
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                lock: Mutex::new(()),
                read_only: true,
            }),
        })
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Inner) -> Result<T, VfsError> + Send + 'static,
    ) -> Result<T, VfsError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || f(&inner))
            .await
            .map_err(backend)?
    }
}

/// Where `path` lives on disk, refusing symlinks and temp names on the way.
/// `Ok((local, ancestor_is_file))`.
fn resolve(root: &Path, path: &VfsPath) -> Result<(PathBuf, bool), VfsError> {
    let mut local = root.to_path_buf();
    let mut ancestor_is_file = false;
    let segs: Vec<&str> = path.segments().collect();
    for (i, seg) in segs.iter().enumerate() {
        if seg.starts_with(TMP_PREFIX) {
            return Err(VfsError::InvalidPath(path.to_string()));
        }
        local.push(seg);
        match fs::symlink_metadata(&local) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(VfsError::InvalidPath(path.to_string()));
            }
            Ok(m) if m.is_file() && i + 1 < segs.len() => ancestor_is_file = true,
            _ => {}
        }
    }
    Ok((local, ancestor_is_file))
}

fn modified_of(m: &fs::Metadata) -> DateTime<Utc> {
    m.modified()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now())
}

fn meta_of(local: &Path) -> Result<Option<VfsMeta>, VfsError> {
    match fs::symlink_metadata(local) {
        Ok(m) if m.is_file() => {
            let bytes = fs::read(local).map_err(backend)?;
            Ok(Some(VfsMeta {
                size: bytes.len() as u64,
                etag: etag_of(&bytes),
                modified: modified_of(&m),
            }))
        }
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(backend(e)),
    }
}

fn prune_empty_parents(root: &Path, mut dir: PathBuf) {
    while dir != root && dir.starts_with(root) {
        // remove_dir only succeeds on an empty directory.
        if fs::remove_dir(&dir).is_err() {
            break;
        }
        if !dir.pop() {
            break;
        }
    }
}

#[async_trait]
impl Vfs for LocalFsVfs {
    async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let path = path.clone();
        self.blocking(move |inner| {
            let (local, _) = resolve(&inner.root, &path)?;
            match fs::symlink_metadata(&local) {
                Ok(m) if m.is_file() => fs::read(&local).map_err(backend),
                Ok(_) => Err(VfsError::NotFound(path)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(VfsError::NotFound(path)),
                Err(e) => Err(backend(e)),
            }
        })
        .await
    }

    async fn write(
        &self,
        path: &VfsPath,
        bytes: &[u8],
        pre: Precondition,
    ) -> Result<VfsMeta, VfsError> {
        if self.inner.read_only {
            return Err(VfsError::ReadOnly);
        }
        let path = path.clone();
        let bytes = bytes.to_vec();
        self.blocking(move |inner| {
            let _g = inner
                .lock
                .lock()
                .map_err(|_| backend(anyhow::anyhow!("fs vfs lock poisoned")))?;
            if path.is_root() {
                return Err(VfsError::InvalidPath(path.to_string()));
            }
            let (local, ancestor_is_file) = resolve(&inner.root, &path)?;
            if ancestor_is_file || local.is_dir() {
                return Err(VfsError::InvalidPath(path.to_string()));
            }
            let current = meta_of(&local)?.map(|m| m.etag);
            check_precondition(&path, current.as_deref(), &pre)?;
            let dir = local
                .parent()
                .ok_or_else(|| VfsError::InvalidPath(path.to_string()))?;
            fs::create_dir_all(dir).map_err(backend)?;
            let tmp = dir.join(format!("{TMP_PREFIX}{}", uuid::Uuid::new_v4()));
            let res = (|| -> std::io::Result<()> {
                let mut f = fs::File::create(&tmp)?;
                f.write_all(&bytes)?;
                f.sync_all()?;
                fs::rename(&tmp, &local)
            })();
            if let Err(e) = res {
                let _ = fs::remove_file(&tmp);
                return Err(backend(e));
            }
            let m = fs::symlink_metadata(&local).map_err(backend)?;
            Ok(VfsMeta {
                size: bytes.len() as u64,
                etag: etag_of(&bytes),
                modified: modified_of(&m),
            })
        })
        .await
    }

    async fn stat(&self, path: &VfsPath) -> Result<Option<VfsMeta>, VfsError> {
        let path = path.clone();
        self.blocking(move |inner| {
            let (local, _) = resolve(&inner.root, &path)?;
            meta_of(&local)
        })
        .await
    }

    async fn list(&self, dir: &VfsPath) -> Result<Vec<VfsEntry>, VfsError> {
        let dir = dir.clone();
        self.blocking(move |inner| {
            let (local, _) = resolve(&inner.root, &dir)?;
            let rd = match fs::read_dir(&local) {
                Ok(rd) => rd,
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        || e.kind() == std::io::ErrorKind::NotADirectory =>
                {
                    return Ok(vec![]);
                }
                Err(e) => return Err(backend(e)),
            };
            let mut out = Vec::new();
            for entry in rd {
                let entry = entry.map_err(backend)?;
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue; // not representable as a VfsPath
                };
                if name.starts_with(TMP_PREFIX) {
                    continue;
                }
                let Ok(child) = dir.join(&name) else {
                    continue;
                };
                let ft = entry.file_type().map_err(backend)?;
                if ft.is_symlink() {
                    continue; // never exposed, never followed
                } else if ft.is_dir() {
                    out.push(VfsEntry::Dir { path: child });
                } else if let Some(meta) = meta_of(&entry.path())? {
                    out.push(VfsEntry::File { path: child, meta });
                }
            }
            out.sort_by(|a, b| entry_path(a).cmp(entry_path(b)));
            Ok(out)
        })
        .await
    }

    async fn delete(&self, path: &VfsPath, pre: Precondition) -> Result<(), VfsError> {
        if self.inner.read_only {
            return Err(VfsError::ReadOnly);
        }
        let path = path.clone();
        self.blocking(move |inner| {
            let _g = inner
                .lock
                .lock()
                .map_err(|_| backend(anyhow::anyhow!("fs vfs lock poisoned")))?;
            let (local, _) = resolve(&inner.root, &path)?;
            let current = meta_of(&local)?.map(|m| m.etag);
            if current.is_none() && pre != Precondition::IfNoneMatch {
                return Err(VfsError::NotFound(path));
            }
            check_precondition(&path, current.as_deref(), &pre)?;
            if current.is_some() {
                fs::remove_file(&local).map_err(backend)?;
                if let Some(parent) = local.parent() {
                    prune_empty_parents(&inner.root, parent.to_path_buf());
                }
            }
            Ok(())
        })
        .await
    }

    fn caps(&self) -> VfsCaps {
        VfsCaps {
            read_only: self.inner.read_only,
            versioned: false,
        }
    }
}

fn entry_path(e: &VfsEntry) -> &VfsPath {
    match e {
        VfsEntry::File { path, .. } | VfsEntry::Dir { path } => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::vfs::conformance;

    fn fresh() -> (tempfile::TempDir, LocalFsVfs) {
        let dir = tempfile::tempdir().unwrap();
        let v = LocalFsVfs::open(dir.path().join("root")).unwrap();
        (dir, v)
    }

    #[tokio::test]
    async fn vfs_conformance_read_write_roundtrip_fs() {
        let (_d, v) = fresh();
        conformance::read_write_roundtrip(&v).await;
    }
    #[tokio::test]
    async fn vfs_conformance_if_match_conflict_fs() {
        let (_d, v) = fresh();
        conformance::if_match_conflict(&v).await;
    }
    #[tokio::test]
    async fn vfs_conformance_if_none_match_create_only_fs() {
        let (_d, v) = fresh();
        conformance::if_none_match_create_only(&v).await;
    }
    #[tokio::test]
    async fn vfs_conformance_list_dir_fs() {
        let (_d, v) = fresh();
        conformance::list_dir(&v).await;
    }
    #[tokio::test]
    async fn vfs_conformance_delete_missing_not_found_fs() {
        let (_d, v) = fresh();
        conformance::delete_missing_not_found(&v).await;
    }
    #[tokio::test]
    async fn vfs_conformance_shape_conflicts_fs() {
        let (_d, v) = fresh();
        conformance::shape_conflicts(&v).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fs_vfs_sandboxed() {
        let (d, v) = fresh();
        // A secret outside the root, and a symlink to it planted inside the root.
        let outside = d.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), b"top secret").unwrap();
        std::os::unix::fs::symlink(&outside, v.root().join("escape")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), v.root().join("link")).unwrap();

        let esc = VfsPath::parse("/escape/secret").unwrap();
        assert!(matches!(v.read(&esc).await, Err(VfsError::InvalidPath(_))));
        assert!(matches!(
            v.write(&esc, b"pwned", Precondition::None).await,
            Err(VfsError::InvalidPath(_))
        ));
        assert!(matches!(
            v.delete(&esc, Precondition::None).await,
            Err(VfsError::InvalidPath(_))
        ));
        let link = VfsPath::parse("/link").unwrap();
        assert!(matches!(v.read(&link).await, Err(VfsError::InvalidPath(_))));
        assert!(matches!(v.stat(&link).await, Err(VfsError::InvalidPath(_))));
        // Symlinks are never listed.
        assert!(
            v.list(&VfsPath::parse("/").unwrap())
                .await
                .unwrap()
                .is_empty()
        );
        // The outside file is untouched.
        assert_eq!(fs::read(outside.join("secret")).unwrap(), b"top secret");
        // `..` never reaches the backend.
        assert!(VfsPath::parse("/../outside/secret").is_err());
        // Temp names are not addressable.
        assert!(matches!(
            v.write(
                &VfsPath::parse("/.vfs-tmp-x").unwrap(),
                b"x",
                Precondition::None
            )
            .await,
            Err(VfsError::InvalidPath(_))
        ));
    }

    #[tokio::test]
    async fn fs_vfs_survives_reopen_and_prunes_dirs() {
        let (d, v) = fresh();
        let p = VfsPath::parse("/modules/billing/1.0.0/descriptor.json").unwrap();
        let meta = v.write(&p, b"{}", Precondition::None).await.unwrap();
        drop(v);
        let v = LocalFsVfs::open(d.path().join("root")).unwrap();
        assert_eq!(v.stat(&p).await.unwrap().unwrap().etag, meta.etag);
        v.delete(&p, Precondition::None).await.unwrap();
        assert!(!v.root().join("modules").exists(), "empty dirs pruned");
        let ro = LocalFsVfs::open_read_only(d.path().join("root")).unwrap();
        assert!(matches!(
            ro.write(&p, b"x", Precondition::None).await,
            Err(VfsError::ReadOnly)
        ));
    }
}
