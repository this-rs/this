//! Storage implementations for different backends

#[cfg(feature = "dynamodb")]
pub mod dynamodb;
pub mod in_memory;
#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "dynamodb")]
pub use dynamodb::{DynamoDBDataService, DynamoDBLinkService};
pub use in_memory::{InMemoryDataService, InMemoryLinkService};
#[cfg(feature = "postgres")]
pub use postgres::{PostgresDataService, PostgresLinkService};
