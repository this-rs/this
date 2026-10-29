//! Server module for building HTTP servers with auto-registered routes
//!
//! This module provides a `ServerBuilder` that automatically registers:
//! - CRUD routes for all entities declared in modules
//! - Link routes for bidirectional entity relationships
//! - Introspection routes for API discovery
//!
//! The server architecture is modular and supports multiple exposure types:
//! - REST (implemented)
//! - GraphQL (available with 'graphql' feature)
//! - gRPC (available with 'grpc' feature)
//! - WebSocket (available with 'websocket' feature)
//! - OpenAPI (planned)

pub mod builder;
/// Live host catalog: one immutable snapshot published atomically (module descriptor RFC).
pub mod catalog;
pub mod entity_registry;
pub mod exposure;
pub mod host;
pub mod middleware;
/// Runtime module loading: `ModuleLoader`, `ModuleInterface`, `ModuleSource` (interfaces).
pub mod module_runtime;
pub mod router;

#[cfg(feature = "frontend")]
pub mod frontend;

#[cfg(feature = "frontend")]
pub use frontend::Frontend;

pub use builder::ServerBuilder;
pub use entity_registry::{EntityDescriptor, EntityRegistry};
pub use exposure::RestExposure;
pub use host::ServerHost;

#[cfg(feature = "graphql")]
pub use exposure::GraphQLExposure;

#[cfg(feature = "websocket")]
pub use exposure::WebSocketExposure;

#[cfg(feature = "grpc")]
pub use exposure::GrpcExposure;

#[cfg(feature = "grpc")]
pub use router::combine_rest_and_grpc;
