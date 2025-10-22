//! Router builder utilities for link routes

use crate::links::handlers::{
    create_link, delete_link, get_link, get_link_by_route, list_available_links, list_links,
    update_link, AppState,
};
use axum::{routing::get, Router};

/// Build link routes from configuration
///
/// These routes are generic and work for all entities using semantic route_names:
/// - GET /links/{link_id} - Get a specific link by ID
/// - GET /{entity_type}/{entity_id}/{route_name} - List links (e.g., /users/123/cars-owned)
/// - GET /{source_type}/{source_id}/{route_name}/{target_id} - Get a specific link (e.g., /users/123/cars-owned/456)
/// - POST /{source_type}/{source_id}/{route_name}/{target_id} - Create link (e.g., /users/123/cars-owned/456)
/// - PUT /{source_type}/{source_id}/{route_name}/{target_id} - Update link metadata
/// - DELETE /{source_type}/{source_id}/{route_name}/{target_id} - Delete link
/// - GET /{entity_type}/{entity_id}/links - List available link types
///
/// The route_name (e.g., "cars-owned", "cars-driven") is resolved to the appropriate
/// link_type (e.g., "owner", "driver") automatically by the LinkRouteRegistry.
pub fn build_link_routes(state: AppState) -> Router {
    Router::new()
        .route("/links/{link_id}", get(get_link))
        .route("/{entity_type}/{entity_id}/{route_name}", get(list_links))
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
        .with_state(state)
}
