//! Tenant resolver middleware for Axum
//!
//! Extracts the tenant ID from the `AuthContext` (set by the auth middleware)
//! and inserts a `TenantContext` into request extensions. This enables
//! downstream handlers to access the tenant ID without dealing with auth details.
//!
//! ## Resolution order — the token speaks, the client does not
//!
//! 1. `AuthContext` from the auth middleware: the tenant the credential carries;
//! 2. `X-Tenant-Id` header or `?tenant_id=` query, honoured **only** when it
//!    agrees with the credential, or when the caller is an administrator.
//!
//! ## ⚠️ 2026-10-31 — fixed: the client used to choose its own tenant
//!
//! Until that date the order was the opposite: `x-tenant-id` (then `?tenant_id=`)
//! was read **before** `AuthContext`, and nothing compared the claimed value with
//! the token. An authenticated user of tenant A sending `X-Tenant-Id: <tenant B>`
//! received a `TenantContext` for B — the value downstream handlers filter reads
//! by. The header is also in `allow_headers` (`config/server.rs`), so a browser
//! could send it. The query parameter was worse: it required no identity at all.
//!
//! Now the credential wins. A **contradicting** value is a `this.auth.forbidden`
//! refusal (403), not a preference; a value with no credential behind it is the
//! same refusal. The single exception is `AuthContext::Admin`, which may name a
//! tenant — the original "impersonation" intent, this time actually checked.
//!
//! ## Auto-wiring
//!
//! Added automatically when `auth.tenant_resolver: true` in config, or when
//! the auth provider is present. Placed AFTER the auth middleware layer.

use crate::core::auth::AuthContext;
use crate::core::errors::http::ApiError;
use crate::core::tenant::TenantContext;
use crate::descriptor::ContractError;
use crate::descriptor::framework::codes;
use axum::response::{IntoResponse, Response};
use axum::{extract::Request, middleware::Next};
use uuid::Uuid;

/// What the request asks for, once the credential has had its say.
enum Resolution {
    /// Nobody names a tenant: the request proceeds without a `TenantContext`.
    None,
    /// One tenant, agreed by the credential (or designated by an administrator).
    Tenant(Uuid),
    /// The request names a tenant it has no right to use.
    Forbidden(&'static str),
}

/// Axum middleware function that resolves the tenant context
///
/// This extracts the tenant ID from the credential, then from an explicit
/// designation that the credential allows, and inserts a `TenantContext` into
/// request extensions. A contradicting designation is refused.
pub async fn tenant_resolver_middleware(mut request: Request, next: Next) -> Response {
    // **Un locataire déjà posé est respecté.** Le garde de domaine
    // (`middleware::origin`) pose celui du domaine servi quand la configuration
    // en désigne un : un jeton d'un autre locataire ne doit pas pouvoir changer
    // de locataire par la parole, une fois qu'un domaine a parlé.
    if request.extensions().get::<TenantContext>().is_some() {
        return next.run(request).await;
    }

    match resolve_tenant(&request) {
        Resolution::None => {}
        Resolution::Tenant(tid) => {
            request.extensions_mut().insert(TenantContext::new(tid));
        }
        Resolution::Forbidden(reason) => {
            return ApiError(
                ContractError::framework(codes::AUTH_FORBIDDEN).with_param("reason", reason),
            )
            .into_response();
        }
    }

    next.run(request).await
}

/// The tenant the request *asks* for, by header or query parameter.
///
/// Asking is not getting: `resolve_tenant` decides whether the ask is allowed.
fn requested_tenant(request: &Request) -> Option<Uuid> {
    if let Some(header_value) = request.headers().get("x-tenant-id")
        && let Ok(s) = header_value.to_str()
        && let Ok(tid) = Uuid::parse_str(s.trim())
    {
        return Some(tid);
    }

    if let Some(query) = request.uri().query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("tenant_id=")
                && let Ok(tid) = Uuid::parse_str(value)
            {
                return Some(tid);
            }
        }
    }

    None
}

/// Decide the tenant of a request: the credential first, the client only where
/// the credential allows it.
fn resolve_tenant(request: &Request) -> Resolution {
    let requested = requested_tenant(request);
    let identity = request.extensions().get::<AuthContext>();

    // An administrator carries no tenant of its own but may designate one:
    // this is the "impersonation" the header was originally meant for.
    if identity.is_some_and(|auth| auth.is_admin()) {
        return match requested.or_else(|| identity.and_then(|auth| auth.tenant_id())) {
            Some(tid) => Resolution::Tenant(tid),
            None => Resolution::None,
        };
    }

    let from_token = identity.and_then(|auth| auth.tenant_id());

    match (requested, from_token) {
        (None, None) => Resolution::None,
        (None, Some(tid)) => Resolution::Tenant(tid),
        // Asking for the tenant you already are is not a request at all.
        (Some(tid), Some(same)) if tid == same => Resolution::Tenant(tid),
        // Asking for another tenant with a credential that does not carry it.
        (Some(_), Some(_)) => Resolution::Forbidden("tenant_mismatch"),
        // Asking for a tenant with no credential at all.
        (Some(_), None) => Resolution::Forbidden("tenant_without_identity"),
    }
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

    /// A router that injects a credential before the tenant resolver runs,
    /// mirroring the production layer order.
    fn app_with_identity(auth: AuthContext) -> Router {
        Router::new()
            .route("/test", get(echo_tenant))
            .layer(middleware::from_fn(tenant_resolver_middleware))
            .layer(middleware::from_fn(
                move |mut req: Request<Body>, next: Next| {
                    let auth = auth.clone();
                    async move {
                        req.extensions_mut().insert(auth);
                        next.run(req).await
                    }
                },
            ))
    }

    fn user_of(tenant: Uuid) -> AuthContext {
        AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: tenant,
            roles: vec![],
        }
    }

    #[tokio::test]
    async fn test_tenant_from_header_without_identity_is_refused() {
        // Regression: this used to answer `tenant:<claimed>`. Anyone could name
        // a tenant by setting a header, with no credential at all.
        let app = test_app();
        let claimed = Uuid::new_v4();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-tenant-id", claimed.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("this.auth.forbidden"),
            "the refusal must name its code, got {text}"
        );
        assert!(text.contains("tenant_without_identity"), "got {text}");
    }

    #[tokio::test]
    async fn test_tenant_from_query_param_without_identity_is_refused() {
        // Same hole through the query string, which required nothing at all.
        let app = test_app();
        let claimed = Uuid::new_v4();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/test?tenant_id={claimed}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_header_matching_the_token_is_honoured() {
        let token_tenant = Uuid::new_v4();
        let app = app_with_identity(user_of(token_tenant));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-tenant-id", token_tenant.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, format!("tenant:{token_tenant}"));
    }

    #[tokio::test]
    async fn test_header_contradicting_the_token_is_refused() {
        // The actual attack: a valid user of tenant A asking for tenant B.
        let my_tenant = Uuid::new_v4();
        let other_tenant = Uuid::new_v4();
        let app = app_with_identity(user_of(my_tenant));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-tenant-id", other_tenant.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("tenant_mismatch"), "got {text}");
        assert!(
            !text.contains(&other_tenant.to_string()),
            "the refusal must not echo the tenant that was asked for: {text}"
        );
    }

    #[tokio::test]
    async fn test_admin_may_designate_a_tenant() {
        // The documented intent of the header: verified impersonation.
        let target = Uuid::new_v4();
        let app = app_with_identity(AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("x-tenant-id", target.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, format!("tenant:{target}"));
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
        // Header precedence is kept, but both are now checked against the
        // credential: here the header agrees with it and the query does not, so
        // the header wins and the query is never consulted.
        let token_tenant = Uuid::new_v4();
        let query_tid = Uuid::new_v4();
        let app = app_with_identity(user_of(token_tenant));

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/test?tenant_id={query_tid}"))
                    .header("x-tenant-id", token_tenant.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, format!("tenant:{token_tenant}"));
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
