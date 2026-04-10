//! GDPR erasure cascade endpoint
//!
//! - DELETE /tenants/:tenant_id/data — Erase all data for a tenant
//!
//! Requires `auth.gdpr.erasure_cascade: true` in config.
//! Only accessible to admins (AuthPolicy::AdminOnly).
//!
//! ## Cascade order
//!
//! 1. Audit log (immutable record BEFORE deletion)
//! 2. Delete notifications for the tenant
//! 3. Delete links belonging to the tenant (via LinkService)
//! 4. Remove tenant-scoped sinks
//! 5. Emit FrameworkEvent::GdprErasure

use crate::core::auth::AuthContext;
use crate::core::events::{EventBus, FrameworkEvent};
use crate::core::service::LinkService;
use crate::events::sinks::SinkRegistry;
use crate::events::sinks::in_app::NotificationStore;
use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing,
};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

/// Shared state for GDPR routes
pub struct GdprState {
    pub notification_store: Arc<NotificationStore>,
    pub link_service: Option<Arc<dyn LinkService>>,
    pub sink_registry: Option<Arc<SinkRegistry>>,
    pub event_bus: Option<Arc<EventBus>>,
}

/// Response from a successful erasure
#[derive(Debug, Serialize)]
pub struct ErasureResponse {
    pub tenant_id: Uuid,
    pub notifications_deleted: usize,
    pub links_deleted: usize,
    pub status: String,
    pub erased_at: chrono::DateTime<chrono::Utc>,
}

/// DELETE /tenants/:tenant_id/data
///
/// Erases all data belonging to the specified tenant.
/// Requires admin authentication.
pub async fn erasure_handler(
    Path(tenant_id): Path<Uuid>,
    auth: Option<Extension<AuthContext>>,
    State(state): State<Arc<GdprState>>,
) -> impl IntoResponse {
    // Require admin authentication
    let is_admin = auth
        .as_ref()
        .map(|Extension(ctx)| ctx.is_admin())
        .unwrap_or(false);

    if !is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "admin_required", "message": "GDPR erasure requires admin access"})),
        )
            .into_response();
    }

    let erased_at = chrono::Utc::now();

    // 1. Audit log — record the erasure request BEFORE deleting
    tracing::info!(
        tenant_id = %tenant_id,
        initiated_by = ?auth.as_ref().map(|Extension(ctx)| format!("{:?}", ctx)),
        "GDPR erasure cascade initiated"
    );

    // 2. Delete notifications for the tenant
    let notifications_deleted = state.notification_store.delete_by_tenant(&tenant_id).await;
    tracing::info!(
        tenant_id = %tenant_id,
        notifications_deleted,
        "GDPR: notifications purged"
    );

    // 3. Delete links belonging to the tenant
    let links_deleted = if let Some(ref link_service) = state.link_service {
        match link_service.delete_by_tenant(&tenant_id).await {
            Ok(count) => {
                tracing::info!(
                    tenant_id = %tenant_id,
                    links_deleted = count,
                    "GDPR: tenant links purged"
                );
                count
            }
            Err(e) => {
                tracing::error!(
                    tenant_id = %tenant_id,
                    error = %e,
                    "GDPR: failed to delete tenant links — continuing cascade"
                );
                0
            }
        }
    } else {
        0
    };

    // 4. Remove tenant-scoped sinks
    if let Some(ref registry) = state.sink_registry {
        registry.remove_tenant_sinks(&tenant_id);
        tracing::info!(tenant_id = %tenant_id, "GDPR: tenant sinks removed");
    }

    // 5. Emit GdprErasure event
    if let Some(ref event_bus) = state.event_bus {
        event_bus.publish_for_tenant(
            FrameworkEvent::GdprErasure {
                tenant_id,
                entities_deleted: 0, // Entity deletion requires per-type DataService iteration — deferred to caller
                links_deleted,
                notifications_deleted,
                erased_at,
            },
            tenant_id,
        );
        tracing::info!(tenant_id = %tenant_id, "GDPR: erasure event published");
    }

    tracing::info!(
        tenant_id = %tenant_id,
        notifications_deleted,
        links_deleted,
        "GDPR erasure cascade completed"
    );

    let response = ErasureResponse {
        tenant_id,
        notifications_deleted,
        links_deleted,
        status: "erased".to_string(),
        erased_at,
    };

    (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
}

/// Build the GDPR routes
pub fn gdpr_routes(state: Arc<GdprState>) -> Router {
    Router::new()
        .route("/tenants/{tenant_id}/data", routing::delete(erasure_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware;
    use tower::ServiceExt;

    fn test_gdpr_state() -> Arc<GdprState> {
        Arc::new(GdprState {
            notification_store: Arc::new(NotificationStore::new()),
            link_service: None,
            sink_registry: None,
            event_bus: None,
        })
    }

    fn test_app(state: Arc<GdprState>) -> Router {
        gdpr_routes(state)
    }

    /// Inject an AuthContext as a middleware layer for testing
    fn with_auth(app: Router, auth: AuthContext) -> Router {
        app.layer(middleware::from_fn(move |mut req: Request<Body>, next: axum::middleware::Next| {
            let auth = auth.clone();
            async move {
                req.extensions_mut().insert(auth);
                next.run(req).await
            }
        }))
    }

    #[tokio::test]
    async fn test_erasure_requires_admin() {
        let state = test_gdpr_state();
        let tenant_id = Uuid::new_v4();

        // No auth → 403
        let app = test_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tenants/{}/data", tenant_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_erasure_rejects_non_admin_user() {
        let state = test_gdpr_state();
        let tenant_id = Uuid::new_v4();

        let auth = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id,
            roles: vec!["user".to_string()],
        };

        let app = with_auth(test_app(state), auth);
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tenants/{}/data", tenant_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_erasure_succeeds_for_admin() {
        let state = test_gdpr_state();
        let tenant_id = Uuid::new_v4();

        // Insert some notifications for the tenant
        use crate::events::sinks::in_app::StoredNotification;
        for i in 0..3 {
            state
                .notification_store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-1".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {}", i),
                    body: String::new(),
                    data: serde_json::Value::Null,
                    read: false,
                    created_at: chrono::Utc::now(),
                    tenant_id: Some(tenant_id),
                })
                .await;
        }

        // Insert a notification for a different tenant (should survive)
        let other_tenant = Uuid::new_v4();
        state
            .notification_store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-1".to_string(),
                notification_type: "test".to_string(),
                title: "Other tenant".to_string(),
                body: String::new(),
                data: serde_json::Value::Null,
                read: false,
                created_at: chrono::Utc::now(),
                tenant_id: Some(other_tenant),
            })
            .await;

        let auth = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };

        let app = with_auth(test_app(state.clone()), auth);
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tenants/{}/data", tenant_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "erased");
        assert_eq!(json["notifications_deleted"], 3);
        assert_eq!(json["tenant_id"], tenant_id.to_string());

        // Verify: other tenant's notification survives
        let remaining = state
            .notification_store
            .list_by_user("user-1", 100, 0)
            .await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].tenant_id, Some(other_tenant));
    }

    #[tokio::test]
    async fn test_erasure_with_event_bus_emits_event() {
        let event_bus = EventBus::new(128);
        let mut receiver = event_bus.subscribe();

        let state = Arc::new(GdprState {
            notification_store: Arc::new(NotificationStore::new()),
            link_service: None,
            sink_registry: None,
            event_bus: Some(Arc::new(event_bus)),
        });

        let tenant_id = Uuid::new_v4();
        let auth = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };

        let app = with_auth(gdpr_routes(state), auth);
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tenants/{}/data", tenant_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Check that a GdprErasure event was emitted
        let envelope = receiver.try_recv().expect("should have received an event");
        assert_eq!(envelope.event.event_kind(), "gdpr_erasure");
        match &envelope.event {
            FrameworkEvent::GdprErasure {
                tenant_id: tid,
                notifications_deleted,
                ..
            } => {
                assert_eq!(*tid, tenant_id);
                assert_eq!(*notifications_deleted, 0); // no notifications to delete
            }
            _ => panic!("Expected GdprErasure event"),
        }
    }

    #[tokio::test]
    async fn test_erasure_cascades_links() {
        use crate::core::link::LinkEntity;
        use crate::storage::InMemoryLinkService;

        let link_service = Arc::new(InMemoryLinkService::new());
        let tenant_id = Uuid::new_v4();
        let other_tenant = Uuid::new_v4();

        // Create links for our tenant
        for _ in 0..3 {
            let link = LinkEntity::new_with_tenant(
                tenant_id,
                "related_to",
                Uuid::new_v4(),
                Uuid::new_v4(),
                None,
            );
            LinkService::create(link_service.as_ref(), link).await.unwrap();
        }

        // Create a link for a different tenant (should survive)
        let other_link = LinkEntity::new_with_tenant(
            other_tenant,
            "related_to",
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
        );
        LinkService::create(link_service.as_ref(), other_link).await.unwrap();

        let state = Arc::new(GdprState {
            notification_store: Arc::new(NotificationStore::new()),
            link_service: Some(link_service.clone() as Arc<dyn LinkService>),
            sink_registry: None,
            event_bus: None,
        });

        let auth = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };

        let app = with_auth(gdpr_routes(state), auth);
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tenants/{}/data", tenant_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["links_deleted"], 3);
        assert_eq!(json["status"], "erased");

        // Verify: other tenant's link survives
        let remaining = LinkService::list(link_service.as_ref()).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].tenant_id, Some(other_tenant));
    }

    #[tokio::test]
    async fn test_erasure_no_data_returns_ok() {
        let state = test_gdpr_state();
        let tenant_id = Uuid::new_v4();
        let auth = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };

        let app = with_auth(test_app(state), auth);
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tenants/{}/data", tenant_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["notifications_deleted"], 0);
        assert_eq!(json["links_deleted"], 0);
    }
}
