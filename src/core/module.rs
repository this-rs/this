//! Module system for this-rs
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
    /// * `entity_id` - The unique ID of the entity to fetch
    ///
    /// # Returns
    /// The entity serialized as JSON, or an error if not found
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value>;

    /// Get a sample entity for schema introspection
    ///
    /// This method returns an entity with all fields populated (can be dummy data)
    /// to allow the GraphQL schema generator to discover the entity structure.
    ///
    /// Default implementation returns an empty object.
    async fn get_sample_entity(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({}))
    }

    /// List entities with pagination
    ///
    /// # Arguments
    /// * `limit` - Maximum number of entities to return
    /// * `offset` - Number of entities to skip
    ///
    /// # Returns
    /// A vector of entities serialized as JSON
    ///
    /// Default implementation returns an empty list.
    async fn list_as_json(
        &self,
        _limit: Option<i32>,
        _offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        Ok(vec![])
    }
}

/// Trait for creating entities dynamically
///
/// This allows the link system to create new entities with automatic linking
/// without knowing the concrete entity types at compile time.
#[async_trait]
pub trait EntityCreator: Send + Sync {
    /// Create a new entity from JSON data
    ///
    /// # Arguments
    /// * `entity_data` - The entity data as JSON
    ///
    /// # Returns
    /// The created entity serialized as JSON (with generated ID, timestamps, etc.)
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value>;

    /// Update an existing entity from JSON data
    ///
    /// # Arguments
    /// * `entity_id` - The ID of the entity to update
    /// * `entity_data` - The updated entity data as JSON
    ///
    /// # Returns
    /// The updated entity serialized as JSON
    ///
    /// Default implementation returns an error.
    async fn update_from_json(
        &self,
        _entity_id: &Uuid,
        _entity_data: serde_json::Value,
    ) -> Result<serde_json::Value> {
        Err(anyhow::anyhow!(
            "Update operation not implemented for this entity type"
        ))
    }

    /// Delete an entity by ID
    ///
    /// # Arguments
    /// * `entity_id` - The ID of the entity to delete
    ///
    /// # Returns
    /// Ok(()) if successful, error otherwise
    ///
    /// Default implementation returns an error.
    async fn delete(&self, _entity_id: &Uuid) -> Result<()> {
        Err(anyhow::anyhow!(
            "Delete operation not implemented for this entity type"
        ))
    }
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

    /// Get an entity creator for a specific entity type
    ///
    /// This allows the framework to create new entities dynamically when creating linked entities.
    ///
    /// # Arguments
    /// * `entity_type` - The type of entity (e.g., "order", "invoice")
    ///
    /// # Returns
    /// An `EntityCreator` implementation, or `None` if the entity type is not managed by this module
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>>;
}
