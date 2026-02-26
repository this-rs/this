//! Entity traits defining the core abstraction for all data types

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;
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
pub trait Entity: Clone + Send + Sync + 'static {
    /// The service type that handles operations for this entity
    type Service: Send + Sync;

    /// The plural resource name used in URLs (e.g., "users", "companies")
    fn resource_name() -> &'static str;

    /// The singular resource name (e.g., "user", "company")
    fn resource_name_singular() -> &'static str;

    /// Extract the service instance from the application host/state
    fn service_from_host(host: &Arc<dyn std::any::Any + Send + Sync>)
    -> Result<Arc<Self::Service>>;

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
        type Service = ();

        fn resource_name() -> &'static str {
            "test_entities"
        }

        fn resource_name_singular() -> &'static str {
            "test_entity"
        }

        fn service_from_host(
            _host: &Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<Arc<Self::Service>> {
            Ok(Arc::new(()))
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

    #[test]
    fn test_entity_default_tenant_id_is_none() {
        let now = Utc::now();
        let entity = TestEntity {
            id: Uuid::new_v4(),
            entity_type: "test".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".to_string(),
        };
        assert_eq!(entity.tenant_id(), None);
    }

    #[test]
    fn test_entity_is_active_with_inactive_status() {
        let now = Utc::now();
        let entity = TestEntity {
            id: Uuid::new_v4(),
            entity_type: "test".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "inactive".to_string(),
        };
        assert!(!entity.is_active());
        assert!(!entity.is_deleted());
    }

    #[test]
    fn test_entity_service_from_host() {
        let host: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
        let svc = TestEntity::service_from_host(&host).expect("service_from_host should succeed");
        // We just verify it returns successfully; the service is ()
        assert_eq!(*svc, ());
    }

    // --- Link trait ---

    #[derive(Clone, Debug)]
    struct TestLink {
        id: Uuid,
        source_id: Uuid,
        target_id: Uuid,
        link_type: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        deleted_at: Option<DateTime<Utc>>,
        status: String,
    }

    impl Entity for TestLink {
        type Service = ();

        fn resource_name() -> &'static str {
            "test_links"
        }

        fn resource_name_singular() -> &'static str {
            "test_link"
        }

        fn service_from_host(
            _host: &Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<Arc<Self::Service>> {
            Ok(Arc::new(()))
        }

        fn id(&self) -> Uuid {
            self.id
        }

        fn entity_type(&self) -> &str {
            "test_link"
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

    impl Link for TestLink {
        fn source_id(&self) -> Uuid {
            self.source_id
        }

        fn target_id(&self) -> Uuid {
            self.target_id
        }

        fn link_type(&self) -> &str {
            &self.link_type
        }
    }

    #[test]
    fn test_link_accessors() {
        let now = Utc::now();
        let src = Uuid::new_v4();
        let tgt = Uuid::new_v4();
        let link = TestLink {
            id: Uuid::new_v4(),
            source_id: src,
            target_id: tgt,
            link_type: "ownership".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".to_string(),
        };
        assert_eq!(link.source_id(), src);
        assert_eq!(link.target_id(), tgt);
        assert_eq!(link.link_type(), "ownership");
    }

    #[test]
    fn test_link_is_deleted_and_is_active() {
        let now = Utc::now();
        let mut link = TestLink {
            id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            link_type: "ref".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".to_string(),
        };
        assert!(!link.is_deleted());
        assert!(link.is_active());

        link.deleted_at = Some(now);
        assert!(link.is_deleted());
        assert!(!link.is_active());
    }

    #[test]
    fn test_link_display_does_not_panic() {
        let now = Utc::now();
        let link = TestLink {
            id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            link_type: "ref".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".to_string(),
        };
        // Calling display() should not panic
        link.display();
    }

    #[test]
    fn test_link_inactive_status() {
        let now = Utc::now();
        let link = TestLink {
            id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            link_type: "ref".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "suspended".to_string(),
        };
        assert!(!link.is_active());
        assert!(!link.is_deleted());
    }
}
