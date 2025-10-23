//! Storage implementations for different backends

#[cfg(feature = "dynamodb")]
pub mod dynamodb;
pub mod in_memory;

#[cfg(feature = "dynamodb")]
pub use dynamodb::{DynamoDBDataService, DynamoDBLinkService};
pub use in_memory::InMemoryLinkService;
