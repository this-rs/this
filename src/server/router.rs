//! Router builder utilities for link routes

use crate::links::handlers::{
    create_link, delete_link, list_available_links, list_links, AppState,
};
use axum::{routing::get, Router};

/// Build link routes from configuration
///
/// These routes are generic and work for all entities:
/// - GET /:entity_type/:entity_id/:route_name - List links
/// - POST /:source_type/:source_id/:link_type/:target_type/:target_id - Create link
/// - DELETE /:source_type/:source_id/:link_type/:target_type/:target_id - Delete link
/// - GET /:entity_type/:entity_id/links - List available link types
pub fn build_link_routes(state: AppState) -> Router {
    Router::new()
        .route("/:entity_type/:entity_id/:route_name", get(list_links))
        .route(
            "/:source_type/:source_id/:link_type/:target_type/:target_id",
            axum::routing::post(create_link).delete(delete_link),
        )
        .route("/:entity_type/:entity_id/links", get(list_available_links))
        .with_state(state)
}
