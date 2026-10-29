//! Integration tests for Obrain storage backends using the storage test harness.
//!
//! Invokes `data_service_tests!`, `link_service_tests!`, and `rest_integration_tests!`
//! to validate that Obrain storage backends fully conform to their contracts.
//!
//! Uses an in-memory ObrainDB instance — no external database required.

#![cfg(feature = "obrain")]

#[macro_use]
mod storage_harness;

use storage_harness::*;
use this::storage::{ObrainDataService, ObrainLinkService, ObrainStore};

fn make_data_service() -> ObrainDataService<TestDataEntity> {
    let store = ObrainStore::new_in_memory();
    ObrainDataService::new(store)
}

fn make_link_service() -> ObrainLinkService {
    let store = ObrainStore::new_in_memory();
    ObrainLinkService::new(store)
}

data_service_tests!(make_data_service());
link_service_tests!(make_link_service());
rest_integration_tests!(make_data_service());
