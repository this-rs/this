//! Tenant resolver middleware for Axum
//!
//! Extracts the tenant ID from the `AuthContext` (set by the auth middleware)
//! and inserts a `TenantContext` into request extensions. This enables
//! downstream handlers to access the tenant ID without dealing with auth details.
//!
//! ## Resolution order
//!
//! 1. `X-Tenant-Id` header (explicit override, e.g. for admin impersonation)
//! 2. `AuthContext.tenant_id()` from JWT claims
//! 3. `?tenant_id=` query parameter (fallback for dev/testing)
//!
//! If no tenant can be resolved, the request proceeds without a `TenantContext`
//! (public/anonymous access). Handlers that require a tenant should extract
//! `Extension<TenantContext>` and will get a 500 if it's missing, or use
//! `Option<Extension<TenantContext>>` for optional tenant support.
//!
//! ## Auto-wiring
//!
//! Added automatically when `auth.tenant_resolver: true` in config, or when
//! the auth provider is present. Placed AFTER the auth middleware layer.

use crate::core::auth::AuthContext;
use crate::core::tenant::TenantContext;
use axum::{extract::Request, middleware::Next, response::Response};
use uuid::Uuid;

/// Axum middleware function that resolves the tenant context
///
/// This extracts the tenant ID from multiple sources (header, auth context,
/// query param) and inserts a `TenantContext` into request extensions.
pub async fn tenant_resolver_middleware(mut request: Request, next: Next) -> Response {
    let tenant_id = resolve_tenant_id(&request);

    if let Some(tid) = tenant_id {
        request.extensions_mut().insert(TenantContext::new(tid));
    }

    next.run(request).await
}

/// Resolve tenant ID from the request using multiple sources
fn resolve_tenant_id(request: &Request) -> Option<Uuid> {
    // 1. Explicit header override (for admin impersonation or cross-tenant ops)
    if let Some(header_value) = request.headers().get("x-tenant-id") {
        if let Ok(s) = header_value.to_str() {
            if let Ok(tid) = Uuid::parse_str(s) {
                return Some(tid);
            }
        }
    }

    // 2. AuthContext from auth middleware
    if let Some(auth) = request.extensions().get::<AuthContext>() {
        if let Some(tid) = auth.tenant_id() {
            return Some(tid);
        }
    }

    // 3. Query parameter fallback
    if let Some(query) = request.uri().query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("tenant_id=") {
                if let Ok(tid) = Uuid::parse_str(value) {
                    return Some(tid);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use axum::routing::get;
    use axum::{Extension, Router};
    use tower::ServiceExt;

    async fn echo_tenant(tenant: Option<Extension<TenantContext>>) -> String {
        match tenant {
            Some(Extension(ctx)) => format!("tenant:{}", ctx.tenant_id()),
            None => "no-tenant".to_string(),
        }
    }

    fn test_app() -> Router {
        Router::new()
            .route("/test", get(echo_tenant))
            .layer(middleware::from_fn(tenant_resolver_middleware))
    }

    #[tokio::test]
    async fn test_no_tenant_returns_no_tenant() {
        let app = test_app();
        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, "no-tenant");
    }

    #[tokio::test]
    async fn test_tenant_from_header() {
        let app = test_app();
        let tid = Uuid::new_v4();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-tenant-id", tid.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, format!("tenant:{}", tid));
    }

    #[tokio::test]
    async fn test_tenant_from_query_param() {
        let app = test_app();
        let tid = Uuid::new_v4();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/test?tenant_id={}", tid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, format!("tenant:{}", tid));
    }

    #[tokio::test]
    async fn test_tenant_from_auth_context() {
        // Build a router that first injects an AuthContext (simulating auth middleware)
        // then resolves tenant from it
        let tid = Uuid::new_v4();
        let uid = Uuid::new_v4();

        let app = Router::new()
            .route("/test", get(echo_tenant))
            .layer(middleware::from_fn(tenant_resolver_middleware))
            .layer(middleware::from_fn(
                move |mut req: Request<Body>, next: Next| {
                    let auth = AuthContext::User {
                        user_id: uid,
                        tenant_id: tid,
                        roles: vec![],
                    };
                    req.extensions_mut().insert(auth);
                    async move { next.run(req).await }
                },
            ));

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, format!("tenant:{}", tid));
    }

    #[tokio::test]
    async fn test_header_takes_priority_over_query() {
        let app = test_app();
        let header_tid = Uuid::new_v4();
        let query_tid = Uuid::new_v4();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/test?tenant_id={}", query_tid))
                    .header("x-tenant-id", header_tid.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, format!("tenant:{}", header_tid));
    }

    #[tokio::test]
    async fn test_invalid_header_falls_through() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-tenant-id", "not-a-uuid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, "no-tenant");
    }

    #[tokio::test]
    async fn test_anonymous_auth_context_no_tenant() {
        let app = Router::new()
            .route("/test", get(echo_tenant))
            .layer(middleware::from_fn(tenant_resolver_middleware))
            .layer(middleware::from_fn(|mut req: Request<Body>, next: Next| {
                req.extensions_mut().insert(AuthContext::Anonymous);
                async move { next.run(req).await }
            }));

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, "no-tenant");
    }
}
