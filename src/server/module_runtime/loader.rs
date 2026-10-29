//! `ModuleLoader`: turn a source (compiled module or stored descriptor) into the interface
//! the host publishes in its catalog.

use crate::core::module::{EntityCreator, EntityFetcher, Module};
use crate::descriptor::{ContractError, ModuleDescriptor, ModuleState};
use crate::storage::vfs::ModuleStore;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Where a module comes from. One enum, one loader: a `ModuleLoader` trait object only
/// pays off with a second loader (wasm plugins), noted as a follow-up.
#[derive(Clone)]
pub enum ModuleSource {
    Compiled(Arc<dyn Module>),
    Stored {
        store: Arc<dyn ModuleStore>,
        name: String,
        /// `None` = highest stored version.
        version: Option<String>,
    },
}

/// What a loaded module exposes to the host. Published through `HostCatalog` (fetchers,
/// creators, descriptor) and the dispatch layer (routes), never through a fallback.
#[derive(Clone)]
pub struct ModuleInterface {
    pub descriptor: Arc<ModuleDescriptor>,
    pub fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    /// Raw creators; the runtime wraps them (`ValidatingCreator`) when building the snapshot.
    pub creators: HashMap<String, Arc<dyn EntityCreator>>,
    pub routes: Option<axum::Router>,
}

/// Observable status of a module.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ModuleStatus {
    pub name: String,
    pub version: String,
    #[serde(flatten)]
    pub state: ModuleState,
    pub requires: Vec<String>,
}

/// Read and check a module, build its interface. Does NOT activate it.
#[async_trait]
pub trait ModuleLoader: Send + Sync {
    async fn load(&self, source: &ModuleSource) -> Result<ModuleInterface, ContractError>;
}
