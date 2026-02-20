//! Integration tests for LMDB storage backends using the storage test harness.
//!
//! # Requirements
//!
//! - Feature flag `lmdb` must be enabled
//! - No external services needed (LMDB is an embedded database)
//!
//! # Running
//!
//! ```sh
//! cargo test --features lmdb --test lmdb_tests -- --test-threads=1
//! ```
//!
//! # Notes
//!
//! Each test gets a fresh temporary directory via `tempfile::TempDir`.
//! The LMDB environment is opened within that directory so tests are
//! fully isolated. `--test-threads=1` is required because LMDB only
//! allows one write transaction at a time per environment, and the
//! shared environment pattern (for `DataService` and `LinkService`
//! using the same env) needs sequential test execution.

#![cfg(feature = "lmdb")]

#[macro_use]
mod storage_harness;

use storage_harness::*;
use tempfile::TempDir;
use this::storage::{LmdbDataService, LmdbLinkService};

// ---------------------------------------------------------------------------
// Factory helpers (fresh temp dir per test for isolation)
// ---------------------------------------------------------------------------

fn fresh_lmdb_data_service() -> LmdbDataService<TestDataEntity> {
    let dir = TempDir::new().expect("Failed to create temp dir");
    // Leak the TempDir so it lives for the duration of the test
    // (otherwise it would be dropped immediately, deleting the DB files)
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    LmdbDataService::open(&path).expect("Failed to open LMDB data service")
}

fn fresh_lmdb_link_service() -> LmdbLinkService {
    let dir = TempDir::new().expect("Failed to create temp dir");
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    LmdbLinkService::open(&path).expect("Failed to open LMDB link service")
}

// ---------------------------------------------------------------------------
// Test suites via macros
// ---------------------------------------------------------------------------

data_service_tests!(fresh_lmdb_data_service());
link_service_tests!(fresh_lmdb_link_service());
rest_integration_tests!(fresh_lmdb_data_service());
