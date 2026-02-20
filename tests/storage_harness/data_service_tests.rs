//! Macro-generated test suite for `DataService<TestDataEntity>` contract validation.
//!
//! The `data_service_tests!` macro generates a comprehensive test module that
//! validates any `DataService<TestDataEntity>` implementation against the full
//! contract: CRUD operations, search across all `FieldValue` variants, edge cases,
//! and concurrent access.
//!
//! # Usage
//!
//! ```rust,ignore
//! #[macro_use]
//! mod storage_harness;
//!
//! use storage_harness::*;
//! use this::storage::InMemoryDataService;
//!
//! data_service_tests!(InMemoryDataService::<TestDataEntity>::new());
//! ```
//!
//! # Generated Tests (16+)
//!
//! ## CRUD
//! - `test_create_and_get` — create then retrieve, verify all fields
//! - `test_get_nonexistent` — get with random UUID returns None
//! - `test_list_empty` — list on empty store returns empty vec
//! - `test_list_multiple` — create 5 entities, list returns all 5
//! - `test_update_existing` — mutate name, verify persisted
//! - `test_update_nonexistent` — update unknown ID returns Err
//! - `test_delete_existing` — delete then get returns None
//! - `test_delete_nonexistent` — delete unknown ID (Ok or Err, both accepted)
//!
//! ## Search
//! - `test_search_string_field` — search by email (FieldValue::String)
//! - `test_search_integer_field` — search by age (FieldValue::Integer)
//! - `test_search_float_field` — search by score (FieldValue::Float)
//! - `test_search_boolean_field` — search by active (FieldValue::Boolean)
//! - `test_search_no_results` — search with non-matching value
//! - `test_search_unknown_field` — search on nonexistent field
//!
//! ## Edge Cases
//! - `test_create_duplicate_id` — insert twice with same UUID (overwrite or error)
//! - `test_concurrent_access` — parallel creates from spawned tasks

/// Generate a full `DataService<TestDataEntity>` conformance test suite.
///
/// `$factory` must be an expression that evaluates to an instance implementing
/// `DataService<TestDataEntity>`. It is re-evaluated for each test to ensure
/// isolation. For the concurrent access test, the returned service must also
/// implement `Clone + 'static` (shared state via Arc pattern).
#[macro_export]
macro_rules! data_service_tests {
    ($factory:expr) => {
        mod data_service_contract_tests {
            use super::*;
            use this::core::entity::{Data, Entity};
            use this::core::service::DataService;
            use uuid::Uuid;

            // ==================================================================
            // CRUD — Create & Get
            // ==================================================================

            #[tokio::test]
            async fn test_create_and_get() {
                let service = $factory;
                let entity = create_test_entity("Alice", "alice@test.com", 30, 4.5, true);
                let original_id = entity.id;

                let created = service.create(entity).await.unwrap();
                assert_eq!(created.id(), original_id);
                assert_eq!(created.name(), "Alice");
                assert_eq!(created.entity_type(), "test_data");
                assert_eq!(created.email, "alice@test.com");
                assert_eq!(created.age, 30);
                assert!((created.score - 4.5).abs() < f64::EPSILON);
                assert!(created.active);
                assert_eq!(created.status(), "active");

                // Retrieve and verify
                let retrieved = service.get(&original_id).await.unwrap();
                assert!(retrieved.is_some(), "Entity should exist after create");
                let retrieved = retrieved.unwrap();
                assert_eq!(retrieved.id(), original_id);
                assert_eq!(retrieved.name(), "Alice");
                assert_eq!(retrieved.email, "alice@test.com");
                assert_eq!(retrieved.age, 30);
                assert!((retrieved.score - 4.5).abs() < f64::EPSILON);
                assert!(retrieved.active);
            }

            // ==================================================================
            // CRUD — Get nonexistent
            // ==================================================================

            #[tokio::test]
            async fn test_get_nonexistent() {
                let service = $factory;
                let random_id = Uuid::new_v4();

                let result = service.get(&random_id).await.unwrap();
                assert!(
                    result.is_none(),
                    "Getting a nonexistent entity should return None"
                );
            }

            // ==================================================================
            // CRUD — List empty
            // ==================================================================

            #[tokio::test]
            async fn test_list_empty() {
                let service = $factory;

                let all = service.list().await.unwrap();
                assert!(
                    all.is_empty(),
                    "List on empty store should return empty vec"
                );
            }

            // ==================================================================
            // CRUD — List multiple
            // ==================================================================

            #[tokio::test]
            async fn test_list_multiple() {
                let service = $factory;
                let batch = sample_batch(5);
                let mut expected_ids: Vec<Uuid> = Vec::new();

                for entity in batch {
                    expected_ids.push(entity.id);
                    service.create(entity).await.unwrap();
                }

                let all = service.list().await.unwrap();
                assert_eq!(all.len(), 5, "List should return all 5 created entities");

                let returned_ids: Vec<Uuid> = all.iter().map(|e| e.id()).collect();
                for id in &expected_ids {
                    assert!(
                        returned_ids.contains(id),
                        "Listed entities should contain id {}",
                        id
                    );
                }
            }

            // ==================================================================
            // CRUD — Update existing
            // ==================================================================

            #[tokio::test]
            async fn test_update_existing() {
                let service = $factory;
                let mut entity = create_test_entity("Alice", "alice@test.com", 25, 3.0, true);
                let id = entity.id;

                service.create(entity.clone()).await.unwrap();

                // Mutate the name and email
                entity.name = "Alice Updated".to_string();
                entity.email = "alice.updated@test.com".to_string();
                entity.age = 26;

                let updated = service.update(&id, entity).await.unwrap();
                assert_eq!(updated.name(), "Alice Updated");
                assert_eq!(updated.email, "alice.updated@test.com");
                assert_eq!(updated.age, 26);

                // Verify persistence
                let retrieved = service.get(&id).await.unwrap().unwrap();
                assert_eq!(retrieved.name(), "Alice Updated");
                assert_eq!(retrieved.email, "alice.updated@test.com");
                assert_eq!(retrieved.age, 26);
            }

            // ==================================================================
            // CRUD — Update nonexistent
            // ==================================================================

            #[tokio::test]
            async fn test_update_nonexistent() {
                let service = $factory;
                let entity = create_test_entity("Ghost", "ghost@test.com", 0, 0.0, false);
                let id = entity.id;

                let result = service.update(&id, entity).await;
                assert!(
                    result.is_err(),
                    "Updating a nonexistent entity should return an error"
                );
            }

            // ==================================================================
            // CRUD — Delete existing
            // ==================================================================

            #[tokio::test]
            async fn test_delete_existing() {
                let service = $factory;
                let entity = create_test_entity("ToDelete", "delete@test.com", 40, 2.0, true);
                let id = entity.id;

                service.create(entity).await.unwrap();

                // Verify it exists
                assert!(service.get(&id).await.unwrap().is_some());

                // Delete
                service.delete(&id).await.unwrap();

                // Verify it's gone
                assert!(
                    service.get(&id).await.unwrap().is_none(),
                    "Entity should be gone after delete"
                );
            }

            // ==================================================================
            // CRUD — Delete nonexistent
            // ==================================================================

            /// Deleting a nonexistent entity: some backends return Ok (idempotent),
            /// others return Err (strict). Both behaviors are accepted.
            #[tokio::test]
            async fn test_delete_nonexistent() {
                let service = $factory;
                let random_id = Uuid::new_v4();

                let result = service.delete(&random_id).await;
                // Both Ok(()) and Err are acceptable behaviors.
                // We just verify the operation doesn't panic.
                match result {
                    Ok(()) => { /* Idempotent delete — in-memory style */ }
                    Err(_) => { /* Strict delete — SQL style (entity not found) */ }
                }
            }

            // ==================================================================
            // Search — String field (email)
            // ==================================================================

            #[tokio::test]
            async fn test_search_string_field() {
                let service = $factory;

                service
                    .create(create_test_entity("Alice", "alice@test.com", 25, 4.0, true))
                    .await
                    .unwrap();
                service
                    .create(create_test_entity("Bob", "bob@test.com", 30, 3.5, true))
                    .await
                    .unwrap();
                service
                    .create(create_test_entity(
                        "Charlie",
                        "charlie@test.com",
                        35,
                        5.0,
                        false,
                    ))
                    .await
                    .unwrap();

                let results = service.search("email", "alice@test.com").await.unwrap();
                assert_eq!(results.len(), 1, "Search should find exactly one match");
                assert_eq!(results[0].name(), "Alice");
                assert_eq!(results[0].email, "alice@test.com");
            }

            // ==================================================================
            // Search — Integer field (age)
            // ==================================================================

            #[tokio::test]
            async fn test_search_integer_field() {
                let service = $factory;

                service
                    .create(create_test_entity("Young", "young@test.com", 25, 1.0, true))
                    .await
                    .unwrap();
                service
                    .create(create_test_entity(
                        "Also25",
                        "also25@test.com",
                        25,
                        2.0,
                        true,
                    ))
                    .await
                    .unwrap();
                service
                    .create(create_test_entity("Old", "old@test.com", 60, 3.0, false))
                    .await
                    .unwrap();

                let results = service.search("age", "25").await.unwrap();
                assert_eq!(results.len(), 2, "Should find both entities with age=25");
                assert!(results.iter().all(|e| e.age == 25));
            }

            // ==================================================================
            // Search — Float field (score)
            // ==================================================================

            #[tokio::test]
            async fn test_search_float_field() {
                let service = $factory;

                service
                    .create(create_test_entity("High", "high@test.com", 30, 9.5, true))
                    .await
                    .unwrap();
                service
                    .create(create_test_entity("Low", "low@test.com", 20, 2.0, true))
                    .await
                    .unwrap();

                let results = service.search("score", "9.5").await.unwrap();
                assert_eq!(results.len(), 1, "Should find entity with score=9.5");
                assert_eq!(results[0].name(), "High");
            }

            // ==================================================================
            // Search — Boolean field (active)
            // ==================================================================

            #[tokio::test]
            async fn test_search_boolean_field() {
                let service = $factory;

                service
                    .create(create_test_entity("Active1", "a1@test.com", 20, 1.0, true))
                    .await
                    .unwrap();
                service
                    .create(create_test_entity("Active2", "a2@test.com", 25, 2.0, true))
                    .await
                    .unwrap();
                service
                    .create(create_test_entity(
                        "Inactive",
                        "inactive@test.com",
                        30,
                        3.0,
                        false,
                    ))
                    .await
                    .unwrap();

                let active_results = service.search("active", "true").await.unwrap();
                assert_eq!(active_results.len(), 2, "Should find 2 active entities");
                assert!(active_results.iter().all(|e| e.active));

                let inactive_results = service.search("active", "false").await.unwrap();
                assert_eq!(inactive_results.len(), 1, "Should find 1 inactive entity");
                assert!(!inactive_results[0].active);
            }

            // ==================================================================
            // Search — No results
            // ==================================================================

            #[tokio::test]
            async fn test_search_no_results() {
                let service = $factory;

                service
                    .create(create_test_entity("Alice", "alice@test.com", 25, 4.0, true))
                    .await
                    .unwrap();

                let results = service
                    .search("email", "nonexistent@nowhere.com")
                    .await
                    .unwrap();
                assert!(
                    results.is_empty(),
                    "Search with non-matching value should return empty vec"
                );
            }

            // ==================================================================
            // Search — Unknown field
            // ==================================================================

            #[tokio::test]
            async fn test_search_unknown_field() {
                let service = $factory;

                service
                    .create(create_test_entity("Alice", "alice@test.com", 25, 4.0, true))
                    .await
                    .unwrap();

                // Searching on a field that doesn't exist in field_value()
                let results = service
                    .search("nonexistent_field", "anything")
                    .await
                    .unwrap();
                assert!(
                    results.is_empty(),
                    "Search on unknown field should return empty vec"
                );
            }

            // ==================================================================
            // Edge case — Duplicate ID
            // ==================================================================

            /// Creating two entities with the same UUID: backends may either
            /// overwrite (upsert/in-memory) or reject (unique constraint/SQL).
            /// Both behaviors are valid — this test documents and accepts both.
            #[tokio::test]
            async fn test_create_duplicate_id() {
                let service = $factory;
                let id = Uuid::new_v4();

                let e1 = create_test_entity_with_id(id, "First", "first@test.com", 20, 1.0, true);
                service.create(e1).await.unwrap();

                let e2 =
                    create_test_entity_with_id(id, "Second", "second@test.com", 30, 2.0, false);
                let result = service.create(e2).await;

                match result {
                    Ok(_created) => {
                        // Overwrite/upsert behavior (in-memory, DynamoDB)
                        let retrieved = service.get(&id).await.unwrap().unwrap();
                        assert_eq!(
                            retrieved.name(),
                            "Second",
                            "Overwrite behavior: second entity should win"
                        );
                    }
                    Err(_) => {
                        // Unique constraint behavior (PostgreSQL, MySQL)
                        let retrieved = service.get(&id).await.unwrap().unwrap();
                        assert_eq!(
                            retrieved.name(),
                            "First",
                            "Unique constraint: first entity should remain"
                        );
                    }
                }
            }

            // ==================================================================
            // Edge case — Concurrent access
            // ==================================================================

            /// Test concurrent creates from multiple spawned tasks.
            ///
            /// Requires the service to be `Clone + Send + 'static` (which is the
            /// standard pattern: Clone shares the backing store via Arc).
            ///
            /// Uses multi-thread runtime because `tokio::spawn` with concurrent
            /// database connections (neo4rs pool) can deadlock on a single-thread
            /// runtime: the RowStream holds a pooled connection until dropped,
            /// starving the second task that awaits a connection from the same pool.
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn test_concurrent_access() {
                let service = $factory;
                let s1 = service.clone();
                let s2 = service.clone();

                let e1 = create_test_entity("Concurrent_A", "ca@test.com", 20, 1.0, true);
                let e2 = create_test_entity("Concurrent_B", "cb@test.com", 30, 2.0, false);
                let id1 = e1.id;
                let id2 = e2.id;

                let h1 = tokio::spawn(async move { s1.create(e1).await });
                let h2 = tokio::spawn(async move { s2.create(e2).await });

                let (r1, r2) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    tokio::try_join!(h1, h2).unwrap()
                })
                .await
                .expect("Concurrent creates timed out after 30s — possible deadlock");

                r1.unwrap();
                r2.unwrap();

                let all = service.list().await.unwrap();
                assert_eq!(
                    all.len(),
                    2,
                    "Both concurrently created entities should be present"
                );

                let ids: Vec<Uuid> = all.iter().map(|e| e.id()).collect();
                assert!(ids.contains(&id1), "Entity A should be present");
                assert!(ids.contains(&id2), "Entity B should be present");
            }
        }
    };
}
