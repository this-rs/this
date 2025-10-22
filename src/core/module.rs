//! Module system for This-RS
//!
//! Defines traits for microservice modules

use crate::config::LinksConfig;
use crate::server::entity_registry::EntityRegistry;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

/// Trait for fetching entities dynamically
///
/// This allows the link system to enrich links with full entity data
/// without knowing the concrete entity types at compile time.
#[async_trait]
pub trait EntityFetcher: Send + Sync {
    /// Fetch an entity by ID and return it as JSON
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant ID for isolation
    /// * `entity_id` - The unique ID of the entity to fetch
    ///
    /// # Returns
    /// The entity serialized as JSON, or an error if not found
    async fn fetch_as_json(&self, tenant_id: &Uuid, entity_id: &Uuid) -> Result<serde_json::Value>;
}

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

    /// Get an entity fetcher for a specific entity type
    ///
    /// This allows the framework to dynamically load entities when enriching links.
    ///
    /// # Arguments
    /// * `entity_type` - The type of entity (e.g., "order", "invoice")
    ///
    /// # Returns
    /// An `EntityFetcher` implementation, or `None` if the entity type is not managed by this module
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>>;
}
