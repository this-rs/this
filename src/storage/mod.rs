//! Storage implementations for different backends

#[cfg(feature = "dynamodb")]
pub mod dynamodb;
pub mod in_memory;
#[cfg(feature = "lmdb")]
pub mod lmdb;
#[cfg(feature = "mongodb_backend")]
pub mod mongodb;
#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "neo4j")]
pub mod neo4j;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "scylladb")]
pub mod scylladb;

#[cfg(feature = "lmdb")]
pub use self::lmdb::{LmdbDataService, LmdbLinkService};
#[cfg(feature = "mongodb_backend")]
pub use self::mongodb::{MongoDataService, MongoLinkService};
#[cfg(feature = "mysql")]
pub use self::mysql::{MysqlDataService, MysqlLinkService};
#[cfg(feature = "neo4j")]
pub use self::neo4j::{Neo4jDataService, Neo4jLinkService};
#[cfg(feature = "dynamodb")]
pub use dynamodb::{DynamoDBDataService, DynamoDBLinkService};
pub use in_memory::{InMemoryDataService, InMemoryLinkService};
#[cfg(feature = "lmdb")]
pub use self::lmdb::{LmdbDataService, LmdbLinkService};
#[cfg(feature = "mongodb_backend")]
pub use self::mongodb::{MongoDataService, MongoLinkService};
#[cfg(feature = "mysql")]
pub use self::mysql::{MysqlDataService, MysqlLinkService};
#[cfg(feature = "neo4j")]
pub use self::neo4j::{Neo4jDataService, Neo4jLinkService};
#[cfg(feature = "postgres")]
pub use postgres::{PostgresDataService, PostgresLinkService};
#[cfg(feature = "scylladb")]
pub use scylladb::{ScyllaDataService, ScyllaLinkService};
