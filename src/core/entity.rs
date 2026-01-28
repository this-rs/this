//! Entity traits defining the core abstraction for all data types

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Base trait for all entities in the system.
///
/// This trait provides the fundamental metadata needed for any entity type.
/// All entities have:
/// - id: Unique identifier
/// - type: Entity type name (e.g., "user", "product")
/// - created_at: Creation timestamp
/// - updated_at: Last modification timestamp
/// - deleted_at: Soft deletion timestamp (optional)
/// - status: Current status of the entity
///
/// Note: Service access is handled separately via EntityFetcher/EntityCreator traits
/// to maintain single responsibility principle.
pub trait Entity: Clone + Send + Sync + 'static {
    /// The plural resource name used in URLs (e.g., "users", "companies")
    fn resource_name() -> &'static str;

    /// The singular resource name (e.g., "user", "company")
    fn resource_name_singular() -> &'static str;

    // === Core Entity Fields ===

    /// Get the unique identifier for this entity instance
    fn id(&self) -> Uuid;

    /// Get the entity type name
    fn entity_type(&self) -> &str;

    /// Get the creation timestamp
    fn created_at(&self) -> DateTime<Utc>;

    /// Get the last update timestamp
    fn updated_at(&self) -> DateTime<Utc>;

    /// Get the deletion timestamp (soft delete)
    fn deleted_at(&self) -> Option<DateTime<Utc>>;

    /// Get the entity status
    fn status(&self) -> &str;

    // === Utility Methods ===

    /// Get the tenant ID for multi-tenant isolation.
    ///
    /// Returns None by default for single-tenant applications or system-wide entities.
    /// Override this method to enable multi-tenancy for specific entity types.
    ///
    /// # Multi-Tenant Usage
    ///
    /// ```rust,ignore
    /// impl Entity for MyEntity {
    ///     fn tenant_id(&self) -> Option<Uuid> {
    ///         self.tenant_id  // Return actual tenant_id field
    ///     }
    /// }
    /// ```
    fn tenant_id(&self) -> Option<Uuid> {
        None
    }

    /// Check if the entity has been soft-deleted
    fn is_deleted(&self) -> bool {
        self.deleted_at().is_some()
    }

    /// Check if the entity is active (status == "active" and not deleted)
    fn is_active(&self) -> bool {
        self.status() == "active" && !self.is_deleted()
    }
}

/// Trait for data entities that represent concrete domain objects.
///
/// Data entities extend the base Entity with:
/// - name: A human-readable name
/// - indexed_fields: Fields that can be searched
/// - field_value: Dynamic field access
pub trait Data: Entity {
    /// Get the name of this data entity
    fn name(&self) -> &str;

    /// List of fields that should be indexed for searching
    fn indexed_fields() -> &'static [&'static str];

    /// Get the value of a specific field by name
    fn field_value(&self, field: &str) -> Option<crate::core::field::FieldValue>;

    /// Display the entity for debugging
    fn display(&self) {
        println!(
            "[{}] {} - {} ({})",
            self.id(),
            self.entity_type(),
            self.name(),
            self.status()
        );
    }
}

/// Trait for link entities that represent relationships between entities.
///
/// Links extend the base Entity with:
/// - source_id: The ID of the source entity
/// - target_id: The ID of the target entity
/// - link_type: The type of relationship
pub trait Link: Entity {
    /// Get the source entity ID
    fn source_id(&self) -> Uuid;

    /// Get the target entity ID
    fn target_id(&self) -> Uuid;

    /// Get the link type (e.g., "owner", "worker")
    fn link_type(&self) -> &str;

    /// Display the link for debugging
    fn display(&self) {
        println!(
            "[{}] {} → {} (type: {}, status: {})",
            self.id(),
            self.source_id(),
            self.target_id(),
            self.link_type(),
            self.status()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    // Example entity for testing trait definitions
    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestEntity {
        id: Uuid,
        entity_type: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        deleted_at: Option<DateTime<Utc>>,
        status: String,
    }

    impl Entity for TestEntity {
        fn resource_name() -> &'static str {
            "test_entities"
        }

        fn resource_name_singular() -> &'static str {
            "test_entity"
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

    #[test]
    fn test_entity_is_deleted() {
        let now = Utc::now();
        let mut entity = TestEntity {
            id: Uuid::new_v4(),
            entity_type: "test".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".to_string(),
        };

        assert!(!entity.is_deleted());
        assert!(entity.is_active());

        entity.deleted_at = Some(now);
        assert!(entity.is_deleted());
        assert!(!entity.is_active());
    }

    #[test]
    fn test_entity_metadata() {
        assert_eq!(TestEntity::resource_name(), "test_entities");
        assert_eq!(TestEntity::resource_name_singular(), "test_entity");
    }
}
