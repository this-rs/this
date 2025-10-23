//! Link management module
//!
//! This module provides link handlers and routing registry
//! that are completely agnostic to entity types.

pub mod handlers;
pub mod registry;

pub use handlers::{create_link, delete_link, list_available_links, list_links, AppState};
pub use registry::{LinkDirection, LinkRouteRegistry, RouteInfo};
