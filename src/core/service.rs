//! Service traits for data and link operations

use crate::core::{Data, EntityReference, Link};
use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

/// Service trait for managing data entities
///
/// Implementations provide CRUD operations for a specific entity type.
/// The framework is agnostic to the underlying storage mechanism.
#[async_trait]
pub trait DataService<T: Data>: Send + Sync {
    /// Create a new entity
    async fn create(&self, tenant_id: &Uuid, entity: T) -> Result<T>;

    /// Get an entity by ID
    async fn get(&self, tenant_id: &Uuid, id: &Uuid) -> Result<Option<T>>;

    /// List all entities for a tenant
    async fn list(&self, tenant_id: &Uuid) -> Result<Vec<T>>;

    /// Update an existing entity
    async fn update(&self, tenant_id: &Uuid, id: &Uuid, entity: T) -> Result<T>;

    /// Delete an entity
    async fn delete(&self, tenant_id: &Uuid, id: &Uuid) -> Result<()>;

    /// Search entities by field values
    async fn search(&self, tenant_id: &Uuid, field: &str, value: &str) -> Result<Vec<T>>;
}

/// Service trait for managing links between entities
///
/// This service is completely agnostic to entity types - it only knows
/// about EntityReferences and link types (both Strings).
#[async_trait]
pub trait LinkService: Send + Sync {
    /// Create a new link between two entities
    async fn create(
        &self,
        tenant_id: &Uuid,
        link_type: &str,
        source: EntityReference,
        target: EntityReference,
        metadata: Option<serde_json::Value>,
    ) -> Result<Link>;

    /// Get a specific link by ID
    async fn get(&self, tenant_id: &Uuid, id: &Uuid) -> Result<Option<Link>>;

    /// List all links for a tenant
    async fn list(&self, tenant_id: &Uuid) -> Result<Vec<Link>>;

    /// Find links by source entity
    ///
    /// Optionally filter by link_type and/or target_type
    async fn find_by_source(
        &self,
        tenant_id: &Uuid,
        source_id: &Uuid,
        source_type: &str,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<Link>>;

    /// Find links by target entity
    ///
    /// Optionally filter by link_type and/or source_type
    async fn find_by_target(
        &self,
        tenant_id: &Uuid,
        target_id: &Uuid,
        target_type: &str,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> Result<Vec<Link>>;

    /// Delete a link
    async fn delete(&self, tenant_id: &Uuid, id: &Uuid) -> Result<()>;

    /// Delete all links involving a specific entity
    ///
    /// Used when deleting an entity to maintain referential integrity
    async fn delete_by_entity(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
        entity_type: &str,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity::Entity;

    // Mock entity for testing
    #[derive(Clone, Debug)]
    struct TestEntity {
        id: Uuid,
        tenant_id: Uuid,
    }

    impl Entity for TestEntity {
        type Service = ();
        fn resource_name() -> &'static str {
            "tests"
        }
        fn resource_name_singular() -> &'static str {
            "test"
        }
        fn service_from_host(
            _: &std::sync::Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<std::sync::Arc<Self::Service>> {
            Ok(std::sync::Arc::new(()))
        }
    }

    impl Data for TestEntity {
        fn id(&self) -> Uuid {
            self.id
        }
        fn tenant_id(&self) -> Uuid {
            self.tenant_id
        }
        fn indexed_fields() -> &'static [&'static str] {
            &[]
        }
        fn field_value(&self, _field: &str) -> Option<crate::core::field::FieldValue> {
            None
        }
    }

    // The traits compile and can be used in generic contexts
    async fn generic_create<T, S>(service: &S, tenant_id: &Uuid, entity: T) -> Result<T>
    where
        T: Data,
        S: DataService<T>,
    {
        service.create(tenant_id, entity).await
    }

    #[test]
    fn test_traits_compile() {
        // This test just verifies that the traits are correctly defined
        // and can be used in generic contexts
    }
}
