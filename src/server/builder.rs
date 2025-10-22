//! ServerBuilder for fluent API to build HTTP servers

use super::entity_registry::EntityRegistry;
use super::router::build_link_routes;
use crate::config::LinksConfig;
use crate::core::module::Module;
use crate::core::service::LinkService;
use crate::core::EntityFetcher;
use crate::links::handlers::AppState;
use crate::links::registry::LinkRouteRegistry;
use anyhow::Result;
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;

/// Builder for creating HTTP servers with auto-registered routes
///
/// # Example
///
/// ```ignore
/// let app = ServerBuilder::new()
///     .with_link_service(InMemoryLinkService::new())
///     .register_module(MyModule)
///     .build()?;
/// ```
pub struct ServerBuilder {
    link_service: Option<Arc<dyn LinkService>>,
    entity_registry: EntityRegistry,
    configs: Vec<LinksConfig>,
    modules: Vec<Arc<dyn Module>>,
}

impl ServerBuilder {
    /// Create a new ServerBuilder
    pub fn new() -> Self {
        Self {
            link_service: None,
            entity_registry: EntityRegistry::new(),
            configs: Vec::new(),
            modules: Vec::new(),
        }
    }

    /// Set the link service (required)
    pub fn with_link_service(mut self, service: impl LinkService + 'static) -> Self {
        self.link_service = Some(Arc::new(service));
        self
    }

    /// Register a module
    ///
    /// This will:
    /// 1. Load the module's configuration
    /// 2. Register all entities from the module
    /// 3. Store the module for entity fetching
    pub fn register_module(mut self, module: impl Module + 'static) -> Result<Self> {
        let module = Arc::new(module);
        
        // Load the module's configuration
        let config = module.links_config()?;
        self.configs.push(config);

        // Register entities from the module
        module.register_entities(&mut self.entity_registry);
        
        // Store module for fetchers
        self.modules.push(module);

        Ok(self)
    }

    /// Build the final router
    ///
    /// This generates:
    /// - CRUD routes for all registered entities
    /// - Link routes (bidirectional)
    /// - Introspection routes
    pub fn build(mut self) -> Result<Router> {
        // Merge all configs
        let merged_config = self.merge_configs()?;
        let config = Arc::new(merged_config);

        // Extract link service
        let link_service = self
            .link_service
            .take()
            .ok_or_else(|| anyhow::anyhow!("LinkService is required. Call .with_link_service()"))?;

        // Create link registry
        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));

        // Build entity fetchers map from all modules
        let mut fetchers_map: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        for module in &self.modules {
            for entity_type in module.entity_types() {
                if let Some(fetcher) = module.get_entity_fetcher(entity_type) {
                    fetchers_map.insert(entity_type.to_string(), fetcher);
                }
            }
        }

        // Create link app state
        let link_state = AppState {
            link_service,
            config,
            registry,
            entity_fetchers: Arc::new(fetchers_map),
        };

        // Build entity routes
        let entity_routes = self.entity_registry.build_routes();

        // Build link routes
        let link_routes = build_link_routes(link_state);

        // Merge all routes
        let app = entity_routes.merge(link_routes);

        Ok(app)
    }

    /// Merge all configurations from registered modules
    fn merge_configs(&self) -> Result<LinksConfig> {
        if self.configs.is_empty() {
            return Ok(LinksConfig {
                entities: vec![],
                links: vec![],
                validation_rules: None,
            });
        }

        // For now, just use the first config
        // TODO: Implement proper config merging
        Ok(self.configs[0].clone())
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}
