//! REST endpoints for notifications, preferences, and device tokens
//!
//! # Notification endpoints
//!
//! - `GET  /notifications/:user_id`                — List notifications (paginated)
//! - `GET  /notifications/:user_id/unread-count`   — Get unread count
//! - `POST /notifications/:user_id/read`           — Mark specific notifications as read
//! - `POST /notifications/:user_id/read-all`       — Mark all notifications as read
//! - `DELETE /notifications/:user_id/:notification_id` — Delete a notification
//!
//! # Preferences endpoints
//!
//! - `GET  /notifications/:user_id/preferences`    — Get user preferences
//! - `PUT  /notifications/:user_id/preferences`    — Update preferences
//! - `POST /notifications/:user_id/mute`           — Mute all notifications
//! - `POST /notifications/:user_id/unmute`         — Unmute all notifications
//!
//! # Device token endpoints
//!
//! - `GET    /device-tokens/:user_id`              — List device tokens
//! - `POST   /device-tokens/:user_id`              — Register a device token
//! - `DELETE /device-tokens/:user_id/:token`       — Unregister a device token

use crate::events::sinks::device_tokens::{DeviceTokenStore, Platform};
use crate::events::sinks::in_app::NotificationStore;
use crate::events::sinks::preferences::{NotificationPreferencesStore, UserPreferences};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, routing::{delete, get, post}};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

// ── Shared state ──────────────────────────────────────────────────────

/// Shared state for notification-related endpoints
#[derive(Clone)]
pub struct NotificationState {
    pub notification_store: Arc<NotificationStore>,
    pub preferences_store: Arc<NotificationPreferencesStore>,
    pub device_token_store: Arc<DeviceTokenStore>,
}

/// Build the notification routes
///
/// Returns a Router with all notification, preferences, and device token endpoints.
pub fn notification_routes(state: NotificationState) -> Router {
    Router::new()
        // Notification endpoints
        .route(
            "/notifications/{user_id}",
            get(list_notifications),
        )
        .route(
            "/notifications/{user_id}/unread-count",
            get(unread_count),
        )
        .route(
            "/notifications/{user_id}/read",
            post(mark_as_read),
        )
        .route(
            "/notifications/{user_id}/read-all",
            post(mark_all_as_read),
        )
        .route(
            "/notifications/{user_id}/{notification_id}",
            delete(delete_notification),
        )
        // Preferences endpoints
        .route(
            "/notifications/{user_id}/preferences",
            get(get_preferences).put(update_preferences),
        )
        .route(
            "/notifications/{user_id}/mute",
            post(mute_user),
        )
        .route(
            "/notifications/{user_id}/unmute",
            post(unmute_user),
        )
        // Device token endpoints
        .route(
            "/device-tokens/{user_id}",
            get(list_device_tokens).post(register_device_token),
        )
        .route(
            "/device-tokens/{user_id}/{token}",
            delete(unregister_device_token),
        )
        .with_state(state)
}

// ── Query parameters ──────────────────────────────────────────────────

/// Pagination query parameters
#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    /// Maximum number of items to return (default: 20, max: 100)
    pub limit: Option<usize>,
    /// Number of items to skip (default: 0)
    pub offset: Option<usize>,
}

// ── Notification handlers ─────────────────────────────────────────────

/// List notifications for a user (newest first)
async fn list_notifications(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(20).min(100);
    let offset = params.offset.unwrap_or(0);

    let notifications = state
        .notification_store
        .list_by_user(&user_id, limit, offset)
        .await;

    let total = state.notification_store.total_count(&user_id).await;
    let unread = state.notification_store.unread_count(&user_id).await;

    Json(json!({
        "notifications": notifications,
        "total": total,
        "unread": unread,
        "limit": limit,
        "offset": offset,
    }))
}

/// Get unread notification count for a user
async fn unread_count(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    let count = state.notification_store.unread_count(&user_id).await;
    Json(json!({ "unread_count": count }))
}

/// Request body for mark_as_read
#[derive(Debug, Deserialize)]
pub struct MarkAsReadRequest {
    /// List of notification IDs to mark as read
    pub ids: Vec<Uuid>,
}

/// Mark specific notifications as read
async fn mark_as_read(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
    Json(body): Json<MarkAsReadRequest>,
) -> impl IntoResponse {
    let marked = state
        .notification_store
        .mark_as_read(&body.ids, Some(&user_id))
        .await;

    Json(json!({ "marked": marked }))
}

/// Mark all notifications as read for a user
async fn mark_all_as_read(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    let marked = state
        .notification_store
        .mark_all_as_read(&user_id)
        .await;

    Json(json!({ "marked": marked }))
}

/// Delete a notification by ID
async fn delete_notification(
    State(state): State<NotificationState>,
    Path((_user_id, notification_id)): Path<(String, Uuid)>,
) -> impl IntoResponse {
    let deleted = state
        .notification_store
        .delete(&notification_id)
        .await;

    if deleted {
        (StatusCode::OK, Json(json!({ "deleted": true })))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "notification not found" })),
        )
    }
}

// ── Preferences handlers ──────────────────────────────────────────────

/// Get notification preferences for a user
async fn get_preferences(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    let prefs = state.preferences_store.get(&user_id).await;
    Json(json!({ "preferences": prefs }))
}

/// Update notification preferences for a user
async fn update_preferences(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
    Json(prefs): Json<UserPreferences>,
) -> impl IntoResponse {
    state.preferences_store.update(&user_id, prefs.clone()).await;
    Json(json!({ "preferences": prefs }))
}

/// Mute all notifications for a user
async fn mute_user(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    state.preferences_store.mute(&user_id).await;
    Json(json!({ "muted": true }))
}

/// Unmute all notifications for a user
async fn unmute_user(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    state.preferences_store.unmute(&user_id).await;
    Json(json!({ "muted": false }))
}

// ── Device token handlers ─────────────────────────────────────────────

/// List device tokens for a user
async fn list_device_tokens(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    let tokens = state.device_token_store.get_tokens(&user_id).await;
    Json(json!({ "tokens": tokens }))
}

/// Request body for device token registration
#[derive(Debug, Deserialize)]
pub struct RegisterTokenRequest {
    /// The push token string
    pub token: String,
    /// Platform: "ios", "android", or "web"
    pub platform: Platform,
}

/// Register a device token for push notifications
async fn register_device_token(
    State(state): State<NotificationState>,
    Path(user_id): Path<String>,
    Json(body): Json<RegisterTokenRequest>,
) -> impl IntoResponse {
    state
        .device_token_store
        .register(&user_id, body.token, body.platform)
        .await;

    (StatusCode::CREATED, Json(json!({ "registered": true })))
}

/// Unregister a device token
async fn unregister_device_token(
    State(state): State<NotificationState>,
    Path((user_id, token)): Path<(String, String)>,
) -> impl IntoResponse {
    let removed = state
        .device_token_store
        .unregister(&user_id, &token)
        .await;

    if removed {
        (StatusCode::OK, Json(json!({ "unregistered": true })))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "token not found" })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    fn test_state() -> NotificationState {
        NotificationState {
            notification_store: Arc::new(NotificationStore::new()),
            preferences_store: Arc::new(NotificationPreferencesStore::new()),
            device_token_store: Arc::new(DeviceTokenStore::new()),
        }
    }

    fn test_router() -> Router {
        notification_routes(test_state())
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let body = axum::body::to_bytes(response.into_body(), 1024 * 64)
            .await
            .expect("body should read");
        serde_json::from_slice(&body).expect("body should be valid JSON")
    }

    // ── Notification tests ────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_notifications_empty() {
        let router = test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/notifications/user-A")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["total"], 0);
        assert_eq!(json["unread"], 0);
        assert!(json["notifications"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_list_and_unread_count() {
        let state = test_state();
        let router = notification_routes(state.clone());

        // Insert some notifications
        for i in 0..3 {
            state
                .notification_store
                .insert(crate::events::sinks::in_app::StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {i}"),
                    body: String::new(),
                    data: serde_json::Value::Null,
                    read: false,
                    created_at: chrono::Utc::now(),
                })
                .await;
        }

        // List
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/notifications/user-A")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = json_body(response).await;
        assert_eq!(json["total"], 3);
        assert_eq!(json["unread"], 3);

        // Unread count
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/notifications/user-A/unread-count")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = json_body(response).await;
        assert_eq!(json["unread_count"], 3);
    }

    #[tokio::test]
    async fn test_mark_as_read() {
        let state = test_state();
        let router = notification_routes(state.clone());

        let id = Uuid::new_v4();
        state
            .notification_store
            .insert(crate::events::sinks::in_app::StoredNotification {
                id,
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "Test".to_string(),
                body: String::new(),
                data: serde_json::Value::Null,
                read: false,
                created_at: chrono::Utc::now(),
            })
            .await;

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/notifications/user-A/read")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({ "ids": [id] })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["marked"], 1);
        assert_eq!(state.notification_store.unread_count("user-A").await, 0);
    }

    #[tokio::test]
    async fn test_mark_all_as_read() {
        let state = test_state();
        let router = notification_routes(state.clone());

        for _ in 0..3 {
            state
                .notification_store
                .insert(crate::events::sinks::in_app::StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: "Test".to_string(),
                    body: String::new(),
                    data: serde_json::Value::Null,
                    read: false,
                    created_at: chrono::Utc::now(),
                })
                .await;
        }

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/notifications/user-A/read-all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["marked"], 3);
    }

    #[tokio::test]
    async fn test_delete_notification() {
        let state = test_state();
        let router = notification_routes(state.clone());

        let id = Uuid::new_v4();
        state
            .notification_store
            .insert(crate::events::sinks::in_app::StoredNotification {
                id,
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "To delete".to_string(),
                body: String::new(),
                data: serde_json::Value::Null,
                read: false,
                created_at: chrono::Utc::now(),
            })
            .await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/notifications/user-A/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Delete again → 404
        let response = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/notifications/user-A/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Preferences tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_preferences_default() {
        let router = test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/notifications/user-A/preferences")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["preferences"]["muted"], false);
        assert!(json["preferences"]["disabled_types"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_update_preferences() {
        let state = test_state();
        let router = notification_routes(state.clone());

        let response = router
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/notifications/user-A/preferences")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "disabled_types": ["new_like"],
                            "muted": false
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!state.preferences_store.is_enabled("user-A", "new_like").await);
        assert!(state.preferences_store.is_enabled("user-A", "new_follower").await);
    }

    #[tokio::test]
    async fn test_mute_unmute() {
        let state = test_state();
        let router = notification_routes(state.clone());

        // Mute
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/notifications/user-A/mute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!state.preferences_store.is_enabled("user-A", "anything").await);

        // Unmute
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/notifications/user-A/unmute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(state.preferences_store.is_enabled("user-A", "anything").await);
    }

    // ── Device token tests ────────────────────────────────────────────

    #[tokio::test]
    async fn test_register_and_list_device_tokens() {
        let state = test_state();
        let router = notification_routes(state.clone());

        // Register
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/device-tokens/user-A")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "token": "ExponentPushToken[xxx]",
                            "platform": "ios"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // List
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/device-tokens/user-A")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        let tokens = json["tokens"].as_array().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0]["token"], "ExponentPushToken[xxx]");
        assert_eq!(tokens[0]["platform"], "ios");
    }

    #[tokio::test]
    async fn test_unregister_device_token() {
        let state = test_state();
        let router = notification_routes(state.clone());

        state
            .device_token_store
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/device-tokens/user-A/token-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Try again → 404
        let response = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/device-tokens/user-A/token-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
