//! Integration tests for PostgreSQL storage backends using the storage test harness.
//!
//! Invokes `data_service_tests!`, `link_service_tests!`, and `rest_integration_tests!`
//! to validate that PostgreSQL storage backends fully conform to their contracts.
//!
//! # Requirements
//!
//! - Docker must be running (testcontainers launches a PostgreSQL container)
//! - Feature flag `postgres` must be enabled
//!
//! # Running
//!
//! ```sh
//! cargo test --features postgres --test postgres_tests -- --test-threads=1
//! ```
//!
//! # Test isolation
//!
//! All tests share a single PostgreSQL container (via `OnceLock`). Each test
//! creates a fresh `PgPool` and truncates tables before running.
//! The `--test-threads=1` flag ensures sequential execution for database safety.

#![cfg(feature = "postgres")]

#[macro_use]
mod storage_harness;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::OnceLock;
use storage_harness::*;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use this::storage::{PostgresDataService, PostgresLinkService};

// ---------------------------------------------------------------------------
// Shared test environment (single container, fresh pool per test)
// ---------------------------------------------------------------------------

/// Holds the testcontainer handle (keeps it alive) and the connection URL.
///
/// The container is stored in a process-global `OnceLock` (not tokio-aware)
/// so it survives across `#[tokio::test]` runtime boundaries.
/// Each test creates its own `PgPool` from the URL to avoid
/// pool-timeout issues caused by tokio runtime recycling.
struct PgTestEnv {
    /// Container handle — dropping this stops the PostgreSQL container.
    /// Stored in a static, so it lives for the entire test binary.
    _container: testcontainers::ContainerAsync<Postgres>,
    /// Connection URL for creating per-test pools.
    connection_url: String,
}

/// Global test environment, initialized once per test binary.
/// Uses `OnceLock` (std, not tokio) because the container must outlive
/// individual tokio runtimes created by `#[tokio::test]`.
static TEST_ENV: OnceLock<PgTestEnv> = OnceLock::new();

/// Initialize the shared PostgreSQL container (if not already started).
///
/// Must be called within a tokio runtime. Uses `OnceLock::get_or_init`
/// with a blocking-compatible pattern.
async fn init_pg_env() -> &'static PgTestEnv {
    if let Some(env) = TEST_ENV.get() {
        return env;
    }

    // First test to reach here starts the container
    let container = Postgres::default()
        .start()
        .await
        .expect("Failed to start PostgreSQL container — is Docker running?");

    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@{}:{}/postgres", host, port);

    // Run migrations with a temporary pool
    let pool = PgPool::connect(&url)
        .await
        .expect("Failed to connect to PostgreSQL");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    // Close the setup pool before caching (its runtime will die after this test)
    pool.close().await;

    let env = PgTestEnv {
        _container: container,
        connection_url: url,
    };

    // Race-safe: if another test initialized concurrently, that's fine
    // (won't happen with --test-threads=1, but defensive anyway)
    let _ = TEST_ENV.set(env);
    TEST_ENV.get().unwrap()
}

/// Create a fresh `PgPool` connected to the shared container.
///
/// Each call creates a NEW pool bound to the CURRENT tokio runtime,
/// avoiding pool-timeout issues from runtime recycling.
/// Pool is configured with limited connections (2 max) since tests
/// run sequentially with `--test-threads=1`.
async fn pg_pool() -> PgPool {
    let env = init_pg_env().await;
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&env.connection_url)
        .await
        .expect("Failed to connect to PostgreSQL")
}

// ---------------------------------------------------------------------------
// Factory helpers (truncate before each test for isolation)
// ---------------------------------------------------------------------------

/// Create a fresh `PostgresDataService` with a clean entities table.
async fn clean_pg_data_service() -> PostgresDataService<TestDataEntity> {
    let pool = pg_pool().await;
    sqlx::query("TRUNCATE entities CASCADE")
        .execute(&pool)
        .await
        .expect("Failed to truncate entities table");
    PostgresDataService::new(pool)
}

/// Create a fresh `PostgresLinkService` with a clean links table.
async fn clean_pg_link_service() -> PostgresLinkService {
    let pool = pg_pool().await;
    sqlx::query("TRUNCATE links CASCADE")
        .execute(&pool)
        .await
        .expect("Failed to truncate links table");
    PostgresLinkService::new(pool)
}

// ---------------------------------------------------------------------------
// Test suites via macros
// ---------------------------------------------------------------------------

data_service_tests!(clean_pg_data_service().await);
link_service_tests!(clean_pg_link_service().await);
rest_integration_tests!(clean_pg_data_service().await);
