//! Virtual file system and module store (interfaces). Implemented by the `vfs` stream:
//! `memory.rs` (InMemoryVfs), `fs.rs` (sandboxed LocalFsVfs), `obrain.rs` (feature
//! `obrain`), `module_store.rs` (`VfsModuleStore`, the `ModuleStore` over a `Vfs`).
//!
//! Storage is OPTIONAL: without a `ModuleStore`, only compiled modules exist.

use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, MessageBundle, ModuleDescriptor};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::fmt;

pub mod fs;
pub mod memory;
pub mod module_store;
#[cfg(feature = "obrain")]
pub mod obrain;

#[cfg(test)]
pub(crate) mod conformance;

pub use fs::LocalFsVfs;
pub use memory::InMemoryVfs;
pub use module_store::{StoredModule, StoredVersion, VfsModuleStore, read_source};
#[cfg(feature = "obrain")]
pub use obrain::ObrainVfs;

/// Etag of a content: sha256, lower-case hex. Same function for every backend, so an
/// etag read from one backend is comparable with one computed by the caller.
pub fn etag_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Shared precondition semantics (every backend calls it under its write lock):
/// - `IfMatch(e)`: the path must exist with etag `e`;
/// - `IfNoneMatch`: the path must not exist;
/// - `None`: always passes.
pub fn check_precondition(
    path: &VfsPath,
    current: Option<&str>,
    pre: &Precondition,
) -> Result<(), VfsError> {
    let ok = match pre {
        Precondition::None => true,
        Precondition::IfMatch(want) => current == Some(want.as_str()),
        Precondition::IfNoneMatch => current.is_none(),
    };
    if ok {
        Ok(())
    } else {
        Err(VfsError::PreconditionFailed {
            path: path.clone(),
            current: current.map(str::to_string),
        })
    }
}

/// Absolute, normalized path: starts with `/`, no `..`, `.`, `\`, NUL or empty segment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VfsPath(String);

impl VfsPath {
    pub fn parse(s: &str) -> Result<Self, VfsError> {
        let bad = || VfsError::InvalidPath(s.to_string());
        let rest = s.strip_prefix('/').ok_or_else(bad)?;
        if s.contains('\\') || s.contains('\0') {
            return Err(bad());
        }
        if rest.is_empty() {
            return Ok(Self("/".into()));
        }
        for seg in rest.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(bad());
            }
        }
        Ok(Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn join(&self, segment: &str) -> Result<Self, VfsError> {
        let base = self.0.trim_end_matches('/');
        Self::parse(&format!("{base}/{segment}"))
    }
    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }
    /// Segments, root excluded (`/a/b` -> `["a", "b"]`).
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|s| !s.is_empty())
    }
    /// Last segment (`None` for the root).
    pub fn file_name(&self) -> Option<&str> {
        self.segments().last()
    }
    /// Parent directory (`None` for the root).
    pub fn parent(&self) -> Option<VfsPath> {
        if self.is_root() {
            return None;
        }
        match self.0.rfind('/') {
            Some(0) => Some(Self("/".into())),
            Some(i) => Some(Self(self.0[..i].to_string())),
            None => None,
        }
    }
    /// `true` when `self` is strictly below `dir`.
    pub fn is_below(&self, dir: &VfsPath) -> bool {
        if dir.is_root() {
            return !self.is_root();
        }
        self.0.len() > dir.0.len()
            && self.0.starts_with(&dir.0)
            && self.0.as_bytes()[dir.0.len()] == b'/'
    }
    /// Child of `dir` on the way to `self` (`/a/b/c` below `/a` -> `/a/b`).
    pub fn child_of(&self, dir: &VfsPath) -> Option<VfsPath> {
        if !self.is_below(dir) {
            return None;
        }
        let start = if dir.is_root() { 1 } else { dir.0.len() + 1 };
        let end = self.0[start..]
            .find('/')
            .map(|i| start + i)
            .unwrap_or(self.0.len());
        Some(Self(self.0[..end].to_string()))
    }
}
impl fmt::Display for VfsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VfsMeta {
    pub size: u64,
    /// sha256 hex of the content.
    pub etag: String,
    pub modified: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsEntry {
    File { path: VfsPath, meta: VfsMeta },
    Dir { path: VfsPath },
}

/// Write / delete precondition (optimistic concurrency).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Precondition {
    #[default]
    None,
    /// Current etag must equal.
    IfMatch(String),
    /// The path must not exist (create only).
    IfNoneMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VfsCaps {
    pub read_only: bool,
    pub versioned: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("not found: {0}")]
    NotFound(VfsPath),
    #[error("precondition failed on {path}")]
    PreconditionFailed {
        path: VfsPath,
        current: Option<String>,
    },
    #[error("read-only storage")]
    ReadOnly,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("backend: {0}")]
    Backend(#[from] anyhow::Error),
}

impl From<VfsError> for ContractError {
    fn from(e: VfsError) -> Self {
        match e {
            VfsError::NotFound(p) => {
                ContractError::framework(codes::VFS_NOT_FOUND).with_param("path", p.as_str())
            }
            VfsError::PreconditionFailed { path, .. } => {
                ContractError::framework(codes::VFS_PRECONDITION_FAILED)
                    .with_param("path", path.as_str())
            }
            VfsError::ReadOnly => ContractError::framework(codes::VFS_READ_ONLY),
            VfsError::InvalidPath(p) => {
                ContractError::framework(codes::VFS_INVALID_PATH).with_param("path", p)
            }
            VfsError::Backend(err) => crate::core::errors::from_anyhow(&err),
        }
    }
}

/// Storage of immutable-by-convention files (modules, catalogs, wasm artifacts).
///
/// Semantics every backend honours (bench: `conformance::run_all`):
/// - directories are IMPLICIT: a directory exists while a file exists below it; the
///   root `/` always exists and cannot be written;
/// - `list(dir)` returns the immediate children of `dir`, sorted by path; a missing
///   directory lists as empty (not an error);
/// - `read` / `delete` of a missing file: `NotFound`; `stat` of a missing file: `Ok(None)`;
/// - a file cannot be written where a directory exists, nor below a file: `InvalidPath`;
/// - writes are atomic per file (a reader never sees a partial content) and the
///   precondition is checked under the backend's write lock (`check_precondition`);
/// - `etag` = `etag_of(content)`, so equal contents have equal etags on every backend.
#[async_trait]
pub trait Vfs: Send + Sync {
    async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError>;
    async fn write(
        &self,
        path: &VfsPath,
        bytes: &[u8],
        pre: Precondition,
    ) -> Result<VfsMeta, VfsError>;
    async fn stat(&self, path: &VfsPath) -> Result<Option<VfsMeta>, VfsError>;
    async fn list(&self, dir: &VfsPath) -> Result<Vec<VfsEntry>, VfsError>;
    async fn delete(&self, path: &VfsPath, pre: Precondition) -> Result<(), VfsError>;
    fn caps(&self) -> VfsCaps;
}

/// Store of declarative modules. Layout of the VFS implementation:
/// `/modules/<name>/<version>/descriptor.json` (no texts),
/// `/modules/<name>/<version>/i18n/<locale>.json` (one `MessageBundle` per locale),
/// `/modules/<name>/<version>/assets/**`.
///
/// Contract for implementations:
/// - `put` runs `ModuleDescriptor::check` with `CheckProfile::Stored` (explicit policies,
///   no `public` unless allowed) AND `this_descriptor::compat::check_compat` against the
///   highest stored version (tags never moved nor reused);
/// - `put` and `remove` are authorized by the caller with the admin operation
///   `this.module.load` (see the RFC trust model);
/// - `get(name, None)` returns the highest semver version.
#[async_trait]
pub trait ModuleStore: Send + Sync {
    async fn put(
        &self,
        descriptor: &ModuleDescriptor,
        bundles: &[MessageBundle],
    ) -> Result<VfsMeta, ContractError>;
    async fn get(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<ModuleDescriptor, ContractError>;
    async fn bundle(
        &self,
        name: &str,
        version: &str,
        locale: &str,
    ) -> Result<Option<MessageBundle>, ContractError>;
    async fn list(&self) -> Result<Vec<(String, String)>, ContractError>;
    async fn remove(&self, name: &str, version: &str) -> Result<(), ContractError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfs_path_rejects_dotdot_and_friends() {
        for bad in [
            "", "rel/x", "/a/../b", "/a//b", "/a/./b", "/a\\b", "/a\0b", "/a/",
        ] {
            assert!(VfsPath::parse(bad).is_err(), "{bad:?}");
        }
        let p = VfsPath::parse("/modules/billing").unwrap();
        assert_eq!(p.join("1.0.0").unwrap().as_str(), "/modules/billing/1.0.0");
        assert!(p.join("..").is_err());
        let c: ContractError = VfsError::PreconditionFailed {
            path: p,
            current: None,
        }
        .into();
        assert_eq!(c.kind, crate::descriptor::ErrorKind::Aborted);
    }
}
