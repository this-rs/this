//! Integration tests for ScyllaDB storage backends using the storage test harness.
//!
//! # Requirements
//!
//! - Docker must be running (testcontainers launches a CQL-compatible container)
//! - Feature flag `scylladb` must be enabled
//!
//! # Running
//!
//! ```sh
//! cargo test --features scylladb --test scylladb_tests -- --test-threads=1
//! ```
//!
//! # Notes
//!
//! Uses a Cassandra container for testing because ScyllaDB requires kernel-level
//! AIO configuration that is unavailable in Docker Desktop (macOS/Windows).
//! The `scylla` Rust driver uses the CQL protocol, which is 100% compatible
//! with Cassandra. The implementation works identically on ScyllaDB in production.

#![cfg(feature = "scylladb")]

#[macro_use]
mod storage_harness;

use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use std::sync::{Arc, OnceLock};
use storage_harness::*;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use this::storage::{ScyllaDataService, ScyllaLinkService};

// ---------------------------------------------------------------------------
// Shared test environment
// ---------------------------------------------------------------------------

/// Holds the container handle and connection address.
/// The Session is NOT cached here — each test creates a fresh one
/// to avoid BrokenConnectionError across tokio runtime boundaries.
struct ScyllaTestEnv {
    _container: testcontainers::ContainerAsync<GenericImage>,
    node_addr: String,
}

const TEST_KEYSPACE: &str = "this_test";

static TEST_ENV: OnceLock<ScyllaTestEnv> = OnceLock::new();

async fn init_scylla_env() -> &'static ScyllaTestEnv {
    if let Some(env) = TEST_ENV.get() {
        return env;
    }

    // Use Cassandra (CQL-compatible) for testing.
    // ScyllaDB requires AIO slots that are unavailable in Docker Desktop.
    let container = GenericImage::new("cassandra", "4.1")
        .with_exposed_port(9042.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Startup complete"))
        .with_env_var("MAX_HEAP_SIZE", "256M")
        .with_env_var("HEAP_NEWSIZE", "50M")
        .with_startup_timeout(std::time::Duration::from_secs(120))
        .start()
        .await
        .expect("Failed to start Cassandra container — is Docker running?");

    let host = container.get_host().await.unwrap();
    let cql_port = container.get_host_port_ipv4(9042).await.unwrap();
    let node_addr = format!("{}:{}", host, cql_port);

    // Wait for CQL to become ready
    let mut session = None;
    for attempt in 0..90 {
        let connect = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            SessionBuilder::new().known_node(&node_addr).build(),
        )
        .await;

        if let Ok(Ok(s)) = connect {
            let ping = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                s.query_unpaged("SELECT now() FROM system.local", ()),
            )
            .await;
            if matches!(ping, Ok(Ok(_))) {
                session = Some(s);
                break;
            }
        }

        if attempt % 10 == 0 && attempt > 0 {
            eprintln!("CQL not ready yet after {} attempts, retrying...", attempt);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let s = session.expect("Failed to connect to CQL server after 90 retries");

    // Create schema with this temporary session
    this::storage::scylladb::ensure_schema(&s, TEST_KEYSPACE)
        .await
        .expect("Failed to create schema");

    let env = ScyllaTestEnv {
        _container: container,
        node_addr,
    };

    let _ = TEST_ENV.set(env);
    TEST_ENV.get().unwrap()
}

/// Create a fresh CQL Session for the current tokio runtime.
async fn cql_session() -> Arc<Session> {
    let env = init_scylla_env().await;
    let session = SessionBuilder::new()
        .known_node(&env.node_addr)
        .build()
        .await
        .expect("Failed to connect to CQL server");
    Arc::new(session)
}

// ---------------------------------------------------------------------------
// Factory helpers (truncate tables before each test for isolation)
// ---------------------------------------------------------------------------

async fn clean_scylla_data_service() -> ScyllaDataService<TestDataEntity> {
    let session = cql_session().await;
    session
        .query_unpaged(format!("TRUNCATE {}.entities", TEST_KEYSPACE), ())
        .await
        .expect("Failed to truncate entities table");
    ScyllaDataService::new(session, TEST_KEYSPACE)
}

async fn clean_scylla_link_service() -> ScyllaLinkService {
    let session = cql_session().await;
    session
        .query_unpaged(format!("TRUNCATE {}.links", TEST_KEYSPACE), ())
        .await
        .expect("Failed to truncate links table");
    ScyllaLinkService::new(session, TEST_KEYSPACE)
}

// ---------------------------------------------------------------------------
// Test suites via macros
// ---------------------------------------------------------------------------

data_service_tests!(clean_scylla_data_service().await);
link_service_tests!(clean_scylla_link_service().await);
rest_integration_tests!(clean_scylla_data_service().await);
