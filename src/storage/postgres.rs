//! PostgreSQL storage backend using sqlx.
//!
//! Provides `PostgresDataService<T>` and `PostgresLinkService` implementations
//! backed by a PostgreSQL database via `sqlx::PgPool`.
//!
//! # Feature flag
//!
//! This module is gated behind the `postgres` feature flag:
//! ```toml
//! [dependencies]
//! this-rs = { version = "0.0.7", features = ["postgres"] }
//! ```

use sqlx::PgPool;

/// Generic data storage service backed by PostgreSQL.
///
/// Stores entities in a shared `entities` table with common columns
/// (id, entity_type, name, status, timestamps) and a JSONB `data`
/// column for type-specific fields.
#[derive(Clone, Debug)]
pub struct PostgresDataService<T> {
    pool: PgPool,
    _marker: std::marker::PhantomData<T>,
}

impl<T> PostgresDataService<T> {
    /// Create a new `PostgresDataService` with the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            _marker: std::marker::PhantomData,
        }
    }
}

/// Link storage service backed by PostgreSQL.
///
/// Stores links in a `links` table with indexed columns for
/// efficient source/target traversal queries.
#[derive(Clone, Debug)]
pub struct PostgresLinkService {
    pool: PgPool,
}

impl PostgresLinkService {
    /// Create a new `PostgresLinkService` with the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
