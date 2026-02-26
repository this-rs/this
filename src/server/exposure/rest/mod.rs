//! REST API exposure for the framework
//!
//! This module provides REST-specific routing and handlers.
//! It is isolated from the core framework logic and can be replaced
//! or extended with other protocols (GraphQL, gRPC, etc.)
//!
//! The REST exposure consumes a `ServerHost` and produces an Axum `Router`.

use super::super::host::ServerHost;
use crate::links::handlers::AppState;
use crate::server::router::build_link_routes;
use anyhow::Result;
use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use std::sync::Arc;

/// REST API exposure implementation
///
/// This struct encapsulates all REST-specific logic for exposing the API.
/// It is completely separate from the framework core and can be replaced
/// with other exposure types (GraphQL, gRPC, etc.).
pub struct RestExposure;

impl RestExposure {
    /// Build the REST router from a host
    ///
    /// This method takes a `ServerHost` (which is transport-agnostic) and
    /// builds an Axum router with all REST endpoints.
    ///
    /// # Arguments
    ///
    /// * `host` - The server host containing all framework state
    /// * `custom_routes` - Additional custom routes to merge
    ///
    /// # Returns
    ///
    /// Returns a fully configured Axum router with:
    /// - Health check routes
    /// - Entity CRUD routes
    /// - Link routes
    /// - Custom routes
    pub fn build_router(host: Arc<ServerHost>, custom_routes: Vec<Router>) -> Result<Router> {
        // Create link app state from host
        let link_state = AppState {
            link_service: host.link_service.clone(),
            config: host.config.clone(),
            registry: host.registry.clone(),
            entity_fetchers: host.entity_fetchers.clone(),
            entity_creators: host.entity_creators.clone(),
            event_bus: host.event_bus.clone(),
        };

        // Build all routes
        let health_routes = Self::health_routes();
        let entity_routes = host.entity_registry.build_routes();
        let link_routes = build_link_routes(link_state.clone());

        // Merge everything
        let mut app = health_routes.merge(entity_routes);

        for custom_router in custom_routes {
            app = app.merge(custom_router);
        }

        app = app.merge(link_routes);

        Ok(app)
    }

    /// Build health check routes
    fn health_routes() -> Router {
        Router::new()
            .route("/health", get(Self::health_check))
            .route("/healthz", get(Self::health_check))
    }

    /// Health check endpoint handler
    async fn health_check() -> Json<Value> {
        Json(json!({
            "status": "ok",
            "service": "this-rs"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::server::entity_registry::EntityRegistry;
    use crate::server::host::ServerHost;
    use crate::storage::InMemoryLinkService;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use tower::ServiceExt;

    /// Build a minimal ServerHost for testing
    fn test_host() -> Arc<ServerHost> {
        let host = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .expect("should build host");
        Arc::new(host)
    }

    #[test]
    fn test_health_routes_builds_router() {
        let router = RestExposure::health_routes();
        let _ = router;
    }

    #[tokio::test]
    async fn test_health_endpoint_returns_ok() {
        let router = RestExposure::health_routes();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body should read");
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("body should be valid JSON");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "this-rs");
    }

    #[tokio::test]
    async fn test_healthz_endpoint_returns_ok() {
        let router = RestExposure::health_routes();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body should read");
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("body should be valid JSON");
        assert_eq!(json["status"], "ok");
    }

    #[test]
    fn test_build_router_succeeds_with_host() {
        let host = test_host();
        let router = RestExposure::build_router(host, vec![]);
        assert!(router.is_ok());
    }

    #[test]
    fn test_build_router_with_custom_routes() {
        use axum::routing::get;

        let host = test_host();
        let custom = Router::new().route("/custom", get(|| async { "custom" }));
        let router = RestExposure::build_router(host, vec![custom]);
        assert!(router.is_ok());
    }

    #[tokio::test]
    async fn test_build_router_health_endpoint_reachable() {
        let host = test_host();
        let router = RestExposure::build_router(host, vec![]).expect("build should succeed");
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
