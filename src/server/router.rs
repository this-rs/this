//! Router builder utilities for link routes

use crate::links::handlers::{
    create_link, delete_link, get_link, list_available_links, list_links, update_link, AppState,
};
use axum::{routing::get, Router};

/// Build link routes from configuration
///
/// These routes are generic and work for all entities:
/// - GET /links/{link_id} - Get a specific link by ID
/// - GET /{entity_type}/{entity_id}/{route_name} - List links
/// - POST /{source_type}/{source_id}/{link_type}/{target_type}/{target_id} - Create link
/// - PUT /{source_type}/{source_id}/{link_type}/{target_type}/{target_id} - Update link metadata
/// - DELETE /{source_type}/{source_id}/{link_type}/{target_type}/{target_id} - Delete link
/// - GET /{entity_type}/{entity_id}/links - List available link types
pub fn build_link_routes(state: AppState) -> Router {
    Router::new()
        .route("/links/{link_id}", get(get_link))
        .route("/{entity_type}/{entity_id}/{route_name}", get(list_links))
        .route(
            "/{source_type}/{source_id}/{link_type}/{target_type}/{target_id}",
            axum::routing::post(create_link)
                .put(update_link)
                .delete(delete_link),
        )
        .route(
            "/{entity_type}/{entity_id}/links",
            get(list_available_links),
        )
        .with_state(state)
}
