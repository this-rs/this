//! STS authentication endpoints
//!
//! - POST /auth/token  -- Login with credentials, get JWT + refresh token
//! - GET  /auth/keys   -- JWKS public key distribution
//! - POST /auth/refresh -- Refresh access token
//! - POST /auth/revoke  -- Revoke a token by JTI

use crate::core::auth::sts::{LoginCredentials, RefreshRequest, RevokeRequest, StsService};
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing};
use std::sync::Arc;

/// Shared state for the STS auth routes
pub struct StsState {
    pub sts: Arc<StsService>,
    pub public_key_pem: String,
    /// Decoding key for refresh-token verification
    pub decoding_key: jsonwebtoken::DecodingKey,
}

/// POST /auth/token
pub async fn login_handler(
    State(state): State<Arc<StsState>>,
    Json(credentials): Json<LoginCredentials>,
) -> impl IntoResponse {
    match state.sts.login(&credentials) {
        Ok(pair) => (StatusCode::OK, Json(serde_json::to_value(pair).unwrap())).into_response(),
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_credentials"})),
        )
            .into_response(),
    }
}

/// GET /auth/keys
pub async fn jwks_handler(State(state): State<Arc<StsState>>) -> impl IntoResponse {
    match StsService::jwks_response(&state.public_key_pem) {
        Ok(jwks) => (StatusCode::OK, Json(jwks)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "jwks_generation_failed"})),
        )
            .into_response(),
    }
}

/// POST /auth/refresh
pub async fn refresh_handler(
    State(state): State<Arc<StsState>>,
    Json(body): Json<RefreshRequest>,
) -> impl IntoResponse {
    match state.sts.refresh(&body.refresh_token, &state.decoding_key) {
        Ok(pair) => (StatusCode::OK, Json(serde_json::to_value(pair).unwrap())).into_response(),
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_refresh_token"})),
        )
            .into_response(),
    }
}

/// POST /auth/revoke
pub async fn revoke_handler(
    State(state): State<Arc<StsState>>,
    Json(body): Json<RevokeRequest>,
) -> impl IntoResponse {
    state.sts.revoke(&body.jti);
    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "revoked"})),
    )
}

/// Build the auth router with all STS endpoints
pub fn auth_router(sts_state: Arc<StsState>) -> Router {
    Router::new()
        .route("/auth/token", routing::post(login_handler))
        .route("/auth/keys", routing::get(jwks_handler))
        .route("/auth/refresh", routing::post(refresh_handler))
        .route("/auth/revoke", routing::post(revoke_handler))
        .with_state(sts_state)
}
