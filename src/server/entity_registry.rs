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

    /// Register an entity descriptor, **refusing** a duplicate (fail-closed).
    ///
    /// Same entity type already registered → `this.module.duplicate_entity`; same plural
    /// served by another entity type → `this.module.duplicate_plural`. The registry is
    /// unchanged on error. [`EntityRegistry::register`] keeps the historical last-wins for
    /// modules without a descriptor.
    #[allow(clippy::result_large_err)] // `ContractError` is the contract (see descriptor crate)
    pub fn try_register(
        &mut self,
        descriptor: Box<dyn EntityDescriptor>,
    ) -> Result<(), crate::descriptor::ContractError> {
        use crate::descriptor::ContractError;
        use crate::descriptor::framework::codes;
        let entity_type = descriptor.entity_type().to_string();
        if self.descriptors.contains_key(&entity_type) {
            return Err(ContractError::framework(codes::MODULE_DUPLICATE_ENTITY)
                .with_param("entity", &entity_type)
                .with_param("modules", "already registered"));
        }
        if self.entity_type_for_plural(descriptor.plural()).is_some() {
            return Err(ContractError::framework(codes::MODULE_DUPLICATE_PLURAL)
                .with_param("plural", descriptor.plural()));
        }
        self.descriptors.insert(entity_type, descriptor);
        Ok(())
    }

    /// Is `entity_type` registered?
    pub fn contains(&self, entity_type: &str) -> bool {
        self.descriptors.contains_key(entity_type)
    }

    /// Remove an entity descriptor (runtime unload of a module). Returns it if present.
    pub fn unregister(&mut self, entity_type: &str) -> Option<Box<dyn EntityDescriptor>> {
        self.descriptors.remove(entity_type)
    }

    /// Take every descriptor out of the registry (sorted by entity type, so the caller's
    /// order is deterministic). Used by the builder to register one module at a time and
    /// decide, per entity, between `try_register` and the legacy `register`.
    pub fn drain(&mut self) -> Vec<Box<dyn EntityDescriptor>> {
        let mut out: Vec<(String, Box<dyn EntityDescriptor>)> = self.descriptors.drain().collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.into_iter().map(|(_, d)| d).collect()
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

#[cfg(test)]
mod try_register_tests {
    use super::*;

    struct D(&'static str, &'static str);
    impl EntityDescriptor for D {
        fn entity_type(&self) -> &str {
            self.0
        }
        fn plural(&self) -> &str {
            self.1
        }
        fn build_routes(&self) -> Router {
            Router::new()
        }
    }

    #[test]
    fn try_register_refuses_duplicate_entity_and_plural() {
        let mut r = EntityRegistry::new();
        r.try_register(Box::new(D("order", "orders"))).unwrap();
        let e = r.try_register(Box::new(D("order", "commandes"))).unwrap_err();
        assert_eq!(e.code.as_str(), "this.module.duplicate_entity");
        let e = r.try_register(Box::new(D("command", "orders"))).unwrap_err();
        assert_eq!(e.code.as_str(), "this.module.duplicate_plural");
        assert_eq!(r.entity_plurals(), vec![("order".into(), "orders".into())]);
        assert!(r.unregister("order").is_some());
        assert!(!r.contains("order"));
    }
}
