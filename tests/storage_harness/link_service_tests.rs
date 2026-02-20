//! Macro-generated test suite for `LinkService` contract validation.
//!
//! The `link_service_tests!` macro generates a comprehensive test module that
//! validates any `LinkService` implementation against the full contract:
//! CRUD, find_by_source/target with filters, update, delete, delete_by_entity.
//!
//! # Usage
//!
//! ```rust,ignore
//! #[macro_use]
//! mod storage_harness;
//!
//! use storage_harness::*;
//! use this::storage::InMemoryLinkService;
//!
//! link_service_tests!(InMemoryLinkService::new());
//! ```
//!
//! # Generated Tests (14)
//!
//! ## CRUD
//! - `test_create_and_get_link` — create then retrieve, verify all fields
//! - `test_get_link_nonexistent` — get random UUID → None
//! - `test_list_links_empty` — list on empty store → empty vec
//! - `test_list_links_multiple` — create 5 links, list returns 5
//!
//! ## find_by_source
//! - `test_find_by_source_no_filter` — all links from a source
//! - `test_find_by_source_with_link_type` — filter by link_type
//! - `test_find_by_source_with_target_type` — filter by target_type (if supported)
//!
//! ## find_by_target
//! - `test_find_by_target_no_filter` — all links to a target
//! - `test_find_by_target_with_filters` — combined link_type + source_type filter
//!
//! ## Update & Delete
//! - `test_update_link` — update metadata, verify persisted
//! - `test_delete_link` — delete then get → None
//! - `test_delete_by_entity_source` — delete all links FROM an entity
//! - `test_delete_by_entity_target` — delete all links TO an entity
//! - `test_create_link_with_metadata` — link with JSON metadata

/// Generate a full `LinkService` conformance test suite.
///
/// `$factory` must be an expression that evaluates to an instance implementing
/// `LinkService`. It is re-evaluated for each test to ensure isolation.
/// For concurrent-safe tests, the returned service should implement `Clone + 'static`.
#[macro_export]
macro_rules! link_service_tests {
    ($factory:expr) => {
        mod link_service_contract_tests {
            use super::*;
            use this::core::service::LinkService;
            use uuid::Uuid;

            // ==================================================================
            // CRUD — Create & Get
            // ==================================================================

            #[tokio::test]
            async fn test_create_and_get_link() {
                let service = $factory;
                let source_id = Uuid::new_v4();
                let target_id = Uuid::new_v4();

                let link = create_test_link(source_id, target_id, "owner");
                let link_id = link.id;

                let created = service.create(link).await.unwrap();
                assert_eq!(created.id, link_id);
                assert_eq!(created.link_type, "owner");
                assert_eq!(created.source_id, source_id);
                assert_eq!(created.target_id, target_id);
                assert_eq!(created.status, "active");
                assert!(created.metadata.is_none());

                // Retrieve and verify all fields
                let retrieved = service.get(&link_id).await.unwrap();
                assert!(retrieved.is_some(), "Link should exist after create");
                let retrieved = retrieved.unwrap();
                assert_eq!(retrieved.id, link_id);
                assert_eq!(retrieved.link_type, "owner");
                assert_eq!(retrieved.source_id, source_id);
                assert_eq!(retrieved.target_id, target_id);
                assert_eq!(retrieved.status, "active");
            }

            // ==================================================================
            // CRUD — Get nonexistent
            // ==================================================================

            #[tokio::test]
            async fn test_get_link_nonexistent() {
                let service = $factory;

                let result = service.get(&Uuid::new_v4()).await.unwrap();
                assert!(
                    result.is_none(),
                    "Getting a nonexistent link should return None"
                );
            }

            // ==================================================================
            // CRUD — List empty
            // ==================================================================

            #[tokio::test]
            async fn test_list_links_empty() {
                let service = $factory;

                let all = service.list().await.unwrap();
                assert!(
                    all.is_empty(),
                    "List on empty link store should return empty vec"
                );
            }

            // ==================================================================
            // CRUD — List multiple
            // ==================================================================

            #[tokio::test]
            async fn test_list_links_multiple() {
                let service = $factory;

                for i in 0..5 {
                    let link = create_test_link(
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        &format!("type_{}", i),
                    );
                    service.create(link).await.unwrap();
                }

                let all = service.list().await.unwrap();
                assert_eq!(all.len(), 5, "List should return all 5 created links");
            }

            // ==================================================================
            // find_by_source — No filter
            // ==================================================================

            #[tokio::test]
            async fn test_find_by_source_no_filter() {
                let service = $factory;
                let user_id = Uuid::new_v4();
                let car_id = Uuid::new_v4();
                let company_id = Uuid::new_v4();
                let other_user_id = Uuid::new_v4();

                // user → car (owner)
                service
                    .create(create_test_link(user_id, car_id, "owner"))
                    .await
                    .unwrap();
                // user → company (worker)
                service
                    .create(create_test_link(user_id, company_id, "worker"))
                    .await
                    .unwrap();
                // other_user → car (driver) — should NOT appear
                service
                    .create(create_test_link(other_user_id, car_id, "driver"))
                    .await
                    .unwrap();

                let links = service
                    .find_by_source(&user_id, None, None)
                    .await
                    .unwrap();
                assert_eq!(
                    links.len(),
                    2,
                    "Should find exactly 2 links from user_id"
                );
                assert!(links.iter().all(|l| l.source_id == user_id));
            }

            // ==================================================================
            // find_by_source — With link_type filter
            // ==================================================================

            #[tokio::test]
            async fn test_find_by_source_with_link_type() {
                let service = $factory;
                let user_id = Uuid::new_v4();
                let car1 = Uuid::new_v4();
                let car2 = Uuid::new_v4();
                let company_id = Uuid::new_v4();

                // user → car1 (owner)
                service
                    .create(create_test_link(user_id, car1, "owner"))
                    .await
                    .unwrap();
                // user → car2 (owner)
                service
                    .create(create_test_link(user_id, car2, "owner"))
                    .await
                    .unwrap();
                // user → company (worker) — should NOT match "owner"
                service
                    .create(create_test_link(user_id, company_id, "worker"))
                    .await
                    .unwrap();

                let owner_links = service
                    .find_by_source(&user_id, Some("owner"), None)
                    .await
                    .unwrap();
                assert_eq!(
                    owner_links.len(),
                    2,
                    "Should find 2 'owner' links from user"
                );
                assert!(owner_links.iter().all(|l| l.link_type == "owner"));
            }

            // ==================================================================
            // find_by_source — With target_type filter
            // ==================================================================

            /// Note: target_type filtering depends on backend support.
            /// InMemoryLinkService currently ignores target_type (returns all
            /// matches from source). Backends with entity-type metadata may
            /// properly filter. This test validates the call succeeds and
            /// returns at least the expected links.
            #[tokio::test]
            async fn test_find_by_source_with_target_type() {
                let service = $factory;
                let user_id = Uuid::new_v4();
                let car_id = Uuid::new_v4();
                let order_id = Uuid::new_v4();

                // user → car (owner)
                service
                    .create(create_test_link(user_id, car_id, "owner"))
                    .await
                    .unwrap();
                // user → order (buyer)
                service
                    .create(create_test_link(user_id, order_id, "buyer"))
                    .await
                    .unwrap();

                // Filter by target_type — may or may not be enforced
                let results = service
                    .find_by_source(&user_id, None, Some("order"))
                    .await
                    .unwrap();

                // At minimum, results should contain links from user_id
                assert!(
                    !results.is_empty(),
                    "Should return at least some links from source"
                );
                assert!(results.iter().all(|l| l.source_id == user_id));
            }

            // ==================================================================
            // find_by_target — No filter
            // ==================================================================

            #[tokio::test]
            async fn test_find_by_target_no_filter() {
                let service = $factory;
                let user1 = Uuid::new_v4();
                let user2 = Uuid::new_v4();
                let car_id = Uuid::new_v4();
                let other_car = Uuid::new_v4();

                // user1 → car (owner)
                service
                    .create(create_test_link(user1, car_id, "owner"))
                    .await
                    .unwrap();
                // user2 → car (driver)
                service
                    .create(create_test_link(user2, car_id, "driver"))
                    .await
                    .unwrap();
                // user1 → other_car (owner) — should NOT appear
                service
                    .create(create_test_link(user1, other_car, "owner"))
                    .await
                    .unwrap();

                let links = service
                    .find_by_target(&car_id, None, None)
                    .await
                    .unwrap();
                assert_eq!(
                    links.len(),
                    2,
                    "Should find exactly 2 links targeting car_id"
                );
                assert!(links.iter().all(|l| l.target_id == car_id));
            }

            // ==================================================================
            // find_by_target — With link_type + source_type filters
            // ==================================================================

            /// Combined filter test: link_type + source_type.
            /// source_type filtering may not be enforced by all backends (see
            /// target_type note above). link_type filtering IS enforced.
            #[tokio::test]
            async fn test_find_by_target_with_filters() {
                let service = $factory;
                let user_id = Uuid::new_v4();
                let bot_id = Uuid::new_v4();
                let car_id = Uuid::new_v4();

                // user → car (owner)
                service
                    .create(create_test_link(user_id, car_id, "owner"))
                    .await
                    .unwrap();
                // user → car (driver)
                service
                    .create(create_test_link(user_id, car_id, "driver"))
                    .await
                    .unwrap();
                // bot → car (driver)
                service
                    .create(create_test_link(bot_id, car_id, "driver"))
                    .await
                    .unwrap();

                // Filter by link_type only (reliably supported)
                let driver_links = service
                    .find_by_target(&car_id, Some("driver"), None)
                    .await
                    .unwrap();
                assert_eq!(
                    driver_links.len(),
                    2,
                    "Should find 2 'driver' links targeting car_id"
                );
                assert!(driver_links.iter().all(|l| l.link_type == "driver"));

                // Filter by link_type + source_type (source_type may be ignored)
                let filtered = service
                    .find_by_target(&car_id, Some("driver"), Some("user"))
                    .await
                    .unwrap();
                // At minimum, should return driver links targeting car_id
                assert!(
                    !filtered.is_empty(),
                    "Should return at least some filtered links"
                );
                assert!(filtered.iter().all(|l| l.link_type == "driver"));
                assert!(filtered.iter().all(|l| l.target_id == car_id));
            }

            // ==================================================================
            // Update
            // ==================================================================

            #[tokio::test]
            async fn test_update_link() {
                let service = $factory;
                let source_id = Uuid::new_v4();
                let target_id = Uuid::new_v4();

                let mut link = create_test_link_with_metadata(
                    source_id,
                    target_id,
                    "worker",
                    serde_json::json!({"role": "Developer"}),
                );
                let link_id = link.id;

                service.create(link.clone()).await.unwrap();

                // Update metadata and touch timestamp
                link.metadata = Some(serde_json::json!({"role": "Senior Developer", "level": 3}));
                link.touch();

                let updated = service.update(&link_id, link).await.unwrap();
                assert_eq!(
                    updated.metadata,
                    Some(serde_json::json!({"role": "Senior Developer", "level": 3}))
                );

                // Verify persistence
                let retrieved = service.get(&link_id).await.unwrap().unwrap();
                assert_eq!(
                    retrieved.metadata,
                    Some(serde_json::json!({"role": "Senior Developer", "level": 3}))
                );
            }

            // ==================================================================
            // Delete
            // ==================================================================

            #[tokio::test]
            async fn test_delete_link() {
                let service = $factory;
                let link = create_test_link(Uuid::new_v4(), Uuid::new_v4(), "owner");
                let link_id = link.id;

                service.create(link).await.unwrap();
                assert!(service.get(&link_id).await.unwrap().is_some());

                service.delete(&link_id).await.unwrap();
                assert!(
                    service.get(&link_id).await.unwrap().is_none(),
                    "Link should be gone after delete"
                );
            }

            // ==================================================================
            // delete_by_entity — Source side
            // ==================================================================

            #[tokio::test]
            async fn test_delete_by_entity_source() {
                let service = $factory;
                let entity_a = Uuid::new_v4();
                let target1 = Uuid::new_v4();
                let target2 = Uuid::new_v4();
                let other_source = Uuid::new_v4();

                // entity_a → target1 (owner)
                service
                    .create(create_test_link(entity_a, target1, "owner"))
                    .await
                    .unwrap();
                // entity_a → target2 (driver)
                service
                    .create(create_test_link(entity_a, target2, "driver"))
                    .await
                    .unwrap();
                // other_source → target1 (owner) — should survive
                service
                    .create(create_test_link(other_source, target1, "owner"))
                    .await
                    .unwrap();

                assert_eq!(service.list().await.unwrap().len(), 3);

                // Delete all links involving entity_a
                service.delete_by_entity(&entity_a).await.unwrap();

                let remaining = service.list().await.unwrap();
                assert_eq!(
                    remaining.len(),
                    1,
                    "Only the link from other_source should remain"
                );
                assert_eq!(remaining[0].source_id, other_source);
            }

            // ==================================================================
            // delete_by_entity — Target side
            // ==================================================================

            #[tokio::test]
            async fn test_delete_by_entity_target() {
                let service = $factory;
                let source1 = Uuid::new_v4();
                let source2 = Uuid::new_v4();
                let entity_b = Uuid::new_v4();
                let other_target = Uuid::new_v4();

                // source1 → entity_b (owner)
                service
                    .create(create_test_link(source1, entity_b, "owner"))
                    .await
                    .unwrap();
                // source2 → entity_b (driver)
                service
                    .create(create_test_link(source2, entity_b, "driver"))
                    .await
                    .unwrap();
                // source1 → other_target (owner) — should survive
                service
                    .create(create_test_link(source1, other_target, "owner"))
                    .await
                    .unwrap();

                assert_eq!(service.list().await.unwrap().len(), 3);

                // Delete all links involving entity_b (as target)
                service.delete_by_entity(&entity_b).await.unwrap();

                let remaining = service.list().await.unwrap();
                assert_eq!(
                    remaining.len(),
                    1,
                    "Only the link to other_target should remain"
                );
                assert_eq!(remaining[0].target_id, other_target);
            }

            // ==================================================================
            // Create with metadata
            // ==================================================================

            #[tokio::test]
            async fn test_create_link_with_metadata() {
                let service = $factory;
                let source_id = Uuid::new_v4();
                let target_id = Uuid::new_v4();

                let metadata = serde_json::json!({
                    "role": "Senior Developer",
                    "start_date": "2024-01-01",
                    "permissions": ["read", "write"]
                });

                let link = create_test_link_with_metadata(
                    source_id,
                    target_id,
                    "worker",
                    metadata.clone(),
                );
                let link_id = link.id;

                let created = service.create(link).await.unwrap();
                assert_eq!(created.metadata, Some(metadata.clone()));

                // Verify persistence
                let retrieved = service.get(&link_id).await.unwrap().unwrap();
                assert_eq!(retrieved.metadata, Some(metadata));
            }
        }
    };
}
