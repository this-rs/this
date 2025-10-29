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

// Future expositions
// pub mod grpc;
// pub mod openapi;
