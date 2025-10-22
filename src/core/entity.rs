//! Entity traits defining the core abstraction for all data types

use crate::core::field::FieldValue;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

/// Base trait for all entities in the system.
///
/// This trait provides the fundamental metadata needed to work with any entity type,
/// including routing information and service access.
pub trait Entity: Sized + Send + Sync + 'static {
    /// The service type that handles operations for this entity
    type Service: Send + Sync;

    /// The plural resource name used in URLs (e.g., "users", "companies")
    fn resource_name() -> &'static str;

    /// The singular resource name (e.g., "user", "company")
    fn resource_name_singular() -> &'static str;

    /// Extract the service instance from the application host/state
    ///
    /// This allows the framework to access entity-specific services without
    /// coupling to specific service implementations
    fn service_from_host(host: &Arc<dyn std::any::Any + Send + Sync>)
        -> Result<Arc<Self::Service>>;
}

/// Trait for data entities that represent concrete domain objects.
///
/// Data entities are the primary building blocks of the system. They have:
/// - A unique identifier
/// - Tenant isolation
/// - Searchable fields
/// - Type information
pub trait Data: Entity {
    /// Get the unique identifier for this entity instance
    fn id(&self) -> Uuid;

    /// Get the tenant ID for multi-tenant isolation
    fn tenant_id(&self) -> Uuid;

    /// List of fields that should be indexed for searching
    fn indexed_fields() -> &'static [&'static str];

    /// Get the value of a specific field by name
    ///
    /// Returns None if the field doesn't exist or can't be converted
    fn field_value(&self, field: &str) -> Option<FieldValue>;

    /// Get the type name of this entity (defaults to singular resource name)
    fn type_name() -> &'static str {
        Self::resource_name_singular()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Example entity for testing
    #[derive(Clone)]
    struct TestEntity {
        id: Uuid,
        tenant_id: Uuid,
        name: String,
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
    }

    impl Data for TestEntity {
        fn id(&self) -> Uuid {
            self.id
        }

        fn tenant_id(&self) -> Uuid {
            self.tenant_id
        }

        fn indexed_fields() -> &'static [&'static str] {
            &["name"]
        }

        fn field_value(&self, field: &str) -> Option<FieldValue> {
            match field {
                "name" => Some(FieldValue::String(self.name.clone())),
                _ => None,
            }
        }
    }

    #[test]
    fn test_entity_metadata() {
        assert_eq!(TestEntity::resource_name(), "test_entities");
        assert_eq!(TestEntity::resource_name_singular(), "test_entity");
        assert_eq!(TestEntity::type_name(), "test_entity");
    }
}
