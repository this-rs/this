//! Authentication middleware for Axum
//!
//! Extracts Bearer tokens from incoming requests, verifies them via the
//! configured `AuthProvider`, and injects an `AuthContext` into the
//! request extensions.
//!
//! ## Auto-wiring
//!
//! The middleware is automatically added when `auth.provider != "none"` in config.
//! Routes with `AuthPolicy::Public` are still accessible without a token.
//!
//! ## Usage
//!
//! The `AuthContext` can be extracted in handlers via:
//! ```ignore
//! use axum::Extension;
//! use this::core::auth::AuthContext;
//!
//! async fn my_handler(Extension(auth): Extension<AuthContext>) -> impl IntoResponse {
//!     if auth.is_admin() { /* ... */ }
//! }
//! ```

use crate::core::auth::{AuthContext, AuthProvider};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// Le nom du cookie que la structure accepte — et que `POST /auth/token` pose.
///
/// Un nom, un seul, partagé par la porte (`/auth/verify`) et par cette couche :
/// deux noms voudraient dire deux comportements.
pub const COOKIE_JETON: &str = "this_token";

/// Le jeton porté par une requête : **l'en-tête d'abord, le cookie ensuite**.
///
/// L'en-tête gagne toujours : une requête qui dit explicitement qui elle est
/// n'est pas réécrite par un cookie — sinon un cookie périmé pourrait
/// contredire un jeton frais.
pub fn jeton_de_la_requete(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        && let Some(token) = value.strip_prefix("Bearer ")
        && !token.trim().is_empty()
    {
        return Some(token.trim().to_string());
    }

    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| {
            let (name, value) = cookie.split_once('=')?;
            (name.trim() == COOKIE_JETON && !value.trim().is_empty())
                .then(|| value.trim().to_string())
        })
}

/// **Traduit le cookie en en-tête** avant la couche d'authentification.
///
/// Un navigateur ne peut pas poser `Authorization: Bearer …` sur une
/// navigation. Sans cette traduction, toute surface rattachée est
/// inatteignable depuis une page — et il ne reste que deux issues : rendre le
/// projet **public**, ou **réécrire son authentification**. Aucune des deux
/// n'est acceptable pour rattacher un projet qui existe déjà.
///
/// Elle **ne vérifie rien** : elle déplace le jeton du cookie vers l'en-tête,
/// et la vérification reste celle de l'[`AuthProvider`] — une seule
/// implémentation de la confiance, jamais deux. Le cookie est donc gardé comme
/// l'en-tête : signature, échéance, émetteur et révocation sont vérifiés
/// exactement pareil.
///
/// À monter **le plus à l'extérieur** (dernier `.layer()` appliqué), pour
/// qu'elle s'exécute avant l'authentification.
pub async fn cookie_vers_en_tete(mut request: Request, next: Next) -> Response {
    if !request.headers().contains_key(header::AUTHORIZATION)
        && let Some(token) = jeton_de_la_requete(request.headers())
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
    {
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    next.run(request).await
}

/// Auth middleware layer that wraps an AuthProvider
#[derive(Clone)]
pub struct AuthLayer {
    provider: Arc<dyn AuthProvider>,
    /// The default policy string from config (e.g., "public", "authenticated")
    default_policy: String,
}

impl AuthLayer {
    /// Create a new auth layer with the given provider
    pub fn new(provider: Arc<dyn AuthProvider>, default_policy: String) -> Self {
        Self {
            provider,
            default_policy,
        }
    }
}

/// Axum middleware function that performs authentication
///
/// This function:
/// 1. Calls `AuthProvider::extract_context()` on the request
/// 2. On success, inserts `AuthContext` into request extensions
/// 3. On failure, returns 401 Unauthorized (unless default policy is "public")
///
/// Policy enforcement (checking if the AuthContext satisfies the endpoint's policy)
/// is done at the handler level, not here. This middleware only handles identity extraction.
pub async fn auth_middleware(
    State(auth_layer): State<AuthLayer>,
    mut request: Request,
    next: Next,
) -> Response {
    match auth_layer.provider.extract_context(request.headers()).await {
        Ok(context) => {
            // Insert the auth context into extensions for handlers to use
            request.extensions_mut().insert(context);
            next.run(request).await
        }
        Err(err) => {
            // If default policy is public, allow anonymous access
            if auth_layer.default_policy == "public" {
                request.extensions_mut().insert(AuthContext::Anonymous);
                next.run(request).await
            } else {
                tracing::debug!("Auth failed: {}", err);
                let mut res = crate::core::errors::error_response_with(
                    crate::core::errors::unauthenticated(),
                    Some(StatusCode::UNAUTHORIZED),
                    serde_json::Map::new(),
                );
                res.headers_mut()
                    .insert("WWW-Authenticate", HeaderValue::from_static("Bearer"));
                res
            }
        }
    }
}

/// Create an Axum middleware layer from an AuthProvider
///
/// Returns a closure suitable for `axum::middleware::from_fn_with_state`.
///
/// # Example
///
/// ```ignore
/// use axum::middleware;
///
/// let auth_layer = AuthLayer::new(provider, "authenticated".to_string());
/// let app = Router::new()
///     .route("/api/orders", get(list_orders))
///     .layer(middleware::from_fn_with_state(auth_layer, auth_middleware));
/// ```
pub fn build_auth_layer(provider: Arc<dyn AuthProvider>, default_policy: String) -> AuthLayer {
    AuthLayer::new(provider, default_policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::auth::{AuthContext, NoAuthProvider};
    use axum::Extension;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn echo_auth(Extension(auth): Extension<AuthContext>) -> String {
        match auth {
            AuthContext::Anonymous => "anonymous".to_string(),
            AuthContext::User { user_id, .. } => format!("user:{}", user_id),
            AuthContext::Admin { admin_id } => format!("admin:{}", admin_id),
            AuthContext::Service { service_name, .. } => format!("service:{}", service_name),
            AuthContext::Owner { user_id, .. } => format!("owner:{}", user_id),
        }
    }

    fn test_app(provider: Arc<dyn AuthProvider>, default_policy: &str) -> Router {
        let auth_layer = AuthLayer::new(provider, default_policy.to_string());
        Router::new()
            .route("/test", get(echo_auth))
            .layer(middleware::from_fn_with_state(auth_layer, auth_middleware))
    }

    #[tokio::test]
    async fn test_no_auth_provider_returns_anonymous() {
        let provider: Arc<dyn AuthProvider> = Arc::new(NoAuthProvider);
        let app = test_app(provider, "public");

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, "anonymous");
    }

    #[tokio::test]
    async fn test_no_token_public_policy_allows_anonymous() {
        let provider: Arc<dyn AuthProvider> = Arc::new(NoAuthProvider);
        let app = test_app(provider, "public");

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_injects_auth_context() {
        // MockAuthProvider that always returns a specific user
        struct AlwaysUserProvider;

        #[async_trait::async_trait]
        impl AuthProvider for AlwaysUserProvider {
            async fn extract_context(
                &self,
                _headers: &axum::http::HeaderMap,
            ) -> anyhow::Result<AuthContext> {
                Ok(AuthContext::User {
                    user_id: uuid::Uuid::nil(),
                    tenant_id: uuid::Uuid::nil(),
                    roles: vec!["admin".to_string()],
                    scopes: Vec::new(),
                })
            }

            async fn is_owner(
                &self,
                _: &uuid::Uuid,
                _: &uuid::Uuid,
                _: &str,
            ) -> anyhow::Result<bool> {
                Ok(false)
            }

            async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
                Ok(true)
            }
        }

        let provider: Arc<dyn AuthProvider> = Arc::new(AlwaysUserProvider);
        let app = test_app(provider, "authenticated");

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, "user:00000000-0000-0000-0000-000000000000");
    }

    #[tokio::test]
    async fn test_failed_auth_returns_401_when_not_public() {
        // Provider that always fails
        struct AlwaysFailProvider;

        #[async_trait::async_trait]
        impl AuthProvider for AlwaysFailProvider {
            async fn extract_context(
                &self,
                _headers: &axum::http::HeaderMap,
            ) -> anyhow::Result<AuthContext> {
                anyhow::bail!("No valid token")
            }

            async fn is_owner(
                &self,
                _: &uuid::Uuid,
                _: &uuid::Uuid,
                _: &str,
            ) -> anyhow::Result<bool> {
                Ok(false)
            }

            async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
                Ok(false)
            }
        }

        let provider: Arc<dyn AuthProvider> = Arc::new(AlwaysFailProvider);
        let app = test_app(provider, "authenticated");

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_failed_auth_allows_anonymous_when_public() {
        struct AlwaysFailProvider;

        #[async_trait::async_trait]
        impl AuthProvider for AlwaysFailProvider {
            async fn extract_context(
                &self,
                _headers: &axum::http::HeaderMap,
            ) -> anyhow::Result<AuthContext> {
                anyhow::bail!("No valid token")
            }

            async fn is_owner(
                &self,
                _: &uuid::Uuid,
                _: &uuid::Uuid,
                _: &str,
            ) -> anyhow::Result<bool> {
                Ok(false)
            }

            async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
                Ok(false)
            }
        }

        let provider: Arc<dyn AuthProvider> = Arc::new(AlwaysFailProvider);
        let app = test_app(provider, "public");

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, "anonymous");
    }
}
