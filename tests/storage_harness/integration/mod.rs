//! Integration test infrastructure for storage backends.
//!
//! Provides HTTP handlers, router builder, and test macros for validating
//! storage backends through the full REST layer (HTTP → handler → DataService → response).
//!
//! # Architecture
//!
//! ```text
//! axum_test::TestServer
//!     └─ Router (built by build_test_router)
//!         ├─ POST   /test_data_entities       → create_handler
//!         ├─ GET    /test_data_entities        → list_handler
//!         ├─ GET    /test_data_entities/{id}   → get_handler
//!         ├─ PUT    /test_data_entities/{id}   → update_handler
//!         └─ DELETE /test_data_entities/{id}   → delete_handler
//! ```

#[macro_use]
pub mod rest_tests;

#[cfg(feature = "graphql")]
pub mod graphql_tests;

#[cfg(feature = "grpc")]
pub mod grpc_tests;

#[cfg(feature = "websocket")]
pub mod ws_tests;

use super::{TestDataEntity, create_test_entity};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde_json::Value;
use std::sync::Arc;
use this::core::entity::Data;
use this::core::query::{PaginatedResponse, PaginationMeta, QueryParams};
use this::core::service::DataService;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared state for test REST handlers
// ---------------------------------------------------------------------------

/// Application state wrapping a type-erased `DataService<TestDataEntity>`.
///
/// Any backend implementing `DataService<TestDataEntity>` can be used.
#[derive(Clone)]
pub struct TestApiState {
    pub data_service: Arc<dyn DataService<TestDataEntity> + Send + Sync>,
}

// ---------------------------------------------------------------------------
// REST handlers for TestDataEntity
// ---------------------------------------------------------------------------

/// POST /test_data_entities — Create a new test entity from JSON body.
///
/// Expects: `{ "name": "...", "email": "...", "age": N, "score": F, "active": B }`
/// Returns: 201 Created + JSON entity
async fn create_handler(
    State(state): State<TestApiState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let entity = create_test_entity(
        body["name"].as_str().unwrap_or("unnamed"),
        body["email"].as_str().unwrap_or(""),
        body["age"].as_i64().unwrap_or(0),
        body["score"].as_f64().unwrap_or(0.0),
        body["active"].as_bool().unwrap_or(true),
    );

    match state.data_service.create(entity).await {
        Ok(created) => {
            let json = serde_json::to_value(created).unwrap();
            (StatusCode::CREATED, Json(json)).into_response()
        }
        Err(e) => {
            let err = serde_json::json!({"error": e.to_string()});
            (StatusCode::INTERNAL_SERVER_ERROR, Json(err)).into_response()
        }
    }
}

/// GET /test_data_entities/{id} — Get a single entity by UUID.
///
/// Returns: 200 + JSON entity, or 404 if not found
async fn get_handler(
    State(state): State<TestApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid UUID"}))).into_response(),
    };

    match state.data_service.get(&id).await {
        Ok(Some(entity)) => {
            let json = serde_json::to_value(entity).unwrap();
            (StatusCode::OK, Json(json)).into_response()
        }
        Ok(None) => {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Not found"}))).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response()
        }
    }
}

/// GET /test_data_entities — List all entities with pagination.
///
/// Query params: `?page=1&limit=20&filter={"status":"active"}&sort=name:asc`
/// Returns: 200 + PaginatedResponse<Value>
async fn list_handler(
    State(state): State<TestApiState>,
    Query(params): Query<QueryParams>,
) -> impl IntoResponse {
    let page = params.page();
    let limit = params.limit();

    match state.data_service.list().await {
        Ok(mut entities) => {
            // Apply filter if provided
            if let Some(filter) = params.filter_value() {
                if let Some(obj) = filter.as_object() {
                    for (key, value) in obj {
                        entities.retain(|e| {
                            e.field_value(key)
                                .map(|fv| match &fv {
                                    this::core::field::FieldValue::String(s) => {
                                        value.as_str().is_some_and(|v| s == v)
                                    }
                                    this::core::field::FieldValue::Integer(i) => {
                                        value.as_i64().is_some_and(|v| *i == v)
                                    }
                                    this::core::field::FieldValue::Boolean(b) => {
                                        value.as_bool().is_some_and(|v| *b == v)
                                    }
                                    _ => false,
                                })
                                .unwrap_or(false)
                        });
                    }
                }
            }

            // Apply sort if provided
            if let Some(sort) = &params.sort {
                match sort.as_str() {
                    "name" | "name:asc" => entities.sort_by(|a, b| a.name.cmp(&b.name)),
                    "name:desc" => entities.sort_by(|a, b| b.name.cmp(&a.name)),
                    "age" | "age:asc" => entities.sort_by(|a, b| a.age.cmp(&b.age)),
                    "age:desc" => entities.sort_by(|a, b| b.age.cmp(&a.age)),
                    _ => {}
                }
            }

            let total = entities.len();
            let start = (page - 1) * limit;

            let paginated: Vec<Value> = entities
                .into_iter()
                .skip(start)
                .take(limit)
                .map(|e| serde_json::to_value(e).unwrap())
                .collect();

            let response = PaginatedResponse {
                data: paginated,
                pagination: PaginationMeta::new(page, limit, total),
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response()
        }
    }
}

/// PUT /test_data_entities/{id} — Update an existing entity.
///
/// Returns: 200 + updated JSON entity, or 404 if not found
async fn update_handler(
    State(state): State<TestApiState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid UUID"}))).into_response(),
    };

    // Get existing entity
    let existing = match state.data_service.get(&id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Not found"}))).into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response();
        }
    };

    // Apply updates (merge with existing)
    let updated = TestDataEntity {
        id: existing.id,
        entity_type: existing.entity_type.clone(),
        created_at: existing.created_at,
        updated_at: chrono::Utc::now(),
        deleted_at: existing.deleted_at,
        status: body["status"]
            .as_str()
            .unwrap_or(&existing.status)
            .to_string(),
        name: body["name"]
            .as_str()
            .unwrap_or(&existing.name)
            .to_string(),
        email: body["email"]
            .as_str()
            .unwrap_or(&existing.email)
            .to_string(),
        age: body["age"].as_i64().unwrap_or(existing.age),
        score: body["score"].as_f64().unwrap_or(existing.score),
        active: body["active"].as_bool().unwrap_or(existing.active),
    };

    match state.data_service.update(&id, updated).await {
        Ok(entity) => {
            let json = serde_json::to_value(entity).unwrap();
            (StatusCode::OK, Json(json)).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response()
        }
    }
}

/// DELETE /test_data_entities/{id} — Delete an entity.
///
/// Returns: 204 No Content, or 404 if not found
async fn delete_handler(
    State(state): State<TestApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Check existence first for consistent 404 behavior
    match state.data_service.get(&id).await {
        Ok(Some(_)) => {
            match state.data_service.delete(&id).await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build a test Router for the given storage backend.
///
/// The router exposes REST CRUD endpoints for `TestDataEntity` backed by
/// the provided `DataService` implementation.
///
/// # Usage
/// ```rust,ignore
/// let router = build_test_router(
///     Arc::new(InMemoryDataService::<TestDataEntity>::new()),
/// );
/// let server = axum_test::TestServer::new(router).unwrap();
/// ```
pub fn build_test_router(
    data_service: Arc<dyn DataService<TestDataEntity> + Send + Sync>,
) -> Router {
    let state = TestApiState { data_service };

    Router::new()
        .route(
            "/test_data_entities",
            get(list_handler).post(create_handler),
        )
        .route(
            "/test_data_entities/{id}",
            get(get_handler).put(update_handler).delete(delete_handler),
        )
        .with_state(state)
}
