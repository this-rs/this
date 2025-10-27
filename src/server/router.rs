//! Router builder utilities for link routes

use crate::links::handlers::{
    AppState, create_link, create_linked_entity, delete_link, get_link, get_link_by_route,
    list_available_links, list_links, update_link,
    handle_nested_path_get,
};
use axum::{Router, routing::get};

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
    use axum::extract::{Request, State as AxumState, Path as AxumPath};
    use axum::response::IntoResponse;
    use axum::http::Method;
    use uuid::Uuid;
    
    // Handler intelligent qui route vers list_links OU handle_nested_path_get selon la profondeur
    let state_clone = state.clone();
    let smart_handler = |AxumState(state): AxumState<AppState>, AxumPath((entity_type_plural, entity_id, route_name)): AxumPath<(String, Uuid, String)>, req: Request| async move {
        let path = req.uri().path();
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        
        // Si plus de 3 segments, c'est une route imbriquée à 3+ niveaux
        if segments.len() >= 5 {
            // Utiliser le handler générique pour chemins profonds
            handle_nested_path_get(
                AxumState(state),
                AxumPath(path.to_string())
            ).await
            .map(|r| r.into_response())
        } else {
            // Route classique à 2 niveaux
            list_links(
                AxumState(state),
                AxumPath((entity_type_plural, entity_id, route_name))
            ).await
            .map(|r| r.into_response())
        }
    };
    
    // Handler fallback pour les autres cas
    let fallback_state = state.clone();
    let fallback_handler = |AxumState(state): AxumState<AppState>, req: Request| async move {
        let path = req.uri().path().to_string();
        handle_nested_path_get(
            AxumState(state),
            AxumPath(path)
        ).await
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
