//! Entity registry for managing entity descriptors and auto-generating CRUD routes

use axum::Router;
use std::collections::HashMap;

/// Trait that describes how to build routes for an entity
///
/// Each entity (Order, Invoice, Payment, etc.) should implement this trait
/// to provide its CRUD routes.
pub trait EntityDescriptor: Send + Sync {
    /// The entity type name (singular, e.g., "order")
    fn entity_type(&self) -> &str;

    /// The plural form (e.g., "orders")
    fn plural(&self) -> &str;

    /// Build the CRUD routes for this entity
    ///
    /// Should return a Router with routes like:
    /// - GET /{plural}
    /// - POST /{plural}
    /// - GET /{plural}/:id
    fn build_routes(&self) -> Router;
}

/// Registry for all entities in the application
///
/// This registry collects entity descriptors from all registered modules
/// and can generate a router with all CRUD routes.
#[derive(Default)]
pub struct EntityRegistry {
    descriptors: HashMap<String, Box<dyn EntityDescriptor>>,
}

impl EntityRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            descriptors: HashMap::new(),
        }
    }

    /// Register an entity descriptor
    ///
    /// The entity type name will be used as the key.
    pub fn register(&mut self, descriptor: Box<dyn EntityDescriptor>) {
        let entity_type = descriptor.entity_type().to_string();
        self.descriptors.insert(entity_type, descriptor);
    }

    /// Build a router with all registered entity routes
    ///
    /// This merges all entity routes into a single router.
    pub fn build_routes(&self) -> Router {
        let mut router = Router::new();

        for descriptor in self.descriptors.values() {
            router = router.merge(descriptor.build_routes());
        }

        router
    }

    /// Get all registered entity types
    pub fn entity_types(&self) -> Vec<&str> {
        self.descriptors.keys().map(|s| s.as_str()).collect()
    }
}
