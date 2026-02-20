//! Integration tests for MongoDB storage backends using the storage test harness.
//!
//! Invokes `data_service_tests!`, `link_service_tests!`, and `rest_integration_tests!`
//! to validate that MongoDB storage backends fully conform to their contracts.
//!
//! # Requirements
//!
//! - Docker must be running (testcontainers launches a MongoDB container)
//! - Feature flag `mongodb_backend` must be enabled
//!
//! # Running
//!
//! ```sh
//! cargo test --features mongodb_backend --test mongodb_tests -- --test-threads=1
//! ```
//!
//! # Test isolation
//!
//! All tests share a single MongoDB container (via `OnceLock`). Each test
//! drops the entity and links collections before running for full isolation.

#![cfg(feature = "mongodb_backend")]

#[macro_use]
mod storage_harness;

use mongodb::Client;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use storage_harness::*;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mongo::Mongo;
use this::storage::{MongoDataService, MongoLinkService};

// ---------------------------------------------------------------------------
// Shared test environment (single container, fresh client per test)
// ---------------------------------------------------------------------------

/// Holds the testcontainer handle (keeps it alive) and the connection URL.
struct MongoTestEnv {
    /// Container handle — dropping this stops the MongoDB container.
    _container: testcontainers::ContainerAsync<Mongo>,
    /// Connection URL for creating per-test clients.
    connection_url: String,
}

/// Global test environment, initialized once per test binary.
static TEST_ENV: OnceLock<MongoTestEnv> = OnceLock::new();

/// Initialize the shared MongoDB container (if not already started).
async fn init_mongo_env() -> &'static MongoTestEnv {
    if let Some(env) = TEST_ENV.get() {
        return env;
    }

    let container = Mongo::default()
        .start()
        .await
        .expect("Failed to start MongoDB container — is Docker running?");

    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(27017).await.unwrap();
    let url = format!("mongodb://{}:{}", host, port);

    let env = MongoTestEnv {
        _container: container,
        connection_url: url,
    };

    let _ = TEST_ENV.set(env);
    TEST_ENV.get().unwrap()
}

/// Atomic counter to generate unique database names per test.
static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a fresh MongoDB client with a unique database for test isolation.
///
/// Each call returns a **different** database so tests can safely run in
/// parallel without interfering with each other.
async fn mongo_database() -> mongodb::Database {
    let env = init_mongo_env().await;
    let client = Client::with_uri_str(&env.connection_url)
        .await
        .expect("Failed to connect to MongoDB");
    let db_num = DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    client.database(&format!("this_test_{}", db_num))
}

// ---------------------------------------------------------------------------
// Factory helpers (drop collections before each test for isolation)
// ---------------------------------------------------------------------------

/// Create a fresh `MongoDataService` with a clean collection.
async fn clean_mongo_data_service() -> MongoDataService<TestDataEntity> {
    let db = mongo_database().await;
    // Drop the entity collection for test isolation
    db.collection::<mongodb::bson::Document>("test_data_entities")
        .drop()
        .await
        .expect("Failed to drop test_data_entities collection");
    MongoDataService::new(db)
}

/// Create a fresh `MongoLinkService` with a clean links collection.
async fn clean_mongo_link_service() -> MongoLinkService {
    let db = mongo_database().await;
    db.collection::<mongodb::bson::Document>("links")
        .drop()
        .await
        .expect("Failed to drop links collection");
    MongoLinkService::new(db)
}

// ---------------------------------------------------------------------------
// Test suites via macros
// ---------------------------------------------------------------------------

data_service_tests!(clean_mongo_data_service().await);
link_service_tests!(clean_mongo_link_service().await);
rest_integration_tests!(clean_mongo_data_service().await);
