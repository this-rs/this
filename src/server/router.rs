//! Router builder utilities for link routes and protocol merging

use crate::core::query::QueryParams;
use crate::links::handlers::{
    AppState, create_link, create_linked_entity, delete_link, get_link, get_link_by_route,
    handle_nested_path_get, list_available_links, list_links, update_link,
};
use axum::{Router, extract::Query, routing::get};

/// Combine a REST router and a gRPC router into a single router.
///
/// This function safely merges the two routers by taking advantage of the fact
/// that the gRPC router was built with
/// [`GrpcExposure::build_router_no_fallback`](crate::server::exposure::grpc::GrpcExposure::build_router_no_fallback),
/// which does **not** install a fallback handler.
///
/// The REST router's fallback (used for deeply nested link paths) is preserved,
/// ensuring that both REST and gRPC routes work correctly on the same server.
///
/// # Arguments
///
/// * `rest_router` - The REST router (from [`RestExposure::build_router`](crate::server::exposure::RestExposure::build_router)),
///   which includes a fallback for nested link paths.
/// * `grpc_router` - The gRPC router (from [`GrpcExposure::build_router_no_fallback`](crate::server::exposure::grpc::GrpcExposure::build_router_no_fallback)),
///   which has **no** fallback.
///
/// # Example
///
/// ```rust,ignore
/// use this::server::exposure::{RestExposure, grpc::GrpcExposure};
/// use this::server::router::combine_rest_and_grpc;
///
/// let host = Arc::new(builder.build_host()?);
/// let rest_router = RestExposure::build_router(host.clone(), vec![])?;
/// let grpc_router = GrpcExposure::build_router_no_fallback(host)?;
/// let app = combine_rest_and_grpc(rest_router, grpc_router);
///
/// axum::serve(listener, app).await?;
/// ```
///
/// # Panics
///
/// Panics if `grpc_router` has a fallback installed (e.g., if built with
/// `GrpcExposure::build_router()` instead of `build_router_no_fallback()`).
/// Always use `build_router_no_fallback()` for the gRPC side.
#[cfg(feature = "grpc")]
pub fn combine_rest_and_grpc(rest_router: Router, grpc_router: Router) -> Router {
    rest_router.merge(grpc_router)
}

/// Build link routes from configuration
///
/// These routes are generic and work for all entities using semantic route_names:
/// - GET /links/{link_id} - Get a specific link by ID
/// - GET /{entity_type}/{entity_id}/{route_name} - List links (e.g., /users/123/cars-owned)
/// - POST /{entity_type}/{entity_id}/{route_name} - Create new entity + link (entity + metadata in body)
/// - GET /{source_type}/{source_id}/{route_name}/{target_id} - Get a specific link (e.g., /users/123/cars-owned/456)
/// - POST /{source_type}/{source_id}/{route_name}/{target_id} - Create link between existing entities
/// - PUT /{source_type}/{source_id}/{route_name}/{target_id} - Update link metadata
/// - DELETE /{source_type}/{source_id}/{route_name}/{target_id} - Delete link
/// - GET /{entity_type}/{entity_id}/links - List available link types
///
/// NOTE: Nested routes are supported up to 2 levels automatically:
/// - GET /{entity_type}/{entity_id}/{route_name} - List linked entities
/// - GET /{entity_type}/{entity_id}/{route_name}/{target_id} - Get specific link
///
/// For deeper nesting (3+ levels), see: docs/guides/CUSTOM_NESTED_ROUTES.md
///
/// The route_name (e.g., "cars-owned", "cars-driven") is resolved to the appropriate
/// link_type (e.g., "owner", "driver") automatically by the LinkRouteRegistry.
pub fn build_link_routes(state: AppState) -> Router {
    use axum::extract::{Path as AxumPath, Request, State as AxumState};
    use axum::response::IntoResponse;
    use uuid::Uuid;

    // Handler intelligent qui route vers list_links OU handle_nested_path_get selon la profondeur
    let smart_handler = |AxumState(state): AxumState<AppState>,
                         AxumPath((entity_type_plural, entity_id, route_name)): AxumPath<(
        String,
        Uuid,
        String,
    )>,
                         Query(params): Query<QueryParams>,
                         req: Request| async move {
        let path = req.uri().path();
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();

        // Si plus de 3 segments, c'est une route imbriquée à 3+ niveaux
        if segments.len() >= 5 {
            // Utiliser le handler générique pour chemins profonds (with pagination)
            handle_nested_path_get(AxumState(state), AxumPath(path.to_string()), Query(params))
                .await
                .map(|r| r.into_response())
        } else {
            // Route classique à 2 niveaux - with pagination
            list_links(
                AxumState(state),
                AxumPath((entity_type_plural, entity_id, route_name)),
                Query(params),
            )
            .await
            .map(|r| r.into_response())
        }
    };

    // Handler fallback pour les autres cas (with pagination)
    let fallback_handler = |AxumState(state): AxumState<AppState>,
                            Query(params): Query<QueryParams>,
                            req: Request| async move {
        let path = req.uri().path().to_string();
        handle_nested_path_get(AxumState(state), AxumPath(path), Query(params))
            .await
            .map(|r| r.into_response())
    };

    Router::new()
        .route("/links/{link_id}", get(get_link))
        .route(
            "/{entity_type}/{entity_id}/{route_name}",
            get(smart_handler).post(create_linked_entity),
        )
        .route(
            "/{source_type}/{source_id}/{route_name}/{target_id}",
            get(get_link_by_route)
                .post(create_link)
                .put(update_link)
                .delete(delete_link),
        )
        .route(
            "/{entity_type}/{entity_id}/links",
            get(list_available_links),
        )
        .fallback(fallback_handler)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::core::events::EventBus;
    use crate::links::handlers::AppState;
    use crate::links::registry::LinkRouteRegistry;
    use crate::storage::InMemoryLinkService;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Build a minimal AppState for testing
    fn test_app_state() -> AppState {
        let config = Arc::new(LinksConfig::default_config());
        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));
        AppState {
            link_service: Arc::new(InMemoryLinkService::new()),
            config,
            registry,
            entity_fetchers: Arc::new(HashMap::new()),
            entity_creators: Arc::new(HashMap::new()),
            event_bus: None,
        }
    }

    #[test]
    fn test_build_link_routes_produces_router() {
        let state = test_app_state();
        let router = build_link_routes(state);
        // Should not panic; router is valid
        let _ = router;
    }

    #[test]
    fn test_build_link_routes_with_event_bus() {
        let config = Arc::new(LinksConfig::default_config());
        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));
        let state = AppState {
            link_service: Arc::new(InMemoryLinkService::new()),
            config,
            registry,
            entity_fetchers: Arc::new(HashMap::new()),
            entity_creators: Arc::new(HashMap::new()),
            event_bus: Some(Arc::new(EventBus::new(16))),
        };
        let router = build_link_routes(state);
        let _ = router;
    }

    #[test]
    fn test_build_link_routes_empty_config() {
        let config = Arc::new(LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: None,
        });
        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));
        let state = AppState {
            link_service: Arc::new(InMemoryLinkService::new()),
            config,
            registry,
            entity_fetchers: Arc::new(HashMap::new()),
            entity_creators: Arc::new(HashMap::new()),
            event_bus: None,
        };
        let router = build_link_routes(state);
        let _ = router;
    }

    #[cfg(feature = "grpc")]
    mod grpc_tests {
        use super::super::combine_rest_and_grpc;
        use axum::Router;

        #[test]
        fn test_combine_rest_and_grpc_merges_routers() {
            let rest = Router::new();
            let grpc = Router::new();
            let combined = combine_rest_and_grpc(rest, grpc);
            let _ = combined;
        }
    }
}
