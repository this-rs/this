//! Module system for This-RS
//!
//! Defines traits for microservice modules

use crate::config::LinksConfig;
use crate::server::entity_registry::EntityRegistry;
use anyhow::Result;

/// Trait for a microservice module
pub trait Module: Send + Sync {
    /// Unique module name
    fn name(&self) -> &str;

    /// Module version
    fn version(&self) -> &str {
        "1.0.0"
    }

    /// List of entity types managed by this module
    fn entity_types(&self) -> Vec<&str>;

    /// Load links configuration
    fn links_config(&self) -> Result<LinksConfig>;

    /// Register entities with the entity registry
    ///
    /// This method should register all entity descriptors for the module.
    /// Each entity descriptor provides the CRUD routes for that entity.
    fn register_entities(&self, registry: &mut EntityRegistry);
}
