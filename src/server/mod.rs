//! Server module for building HTTP servers with auto-registered routes
//!
//! This module provides a `ServerBuilder` that automatically registers:
//! - CRUD routes for all entities declared in modules
//! - Link routes for bidirectional entity relationships
//! - Introspection routes for API discovery

pub mod builder;
pub mod entity_registry;
pub mod router;

pub use builder::ServerBuilder;
pub use entity_registry::{EntityDescriptor, EntityRegistry};
