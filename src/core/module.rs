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

    /// List with a typed query (filter, sort, page), already checked against the
    /// entity's `QuerySchema` by the caller.
    ///
    /// Default: **O(n)** — fetch everything with `list_as_json(None, None)`, then filter,
    /// sort and paginate in memory (filter BEFORE pagination, so pages are correct). A
    /// backend that can push the query down overrides it. Returns `(page, total)`.
    async fn list_query(
        &self,
        query: &crate::descriptor::Query,
    ) -> Result<(Vec<serde_json::Value>, usize)> {
        let all = self.list_as_json(None, None).await?;
        Ok(crate::descriptor::engine::query::apply_in_memory(
            query, all,
        ))
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
    ///
    /// **Défaut : `None`.** Ces deux accesseurs ne servent qu'à la surface
    /// GraphQL et à la résolution de liens par identifiant : un module qui
    /// n'expose pas ces chemins n'a rien à fournir. Les rendre obligatoires
    /// forçait chaque module — et donc chaque projet qui adopte this-rs — à
    /// écrire deux méthodes vides pour compiler.
    fn get_entity_fetcher(&self, _entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        None
    }

    /// Get an entity creator for a specific entity type
    ///
    /// This allows the framework to create new entities dynamically when creating linked entities.
    ///
    /// # Arguments
    /// * `entity_type` - The type of entity (e.g., "order", "invoice")
    ///
    /// # Returns
    /// An `EntityCreator` implementation, or `None` if the entity type is not managed by this module
    ///
    /// **Défaut : `None`** — voir [`Module::get_entity_fetcher`].
    fn get_entity_creator(&self, _entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        None
    }

    /// Descriptor of the module (entities, fields, rules, errors, permissions, messages,
    /// dependencies), declared once.
    ///
    /// **Default: `None`** — an existing module compiles and behaves exactly as before.
    /// When `Some`, `ServerBuilder::build_host` resolves dependencies (topological,
    /// semver, cycles, duplicates), checks that `name`, `version` and `entity_types` match the
    /// descriptor, and that the plural agrees with `EntityDescriptor::plural` and
    /// `EntityConfig::plural`.
    fn descriptor(&self) -> Option<crate::descriptor::ModuleDescriptor> {
        None
    }

    /// Message bundles of the module (translations of its error codes and keys, per locale),
    /// added to `ServerHost::locales` by `ServerBuilder::build_host` so that every surface
    /// (REST, gRPC, GraphQL) renders the module's errors in the request locale, exactly as
    /// for a runtime-loaded module.
    ///
    /// **Default: empty** — an existing module is unchanged. A bundle the catalog refuses
    /// (unknown locale tag, placeholder mismatch...) fails `build_host` with the contract
    /// error, instead of being silently dropped.
    fn bundles(&self) -> Vec<crate::descriptor::MessageBundle> {
        Vec::new()
    }
}
