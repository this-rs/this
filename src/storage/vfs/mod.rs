//! Virtual file system and module store (interfaces).
//!
//! The **contract** — `VfsPath`, `VfsMeta`, `VfsEntry`, `Precondition`, `VfsCaps`,
//! `VfsError`, the `Vfs` trait, `etag_of` and the precondition rules — lives in the
//! `this-vfs` crate, because it has to compile for `wasm32`: a page must be able to
//! store a module it just compiled, and load it back.
//!
//! What stays here is everything that cannot be portable: `fs.rs` (`std::fs`),
//! `obrain.rs`, `module_store.rs` (the module store over a `Vfs`), and the
//! `ModuleStore` trait of the server side.

use async_trait::async_trait;
use this_descriptor::{ContractError, MessageBundle, ModuleDescriptor};

pub mod fs;
pub mod module_store;
#[cfg(feature = "obrain")]
pub mod obrain;

pub use fs::LocalFsVfs;
pub use module_store::{StoredModule, StoredVersion, VfsModuleStore, read_source};
#[cfg(feature = "obrain")]
pub use obrain::ObrainVfs;

// Le contrat vient de `this-vfs` : une seule implémentation, partagée avec le client.
pub use this_vfs::{
    InMemoryVfs, Precondition, Vfs, VfsCaps, VfsEntry, VfsError, VfsMeta, VfsPath,
    check_precondition, etag_of,
};

/// Compatibilité : `storage::vfs::memory::InMemoryVfs` reste le chemin historique.
pub mod memory {
    pub use this_vfs::InMemoryVfs;
}

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
