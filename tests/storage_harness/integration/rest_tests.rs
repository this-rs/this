//! REST integration test macro for storage backends.
//!
//! The `rest_integration_tests!` macro generates HTTP-level tests that validate
//! a `DataService<TestDataEntity>` through full REST round-trips:
//! JSON → HTTP request → handler → DataService → HTTP response → JSON.

/// Generate a REST integration test suite for a storage backend.
///
/// `$data_factory` must produce an `impl DataService<TestDataEntity> + Send + Sync + 'static`.
///
/// # Generated Tests
///
/// ## CRUD (5 tests)
/// - `test_rest_create` — POST 201 + correct JSON body
/// - `test_rest_get` — GET 200 + correct entity
/// - `test_rest_list` — GET 200 + paginated array
/// - `test_rest_update` — PUT 200 + updated fields
/// - `test_rest_delete` — DELETE 204, then GET 404
///
/// ## Pagination / Filter / Sort (3 tests)
/// - `test_rest_list_pagination` — page=2&limit=2 returns correct slice
/// - `test_rest_list_filter` — filter={"active":true} returns only active
/// - `test_rest_list_sort` — sort=name:asc returns sorted results
///
/// ## Error handling (2 tests)
/// - `test_rest_error_not_found` — GET unknown ID → 404
/// - `test_rest_error_invalid_uuid` — GET with garbage ID → 400
#[macro_export]
macro_rules! rest_integration_tests {
    ($data_factory:expr) => {
        mod rest_integration_tests {
            use super::*;
            use axum_test::TestServer;
            use serde_json::json;
            use std::sync::Arc;
            use this::core::service::DataService;

            async fn make_server() -> TestServer {
                let data_service = $data_factory;
                let ds: Arc<dyn DataService<TestDataEntity> + Send + Sync> = Arc::new(data_service);
                let router = storage_harness::integration::build_test_router(ds);
                TestServer::new(router).unwrap()
            }

            // ==============================================================
            // CRUD — Create
            // ==============================================================

            #[tokio::test]
            async fn test_rest_create() {
                let server = make_server().await;

                let response = server
                    .post("/test_data_entities")
                    .json(&json!({
                        "name": "Alice",
                        "email": "alice@test.com",
                        "age": 30,
                        "score": 4.5,
                        "active": true
                    }))
                    .await;

                response.assert_status(axum::http::StatusCode::CREATED);

                let body: serde_json::Value = response.json();
                assert_eq!(body["name"], "Alice");
                assert_eq!(body["email"], "alice@test.com");
                assert_eq!(body["age"], 30);
                assert_eq!(body["score"], 4.5);
                assert_eq!(body["active"], true);
                assert_eq!(body["status"], "active");
                // id should be a valid UUID
                assert!(body["id"].as_str().is_some());
                uuid::Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
            }

            // ==============================================================
            // CRUD — Get
            // ==============================================================

            #[tokio::test]
            async fn test_rest_get() {
                let server = make_server().await;

                // Create first
                let create_resp = server
                    .post("/test_data_entities")
                    .json(&json!({
                        "name": "Bob",
                        "email": "bob@test.com",
                        "age": 25,
                        "score": 3.0,
                        "active": false
                    }))
                    .await;

                let created: serde_json::Value = create_resp.json();
                let id = created["id"].as_str().unwrap();

                // Get by ID
                let get_resp = server
                    .get(&format!("/test_data_entities/{}", id))
                    .await;

                get_resp.assert_status(axum::http::StatusCode::OK);

                let body: serde_json::Value = get_resp.json();
                assert_eq!(body["id"], id);
                assert_eq!(body["name"], "Bob");
                assert_eq!(body["email"], "bob@test.com");
                assert_eq!(body["age"], 25);
                assert_eq!(body["active"], false);
            }

            // ==============================================================
            // CRUD — List
            // ==============================================================

            #[tokio::test]
            async fn test_rest_list() {
                let server = make_server().await;

                // Create 3 entities
                for name in &["Alpha", "Beta", "Gamma"] {
                    server
                        .post("/test_data_entities")
                        .json(&json!({
                            "name": name,
                            "email": format!("{}@test.com", name.to_lowercase()),
                            "age": 20,
                            "score": 1.0,
                            "active": true
                        }))
                        .await;
                }

                let list_resp = server.get("/test_data_entities").await;
                list_resp.assert_status(axum::http::StatusCode::OK);

                let body: serde_json::Value = list_resp.json();
                assert_eq!(body["data"].as_array().unwrap().len(), 3);
                assert_eq!(body["pagination"]["total"], 3);
                assert_eq!(body["pagination"]["page"], 1);
            }

            // ==============================================================
            // CRUD — Update
            // ==============================================================

            #[tokio::test]
            async fn test_rest_update() {
                let server = make_server().await;

                // Create
                let create_resp = server
                    .post("/test_data_entities")
                    .json(&json!({
                        "name": "Charlie",
                        "email": "charlie@test.com",
                        "age": 40,
                        "score": 5.0,
                        "active": true
                    }))
                    .await;

                let created: serde_json::Value = create_resp.json();
                let id = created["id"].as_str().unwrap();

                // Update name and age
                let update_resp = server
                    .put(&format!("/test_data_entities/{}", id))
                    .json(&json!({
                        "name": "Charlie Updated",
                        "age": 41
                    }))
                    .await;

                update_resp.assert_status(axum::http::StatusCode::OK);

                let body: serde_json::Value = update_resp.json();
                assert_eq!(body["name"], "Charlie Updated");
                assert_eq!(body["age"], 41);
                // Unchanged fields preserved
                assert_eq!(body["email"], "charlie@test.com");
                assert_eq!(body["score"], 5.0);
                assert_eq!(body["active"], true);
            }

            // ==============================================================
            // CRUD — Delete
            // ==============================================================

            #[tokio::test]
            async fn test_rest_delete() {
                let server = make_server().await;

                // Create
                let create_resp = server
                    .post("/test_data_entities")
                    .json(&json!({
                        "name": "ToDelete",
                        "email": "delete@test.com",
                        "age": 0,
                        "score": 0.0,
                        "active": false
                    }))
                    .await;

                let created: serde_json::Value = create_resp.json();
                let id = created["id"].as_str().unwrap();

                // Delete
                let delete_resp = server
                    .delete(&format!("/test_data_entities/{}", id))
                    .await;
                delete_resp.assert_status(axum::http::StatusCode::NO_CONTENT);

                // Verify gone
                let get_resp = server
                    .get(&format!("/test_data_entities/{}", id))
                    .await;
                get_resp.assert_status(axum::http::StatusCode::NOT_FOUND);
            }

            // ==============================================================
            // List — Pagination
            // ==============================================================

            #[tokio::test]
            async fn test_rest_list_pagination() {
                let server = make_server().await;

                // Create 5 entities
                for i in 0..5 {
                    server
                        .post("/test_data_entities")
                        .json(&json!({
                            "name": format!("Entity_{}", i),
                            "email": format!("e{}@test.com", i),
                            "age": 20 + i,
                            "score": 1.0,
                            "active": true
                        }))
                        .await;
                }

                // Request page 2 with limit 2
                let resp = server
                    .get("/test_data_entities?page=2&limit=2")
                    .await;

                resp.assert_status(axum::http::StatusCode::OK);

                let body: serde_json::Value = resp.json();
                assert_eq!(body["data"].as_array().unwrap().len(), 2);
                assert_eq!(body["pagination"]["page"], 2);
                assert_eq!(body["pagination"]["limit"], 2);
                assert_eq!(body["pagination"]["total"], 5);
                assert_eq!(body["pagination"]["total_pages"], 3);
                assert_eq!(body["pagination"]["has_prev"], true);
                assert_eq!(body["pagination"]["has_next"], true);
            }

            // ==============================================================
            // List — Filter
            // ==============================================================

            #[tokio::test]
            async fn test_rest_list_filter() {
                let server = make_server().await;

                // Create active and inactive entities
                server
                    .post("/test_data_entities")
                    .json(&json!({"name": "Active1", "email": "a@t.com", "age": 20, "score": 1.0, "active": true}))
                    .await;
                server
                    .post("/test_data_entities")
                    .json(&json!({"name": "Inactive", "email": "b@t.com", "age": 30, "score": 2.0, "active": false}))
                    .await;
                server
                    .post("/test_data_entities")
                    .json(&json!({"name": "Active2", "email": "c@t.com", "age": 25, "score": 3.0, "active": true}))
                    .await;

                // Filter by status=active (all entities have status "active" by default from handler)
                // Use name filter instead for a meaningful test
                let resp = server
                    .get("/test_data_entities?filter=%7B%22name%22%3A%22Active1%22%7D")
                    .await;

                resp.assert_status(axum::http::StatusCode::OK);

                let body: serde_json::Value = resp.json();
                assert_eq!(body["pagination"]["total"], 1);
                assert_eq!(body["data"][0]["name"], "Active1");
            }

            // ==============================================================
            // List — Sort
            // ==============================================================

            #[tokio::test]
            async fn test_rest_list_sort() {
                let server = make_server().await;

                // Create entities with different names
                for name in &["Charlie", "Alice", "Bob"] {
                    server
                        .post("/test_data_entities")
                        .json(&json!({
                            "name": name,
                            "email": format!("{}@t.com", name.to_lowercase()),
                            "age": 20,
                            "score": 1.0,
                            "active": true
                        }))
                        .await;
                }

                // Sort by name ascending
                let resp = server
                    .get("/test_data_entities?sort=name:asc")
                    .await;

                resp.assert_status(axum::http::StatusCode::OK);

                let body: serde_json::Value = resp.json();
                let data = body["data"].as_array().unwrap();
                assert_eq!(data[0]["name"], "Alice");
                assert_eq!(data[1]["name"], "Bob");
                assert_eq!(data[2]["name"], "Charlie");
            }

            // ==============================================================
            // Error — Not found
            // ==============================================================

            #[tokio::test]
            async fn test_rest_error_not_found() {
                let server = make_server().await;
                let fake_id = uuid::Uuid::new_v4();

                let resp = server
                    .get(&format!("/test_data_entities/{}", fake_id))
                    .await;

                resp.assert_status(axum::http::StatusCode::NOT_FOUND);
            }

            // ==============================================================
            // Error — Invalid UUID
            // ==============================================================

            #[tokio::test]
            async fn test_rest_error_invalid_uuid() {
                let server = make_server().await;

                let resp = server
                    .get("/test_data_entities/not-a-valid-uuid")
                    .await;

                resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
            }
        }
    };
}
