//! Integration tests for InMemoryDataService using the storage test harness.
//!
//! This file invokes `data_service_tests!` to validate that InMemoryDataService
//! fully conforms to the DataService<T> contract.

#[macro_use]
mod storage_harness;

use storage_harness::*;
use this::storage::InMemoryDataService;

data_service_tests!(InMemoryDataService::<TestDataEntity>::new());
