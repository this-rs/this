//! Integration tests for InMemory storage backends using the storage test harness.
//!
//! Invokes `data_service_tests!` and `link_service_tests!` to validate that
//! InMemoryDataService and InMemoryLinkService fully conform to their contracts.

#[macro_use]
mod storage_harness;

use storage_harness::*;
use this::storage::{InMemoryDataService, InMemoryLinkService};

data_service_tests!(InMemoryDataService::<TestDataEntity>::new());
link_service_tests!(InMemoryLinkService::new());
