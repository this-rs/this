//! API Exposure modules for different protocols
//!
//! This module provides implementations for exposing the API via different protocols.
//! Each exposure type consumes a `ServerHost` and produces a Router for that protocol.

pub mod rest;

#[cfg(feature = "graphql")]
pub mod graphql;

// Re-export for convenience
pub use rest::RestExposure;

#[cfg(feature = "graphql")]
pub use graphql::GraphQLExposure;

#[cfg(feature = "websocket")]
pub mod websocket;

#[cfg(feature = "websocket")]
pub use websocket::WebSocketExposure;

#[cfg(feature = "grpc")]
pub mod grpc;

#[cfg(feature = "grpc")]
pub use grpc::GrpcExposure;

// Future expositions
// pub mod openapi;
