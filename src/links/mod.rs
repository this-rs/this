//! Link management module
//!
//! This module provides link handlers and routing registry
//! that are completely agnostic to entity types.

pub mod handlers;
pub mod registry;

pub use handlers::{
    AppState, create_link, delete_link, handle_nested_path_get, handle_nested_path_post,
    list_available_links, list_links,
};
pub use registry::{LinkDirection, LinkRouteRegistry, RouteInfo};
