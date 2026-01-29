//! Service traits for data and link operations

use crate::core::error::{LinkError, ThisError, ThisResult};
use crate::core::{link::LinkEntity, Data};
use async_trait::async_trait;
use uuid::Uuid;

/// Service trait for managing data entities
///
/// Implementations provide CRUD operations for a specific entity type.
/// The framework is agnostic to the underlying storage mechanism.
#[async_trait]
pub trait DataService<T: Data>: Send + Sync {
    /// Create a new entity
    async fn create(&self, entity: T) -> ThisResult<T>;

    /// Get an entity by ID
    async fn get(&self, id: &Uuid) -> ThisResult<Option<T>>;

    /// List all entities
    async fn list(&self) -> ThisResult<Vec<T>>;

    /// Update an existing entity
    async fn update(&self, id: &Uuid, entity: T) -> ThisResult<T>;

    /// Delete an entity
    async fn delete(&self, id: &Uuid) -> ThisResult<()>;

    /// Search entities by field values
    async fn search(&self, field: &str, value: &str) -> ThisResult<Vec<T>>;
}

/// Service trait for managing links between entities
///
/// This service is completely agnostic to entity types - it only manages
/// relationships using UUIDs and string identifiers.
///
/// # Error Handling
///
/// All methods return `ThisResult<T>` which provides typed errors:
/// - `LinkError::NotFoundById` when a link ID doesn't exist
/// - `LinkError::NotFound` when a specific link relationship doesn't exist
/// - `StorageError::*` for underlying storage issues
///
/// # Example
///
/// ```rust,ignore
/// use this::prelude::*;
///
/// async fn example(service: &impl LinkService) -> ThisResult<()> {
///     let link = LinkEntity::new("owner", user_id, car_id, None);
///
///     match service.create(link).await {
///         Ok(created) => println!("Created link: {}", created.id),
///         Err(ThisError::Link(LinkError::AlreadyExists { .. })) => {
///             println!("Link already exists");
///         }
///         Err(e) => return Err(e),
///     }
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait LinkService: Send + Sync {
    /// Create a new link between two entities
    ///
    /// # Errors
    ///
    /// - `LinkError::AlreadyExists` if a link with the same source, target, and type exists
    /// - `StorageError::*` for storage failures
    async fn create(&self, link: LinkEntity) -> ThisResult<LinkEntity>;

    /// Get a specific link by ID
    ///
    /// Returns `Ok(None)` if the link doesn't exist.
    async fn get(&self, id: &Uuid) -> ThisResult<Option<LinkEntity>>;

    /// List all links
    async fn list(&self) -> ThisResult<Vec<LinkEntity>>;

    /// Find links by source entity
    ///
    /// Optionally filter by link_type and/or target_type
    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> ThisResult<Vec<LinkEntity>>;

    /// Find links by target entity
    ///
    /// Optionally filter by link_type and/or source_type
    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> ThisResult<Vec<LinkEntity>>;

    /// Update a link's metadata
    ///
    /// # Errors
    ///
    /// - `LinkError::NotFoundById` if the link doesn't exist
    /// - `StorageError::*` for storage failures
    async fn update(&self, id: &Uuid, link: LinkEntity) -> ThisResult<LinkEntity>;

    /// Delete a link
    ///
    /// This operation is idempotent - deleting a non-existent link succeeds.
    async fn delete(&self, id: &Uuid) -> ThisResult<()>;

    /// Delete all links involving a specific entity
    ///
    /// Used when deleting an entity to maintain referential integrity.
    /// This operation is idempotent.
    async fn delete_by_entity(&self, entity_id: &Uuid) -> ThisResult<()>;

    /// Get a link by ID, returning an error if not found
    ///
    /// This is a convenience method that wraps `get()` and returns
    /// `LinkError::NotFoundById` instead of `None`.
    async fn get_or_error(&self, id: &Uuid) -> ThisResult<LinkEntity> {
        self.get(id).await?.ok_or_else(|| {
            ThisError::Link(LinkError::NotFoundById { id: *id })
        })
    }
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
        fn resource_name() -> &'static str {
            "tests"
        }

        fn resource_name_singular() -> &'static str {
            "test"
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
    async fn generic_create<T, S>(service: &S, entity: T) -> ThisResult<T>
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
