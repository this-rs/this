//! GraphQL executor module
//!
//! This module contains the custom GraphQL executor that executes queries
//! against the dynamically generated schema.
//!
//! The executor is split into several sub-modules for better maintainability:
//! - `core`: Main executor orchestration
//! - `query_executor`: Query resolution logic
//! - `mutation_executor`: Mutation resolution logic
//! - `link_mutations`: Link-specific mutations
//! - `field_resolver`: Field and relation resolution
//! - `utils`: Utility functions

#[cfg(feature = "graphql")]
mod core;
#[cfg(feature = "graphql")]
mod field_resolver;
#[cfg(feature = "graphql")]
mod link_mutations;
#[cfg(feature = "graphql")]
mod mutation_executor;
#[cfg(feature = "graphql")]
mod query_executor;
#[cfg(feature = "graphql")]
mod utils;

#[cfg(feature = "graphql")]
pub use core::GraphQLExecutor;
