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

    /// Les couples `(type d'entité, pluriel)` des descripteurs enregistrés.
    ///
    /// C'est la jointure dont dépend l'**application des politiques** : les
    /// routes qui existent vraiment viennent des descripteurs (leur `plural()`),
    /// et la politique vient de la configuration (par type d'entité). Sans cette
    /// table, un point d'application ne pourrait deviner ni l'un ni l'autre.
    pub fn entity_plurals(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .descriptors
            .values()
            .map(|d| (d.entity_type().to_string(), d.plural().to_string()))
            .collect();
        out.sort();
        out
    }

    /// Le type d'entité d'un pluriel de route, s'il est enregistré.
    pub fn entity_type_for_plural(&self, plural: &str) -> Option<&str> {
        self.descriptors
            .values()
            .find(|d| d.plural() == plural)
            .map(|d| d.entity_type())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal mock EntityDescriptor for testing
    struct MockDescriptor {
        entity_type: String,
        plural: String,
    }

    impl MockDescriptor {
        fn new(entity_type: &str, plural: &str) -> Self {
            Self {
                entity_type: entity_type.to_string(),
                plural: plural.to_string(),
            }
        }
    }

    impl EntityDescriptor for MockDescriptor {
        fn entity_type(&self) -> &str {
            &self.entity_type
        }

        fn plural(&self) -> &str {
            &self.plural
        }

        fn build_routes(&self) -> Router {
            Router::new()
        }
    }

    #[test]
    fn test_new_registry_is_empty() {
        let registry = EntityRegistry::new();
        assert!(registry.entity_types().is_empty());
    }

    #[test]
    fn test_default_registry_is_empty() {
        let registry = EntityRegistry::default();
        assert!(registry.entity_types().is_empty());
    }

    #[test]
    fn test_register_single_entity() {
        let mut registry = EntityRegistry::new();
        registry.register(Box::new(MockDescriptor::new("order", "orders")));
        let types = registry.entity_types();
        assert_eq!(types.len(), 1);
        assert!(types.contains(&"order"));
    }

    #[test]
    fn test_register_multiple_entities() {
        let mut registry = EntityRegistry::new();
        registry.register(Box::new(MockDescriptor::new("order", "orders")));
        registry.register(Box::new(MockDescriptor::new("invoice", "invoices")));
        registry.register(Box::new(MockDescriptor::new("user", "users")));
        assert_eq!(registry.entity_types().len(), 3);
    }

    #[test]
    fn test_register_duplicate_replaces() {
        let mut registry = EntityRegistry::new();
        registry.register(Box::new(MockDescriptor::new("order", "orders")));
        registry.register(Box::new(MockDescriptor::new("order", "commandes")));
        // Same key → replaced, still 1 entry
        assert_eq!(registry.entity_types().len(), 1);
    }

    #[test]
    fn test_build_routes_empty_registry() {
        let registry = EntityRegistry::new();
        let _router = registry.build_routes(); // Should not panic
    }

    #[test]
    fn test_build_routes_with_entities() {
        let mut registry = EntityRegistry::new();
        registry.register(Box::new(MockDescriptor::new("order", "orders")));
        registry.register(Box::new(MockDescriptor::new("invoice", "invoices")));
        let _router = registry.build_routes(); // Should not panic
    }
}
