//! Service traits for data and link operations

use crate::core::{Data, link::LinkEntity};
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
    async fn create(&self, entity: T) -> Result<T>;

    /// Get an entity by ID
    async fn get(&self, id: &Uuid) -> Result<Option<T>>;

    /// List all entities
    async fn list(&self) -> Result<Vec<T>>;

    /// Update an existing entity
    async fn update(&self, id: &Uuid, entity: T) -> Result<T>;

    /// Delete an entity
    async fn delete(&self, id: &Uuid) -> Result<()>;

    /// Search entities by field values
    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>>;
}

/// Service trait for managing links between entities
///
/// This service is completely agnostic to entity types - it only manages
/// relationships using UUIDs and string identifiers.
#[async_trait]
pub trait LinkService: Send + Sync {
    /// Create a new link between two entities
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity>;

    /// Get a specific link by ID
    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>>;

    /// List all links
    async fn list(&self) -> Result<Vec<LinkEntity>>;

    /// Find links by source entity
    ///
    /// Optionally filter by link_type and/or target_type
    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>>;

    /// Find links by target entity
    ///
    /// Optionally filter by link_type and/or source_type
    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>>;

    /// Update a link's metadata
    ///
    /// This allows updating the metadata associated with a link without
    /// recreating it. Useful for adding/modifying contextual information
    /// like status, dates, permissions, etc.
    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity>;

    /// Delete a link
    async fn delete(&self, id: &Uuid) -> Result<()>;

    /// Delete all links involving a specific entity
    ///
    /// Used when deleting an entity to maintain referential integrity
    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity::Entity;
    use chrono::{DateTime, Utc};

    // Mock entity for testing
    #[allow(dead_code)]
    #[derive(Clone, Debug)]
    struct TestEntity {
        id: Uuid,
        entity_type: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        deleted_at: Option<DateTime<Utc>>,
        status: String,
        name: String,
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

        fn id(&self) -> Uuid {
            self.id
        }

        fn entity_type(&self) -> &str {
            &self.entity_type
        }

        fn created_at(&self) -> DateTime<Utc> {
            self.created_at
        }

        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }

        fn deleted_at(&self) -> Option<DateTime<Utc>> {
            self.deleted_at
        }

        fn status(&self) -> &str {
            &self.status
        }
    }

    impl Data for TestEntity {
        fn name(&self) -> &str {
            &self.name
        }

        fn indexed_fields() -> &'static [&'static str] {
            &[]
        }

        fn field_value(&self, _field: &str) -> Option<crate::core::field::FieldValue> {
            None
        }
    }

    // The traits compile and can be used in generic contexts
    #[allow(dead_code)]
    async fn generic_create<T, S>(service: &S, entity: T) -> Result<T>
    where
        T: Data,
        S: DataService<T>,
    {
        service.create(entity).await
    }

    #[test]
    fn test_traits_compile() {
        // This test just verifies that the traits are correctly defined
        // and can be used in generic contexts
    }
}
