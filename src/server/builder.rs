//! ServerBuilder for fluent API to build HTTP servers

use super::entity_registry::EntityRegistry;
use super::router::build_link_routes;
use crate::config::LinksConfig;
use crate::core::module::Module;
use crate::core::service::LinkService;
use crate::core::{EntityCreator, EntityFetcher};
use crate::links::handlers::AppState;
use crate::links::registry::LinkRouteRegistry;
use anyhow::Result;
use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
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

        // Build entity creators map from all modules
        let mut creators_map: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        for module in &self.modules {
            for entity_type in module.entity_types() {
                if let Some(creator) = module.get_entity_creator(entity_type) {
                    creators_map.insert(entity_type.to_string(), creator);
                }
            }
        }

        // Create link app state
        let link_state = AppState {
            link_service,
            config,
            registry,
            entity_fetchers: Arc::new(fetchers_map),
            entity_creators: Arc::new(creators_map),
        };

        // Add health check routes FIRST (before fallback catches them)
        let health_routes = Router::new()
            .route("/health", get(health_check))
            .route("/healthz", get(health_check));

        // Build entity routes
        let entity_routes = self.entity_registry.build_routes();

        // Build standard link routes (2 levels)
        let link_routes = build_link_routes(link_state.clone());

        // Merge custom routes (before link routes to avoid fallback override)
        let mut app = health_routes.merge(entity_routes);

        for custom_router in self.custom_routes {
            app = app.merge(custom_router);
        }

        // Add link routes last (they have a fallback that catches everything)
        app = app.merge(link_routes);

        Ok(app)
    }

    /// Merge all configurations from registered modules
    fn merge_configs(&self) -> Result<LinksConfig> {
        Ok(LinksConfig::merge(self.configs.clone()))
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
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Health check endpoint handler
async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "this-rs"
    }))
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
