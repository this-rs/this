//! ServerBuilder for fluent API to build HTTP servers

use super::entity_registry::EntityRegistry;
use super::exposure::RestExposure;
use super::host::ServerHost;
use crate::config::LinksConfig;
use crate::core::events::EventBus;
use crate::core::module::Module;
use crate::core::service::LinkService;
use crate::core::{EntityCreator, EntityFetcher};
use anyhow::Result;
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;

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
    custom_routes: Vec<Router>,
    event_bus: Option<EventBus>,
}

impl ServerBuilder {
    /// Create a new ServerBuilder
    pub fn new() -> Self {
        Self {
            link_service: None,
            entity_registry: EntityRegistry::new(),
            configs: Vec::new(),
            modules: Vec::new(),
            custom_routes: Vec::new(),
            event_bus: None,
        }
    }

    /// Set the link service (required)
    pub fn with_link_service(mut self, service: impl LinkService + 'static) -> Self {
        self.link_service = Some(Arc::new(service));
        self
    }

    /// Add custom routes to the server
    ///
    /// Use this to add routes that don't fit the CRUD pattern, such as:
    /// - Authentication endpoints (/login, /logout)
    /// - OAuth flows (/oauth/token, /oauth/callback)
    /// - Webhooks (/webhooks/stripe)
    /// - Custom business logic endpoints
    ///
    /// # Example
    ///
    /// ```ignore
    /// use axum::{Router, routing::{post, get}, Json};
    /// use serde_json::json;
    ///
    /// let auth_routes = Router::new()
    ///     .route("/login", post(login_handler))
    ///     .route("/logout", post(logout_handler))
    ///     .route("/oauth/token", post(oauth_token_handler));
    ///
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .with_custom_routes(auth_routes)
    ///     .register_module(module)?
    ///     .build()?;
    /// ```
    pub fn with_custom_routes(mut self, routes: Router) -> Self {
        self.custom_routes.push(routes);
        self
    }

    /// Enable the event bus for real-time notifications
    ///
    /// When enabled, REST/GraphQL handlers will publish events for mutations,
    /// and real-time exposures (WebSocket, SSE) can subscribe to receive them.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Buffer size for the broadcast channel (recommended: 1024)
    ///
    /// # Example
    ///
    /// ```ignore
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .with_event_bus(1024)
    ///     .register_module(module)?
    ///     .build_host()?;
    /// ```
    pub fn with_event_bus(mut self, capacity: usize) -> Self {
        self.event_bus = Some(EventBus::new(capacity));
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

    /// Build the transport-agnostic host
    ///
    /// This generates a `ServerHost` that can be used with any exposure type
    /// (REST, GraphQL, gRPC, etc.).
    ///
    /// # Returns
    ///
    /// Returns a `ServerHost` containing all framework state.
    pub fn build_host(mut self) -> Result<ServerHost> {
        // Merge all configs
        let merged_config = self.merge_configs()?;

        // Extract link service
        let link_service = self
            .link_service
            .take()
            .ok_or_else(|| anyhow::anyhow!("LinkService is required. Call .with_link_service()"))?;

        // Build entity fetchers map from all modules
        let mut fetchers_map: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        for module in &self.modules {
            for entity_type in module.entity_types() {
                if let Some(fetcher) = module.get_entity_fetcher(entity_type) {
                    fetchers_map.insert(entity_type.to_string(), fetcher);
                }
            }
        }

        // Build entity creators map from all modules
        let mut creators_map: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        for module in &self.modules {
            for entity_type in module.entity_types() {
                if let Some(creator) = module.get_entity_creator(entity_type) {
                    creators_map.insert(entity_type.to_string(), creator);
                }
            }
        }

        // Build the host
        let mut host = ServerHost::from_builder_components(
            link_service,
            merged_config,
            self.entity_registry,
            fetchers_map,
            creators_map,
        )?;

        // Attach event bus if configured
        if let Some(event_bus) = self.event_bus.take() {
            host = host.with_event_bus(event_bus);
        }

        Ok(host)
    }

    /// Build the final REST router
    ///
    /// This generates:
    /// - CRUD routes for all registered entities
    /// - Link routes (bidirectional)
    /// - Introspection routes
    ///
    /// Note: This is a convenience method that builds the host and immediately
    /// exposes it via REST. For other exposure types, use `build_host_arc()`.
    pub fn build(mut self) -> Result<Router> {
        let custom_routes = std::mem::take(&mut self.custom_routes);
        let host = Arc::new(self.build_host()?);
        RestExposure::build_router(host, custom_routes)
    }

    /// Merge all configurations from registered modules
    fn merge_configs(&self) -> Result<LinksConfig> {
        Ok(LinksConfig::merge(self.configs.clone()))
    }

    /// Build a combined REST + gRPC router
    ///
    /// This is a convenience method that builds both REST and gRPC routers
    /// from the registered modules and merges them safely into a single router.
    ///
    /// Internally, it uses [`GrpcExposure::build_router_no_fallback`](super::GrpcExposure::build_router_no_fallback) for the
    /// gRPC side (no fallback) and [`RestExposure::build_router`] for REST
    /// (with its nested link path fallback), then merges them via
    /// [`combine_rest_and_grpc`](super::router::combine_rest_and_grpc).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let app = ServerBuilder::new()
    ///     .with_link_service(InMemoryLinkService::new())
    ///     .register_module(MyModule)?
    ///     .build_with_grpc()?;
    ///
    /// let listener = TcpListener::bind("127.0.0.1:3000").await?;
    /// axum::serve(listener, app).await?;
    /// ```
    #[cfg(feature = "grpc")]
    pub fn build_with_grpc(mut self) -> Result<Router> {
        use super::exposure::grpc::GrpcExposure;
        use super::router::combine_rest_and_grpc;

        let custom_routes = std::mem::take(&mut self.custom_routes);
        let host = Arc::new(self.build_host()?);

        let rest_router = RestExposure::build_router(host.clone(), custom_routes)?;
        let grpc_router = GrpcExposure::build_router_no_fallback(host)?;

        Ok(combine_rest_and_grpc(rest_router, grpc_router))
    }

    /// Serve the application with graceful shutdown
    ///
    /// This will:
    /// - Bind to the provided address
    /// - Start serving requests
    /// - Handle SIGTERM and SIGINT (Ctrl+C) for graceful shutdown
    ///
    /// # Example
    ///
    /// ```ignore
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .register_module(module)?
    ///     .serve("127.0.0.1:3000").await?;
    /// ```
    pub async fn serve(self, addr: &str) -> Result<()> {
        let app = self.build()?;
        let listener = TcpListener::bind(addr).await?;

        tracing::info!("Server listening on {}", addr);

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        tracing::info!("Server shutdown complete");
        Ok(())
    }

    /// Serve the application with REST + gRPC and graceful shutdown
    ///
    /// This is the gRPC equivalent of [`serve`](Self::serve). It builds a combined
    /// REST+gRPC router and serves it on the given address.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .register_module(module)?
    ///     .serve_with_grpc("127.0.0.1:3000").await?;
    /// ```
    #[cfg(feature = "grpc")]
    pub async fn serve_with_grpc(self, addr: &str) -> Result<()> {
        let app = self.build_with_grpc()?;
        let listener = TcpListener::bind(addr).await?;

        tracing::info!("Server listening on {} (REST + gRPC)", addr);

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        tracing::info!("Server shutdown complete");
        Ok(())
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::LinkDefinition;
    use crate::core::module::Module;
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use std::sync::Arc;

    // ── Mock Module for testing ──────────────────────────────────────────

    /// A minimal Module implementation for builder tests
    struct StubModule {
        name: &'static str,
        entity_types: Vec<&'static str>,
        config: LinksConfig,
    }

    impl StubModule {
        fn single_entity() -> Self {
            Self {
                name: "stub",
                entity_types: vec!["order"],
                config: LinksConfig {
                    entities: vec![EntityConfig {
                        singular: "order".to_string(),
                        plural: "orders".to_string(),
                        auth: EntityAuthConfig::default(),
                    }],
                    links: vec![],
                    validation_rules: None,
                },
            }
        }

        fn with_link() -> Self {
            Self {
                name: "linked_stub",
                entity_types: vec!["user", "car"],
                config: LinksConfig {
                    entities: vec![
                        EntityConfig {
                            singular: "user".to_string(),
                            plural: "users".to_string(),
                            auth: EntityAuthConfig::default(),
                        },
                        EntityConfig {
                            singular: "car".to_string(),
                            plural: "cars".to_string(),
                            auth: EntityAuthConfig::default(),
                        },
                    ],
                    links: vec![LinkDefinition {
                        link_type: "owner".to_string(),
                        source_type: "user".to_string(),
                        target_type: "car".to_string(),
                        forward_route_name: "cars-owned".to_string(),
                        reverse_route_name: "users-owners".to_string(),
                        description: Some("User owns a car".to_string()),
                        required_fields: None,
                        auth: None,
                    }],
                    validation_rules: None,
                },
            }
        }
    }

    impl Module for StubModule {
        fn name(&self) -> &str {
            self.name
        }

        fn entity_types(&self) -> Vec<&str> {
            self.entity_types.clone()
        }

        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            Ok(self.config.clone())
        }

        fn register_entities(&self, _registry: &mut EntityRegistry) {
            // No entity descriptors in stub
        }

        fn get_entity_fetcher(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityFetcher>> {
            None
        }

        fn get_entity_creator(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityCreator>> {
            None
        }
    }

    /// A module whose links_config() returns an error
    struct FailingModule;

    impl Module for FailingModule {
        fn name(&self) -> &str {
            "failing"
        }

        fn entity_types(&self) -> Vec<&str> {
            vec![]
        }

        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            Err(anyhow::anyhow!("config load failed"))
        }

        fn register_entities(&self, _registry: &mut EntityRegistry) {}

        fn get_entity_fetcher(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityFetcher>> {
            None
        }

        fn get_entity_creator(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityCreator>> {
            None
        }
    }

    // ── Constructor tests ────────────────────────────────────────────────

    #[test]
    fn test_new_creates_empty_builder() {
        let builder = ServerBuilder::new();
        assert!(builder.link_service.is_none());
        assert!(builder.configs.is_empty());
        assert!(builder.modules.is_empty());
        assert!(builder.custom_routes.is_empty());
        assert!(builder.event_bus.is_none());
    }

    #[test]
    fn test_default_is_same_as_new() {
        let builder = ServerBuilder::default();
        assert!(builder.link_service.is_none());
        assert!(builder.configs.is_empty());
        assert!(builder.modules.is_empty());
        assert!(builder.custom_routes.is_empty());
        assert!(builder.event_bus.is_none());
    }

    // ── with_link_service ────────────────────────────────────────────────

    #[test]
    fn test_with_link_service_sets_service() {
        let builder = ServerBuilder::new().with_link_service(InMemoryLinkService::new());
        assert!(builder.link_service.is_some());
    }

    // ── with_event_bus ───────────────────────────────────────────────────

    #[test]
    fn test_with_event_bus_sets_bus() {
        let builder = ServerBuilder::new().with_event_bus(1024);
        assert!(builder.event_bus.is_some());
    }

    // ── with_custom_routes ───────────────────────────────────────────────

    #[test]
    fn test_with_custom_routes_appends_router() {
        let builder = ServerBuilder::new()
            .with_custom_routes(Router::new())
            .with_custom_routes(Router::new());
        assert_eq!(builder.custom_routes.len(), 2);
    }

    // ── register_module ──────────────────────────────────────────────────

    #[test]
    fn test_register_module_stores_config_and_module() {
        let builder = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("register_module should succeed");
        assert_eq!(builder.configs.len(), 1);
        assert_eq!(builder.modules.len(), 1);
    }

    #[test]
    fn test_register_multiple_modules() {
        let builder = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("first module should register")
            .register_module(StubModule::with_link())
            .expect("second module should register");
        assert_eq!(builder.configs.len(), 2);
        assert_eq!(builder.modules.len(), 2);
    }

    #[test]
    fn test_register_module_failing_config_returns_error() {
        let result = ServerBuilder::new().register_module(FailingModule);
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().expect("should be Err"));
        assert!(
            err_msg.contains("config load failed"),
            "error should contain cause: {}",
            err_msg
        );
    }

    // ── build_host ───────────────────────────────────────────────────────

    #[test]
    fn test_build_host_without_link_service_fails() {
        let result = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host();
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().expect("should be Err"));
        assert!(
            err_msg.contains("LinkService is required"),
            "error should mention LinkService: {}",
            err_msg
        );
    }

    #[test]
    fn test_build_host_single_module() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        assert_eq!(host.config.entities.len(), 1);
        assert_eq!(host.config.entities[0].singular, "order");
        assert!(host.event_bus.is_none());
    }

    #[test]
    fn test_build_host_multi_module_merges_configs() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(StubModule::single_entity())
            .expect("register first should succeed")
            .register_module(StubModule::with_link())
            .expect("register second should succeed")
            .build_host()
            .expect("build_host should succeed");

        // Merged config should contain entities from both modules
        let entity_names: Vec<&str> = host
            .config
            .entities
            .iter()
            .map(|e| e.singular.as_str())
            .collect();
        assert!(entity_names.contains(&"order"), "should contain order");
        assert!(entity_names.contains(&"user"), "should contain user");
        assert!(entity_names.contains(&"car"), "should contain car");
    }

    #[test]
    fn test_build_host_with_event_bus_attaches_bus() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_event_bus(16)
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        assert!(host.event_bus().is_some());
    }

    #[test]
    fn test_build_host_no_modules_empty_config() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .build_host()
            .expect("build_host with no modules should succeed");

        assert!(host.config.entities.is_empty());
        assert!(host.config.links.is_empty());
    }

    // ── build (REST router) ──────────────────────────────────────────────

    #[test]
    fn test_build_produces_router() {
        let router = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build()
            .expect("build should produce a Router");

        // We cannot inspect the Router deeply, but it should not panic
        let _ = router;
    }

    #[test]
    fn test_build_without_link_service_fails() {
        let result = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_with_custom_routes() {
        use axum::routing::get;

        let custom = Router::new().route("/custom", get(|| async { "ok" }));
        let router = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_custom_routes(custom)
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build()
            .expect("build should succeed with custom routes");

        let _ = router;
    }

    // ── Fluent chaining ──────────────────────────────────────────────────

    #[test]
    fn test_fluent_chaining_full_pipeline() {
        let result = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_event_bus(256)
            .with_custom_routes(Router::new())
            .register_module(StubModule::with_link())
            .expect("register should succeed")
            .build();
        assert!(result.is_ok(), "full fluent pipeline should succeed");
    }
}

/// Wait for shutdown signal (SIGTERM or Ctrl+C)
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C signal, initiating graceful shutdown...");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM signal, initiating graceful shutdown...");
        },
    }
}
