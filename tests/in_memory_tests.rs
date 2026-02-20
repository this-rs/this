//! Integration tests for InMemory storage backends using the storage test harness.
//!
//! Invokes `data_service_tests!`, `link_service_tests!`, and `rest_integration_tests!`
//! to validate that InMemory storage backends fully conform to their contracts.

#[macro_use]
mod storage_harness;

use storage_harness::*;
use this::storage::{InMemoryDataService, InMemoryLinkService};

data_service_tests!(InMemoryDataService::<TestDataEntity>::new());
link_service_tests!(InMemoryLinkService::new());
rest_integration_tests!(InMemoryDataService::<TestDataEntity>::new());
