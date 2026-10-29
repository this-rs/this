//! gRPC identity: the caller of a gRPC call, from its **metadata**.
//!
//! The `AuthProvider` is async, so a tonic `Interceptor` (sync) cannot call it. The
//! interception is therefore an axum layer in front of the gRPC services
//! ([`identity_layer`]): it resolves the identity once per HTTP/2 request with
//! `AuthProvider::extract_context_from_metadata` and inserts the `AuthContext` in the
//! request extensions, which tonic hands to every service (`EventService` scoping
//! reads it there). [`context_of`] is the per-call accessor, and falls back to the
//! metadata when no layer ran (a service called directly).
//!
//! Rule on failure, shared with REST (`auth_middleware`): anonymous if
//! `auth.default_policy` is `public`, else `UNAUTHENTICATED`.

use std::sync::Arc;

use axum::extract::{Request as HttpRequest, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tonic::{Request, Status};

use crate::core::auth::AuthContext;
use crate::core::errors::to_status;
use crate::descriptor::framework::codes;
use crate::server::exposure::guard::{SurfaceGuard, identity_failure};
use crate::server::host::ServerHost;

/// Identity of a gRPC call.
pub async fn context_of<T>(host: &ServerHost, request: &Request<T>) -> Result<AuthContext, Status> {
    if let Some(ctx) = request.extensions().get::<AuthContext>() {
        return Ok(ctx.clone());
    }
    let Some(provider) = host.auth_provider.as_ref() else {
        return Ok(AuthContext::Anonymous);
    };
    match provider
        .extract_context_from_metadata(request.metadata())
        .await
    {
        Ok(ctx) => Ok(ctx),
        Err(e) => identity_failure(host, &e).map_err(to_status),
    }
}

/// Guard of a gRPC call: enforced iff `enforce` (the router was built armed) and the
/// host's policies are armed. An unenforced call does not even resolve the identity
/// (historical behavior of the `*_unenforced` routers).
pub async fn guard_of<T>(
    host: &ServerHost,
    request: &Request<T>,
    enforce: bool,
) -> Result<SurfaceGuard, Status> {
    if !enforce || !host.authorize {
        return Ok(SurfaceGuard::unenforced(AuthContext::Anonymous));
    }
    let ctx = context_of(host, request).await?;
    Ok(SurfaceGuard::for_host(host, ctx, "gRPC"))
}

/// axum layer of the armed gRPC routers: resolve the identity from the metadata
/// (HTTP/2 headers) and insert it in the extensions.
pub async fn identity_layer(
    State(host): State<Arc<ServerHost>>,
    mut request: HttpRequest,
    next: Next,
) -> Response {
    if let Some(provider) = host.auth_provider.as_ref() {
        let metadata = tonic::metadata::MetadataMap::from_headers(request.headers().clone());
        let ctx = match provider.extract_context_from_metadata(&metadata).await {
            Ok(ctx) => ctx,
            Err(e) => match identity_failure(&host, &e) {
                Ok(ctx) => ctx,
                Err(err) => {
                    debug_assert_eq!(err.code.as_str(), codes::AUTH_UNAUTHENTICATED);
                    return to_status(err)
                        .into_http::<axum::body::Body>()
                        .into_response();
                }
            },
        };
        request.extensions_mut().insert(ctx);
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::auth::AuthProvider;
    use async_trait::async_trait;
    use axum::http::HeaderMap;
    use uuid::Uuid;

    /// Accepts `Bearer good` (user with role `admin`), refuses anything else.
    pub(crate) struct TokenProvider;

    #[async_trait]
    impl AuthProvider for TokenProvider {
        async fn extract_context(&self, headers: &HeaderMap) -> anyhow::Result<AuthContext> {
            match headers.get("authorization").and_then(|v| v.to_str().ok()) {
                Some("Bearer good") => Ok(AuthContext::User {
                    user_id: Uuid::from_u128(7),
                    tenant_id: Uuid::from_u128(8),
                    roles: vec!["admin".into()],
                }),
                _ => anyhow::bail!("no or bad token"),
            }
        }
        async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn has_role(&self, _: &Uuid, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
    }

    fn host(default_policy: Option<&str>) -> ServerHost {
        let mut h = ServerHost::minimal_for_test().with_auth_provider(Arc::new(TokenProvider));
        if let Some(p) = default_policy {
            let mut cfg = (*h.config).clone();
            cfg.auth = Some(serde_yaml::from_str(&format!("default_policy: {p}")).unwrap());
            h.config = Arc::new(cfg);
        }
        h
    }

    #[tokio::test]
    async fn grpc_identity_from_metadata() {
        let h = host(Some("authenticated"));
        let mut req = Request::new(());
        req.metadata_mut()
            .insert("authorization", "Bearer good".parse().unwrap());
        let ctx = context_of(&h, &req).await.unwrap();
        assert!(matches!(&ctx, AuthContext::User { roles, .. } if roles == &["admin".to_string()]));

        // Bad token, non-public default: UNAUTHENTICATED (as REST's 401).
        let mut bad = Request::new(());
        bad.metadata_mut()
            .insert("authorization", "Bearer forged".parse().unwrap());
        let st = context_of(&h, &bad).await.unwrap_err();
        assert_eq!(st.code(), tonic::Code::Unauthenticated);

        // Public default: anonymous.
        let open = host(None);
        assert!(matches!(
            context_of(&open, &bad).await.unwrap(),
            AuthContext::Anonymous
        ));

        // An identity already set by the layer wins.
        let mut pre = Request::new(());
        pre.extensions_mut().insert(AuthContext::Admin {
            admin_id: Uuid::nil(),
        });
        assert!(context_of(&h, &pre).await.unwrap().is_admin());
    }
}
