//! Admin interface of the module runtime (REST), protected by its own policy.
//!
//! | Method | Path | Effect |
//! |---|---|---|
//! | GET | `{prefix}` | status of every module (active, draining, unloaded, failed, offered) |
//! | GET | `{prefix}/{name}` | status of one module |
//! | POST | `{prefix}` | load: `{"module": "<offered name>"}`, `{"manifest": {…descriptor…}}` or `{"store": {"name": "…", "version": "…"}}` |
//! | DELETE | `{prefix}/{name}` | unload (refused with active dependents) |
//!
//! The interface is a layer mounted OUTSIDE the legacy `Authorization` layer (which
//! would refuse its paths by abstention) and INSIDE authentication: it reads the
//! caller's `AuthContext` and evaluates `RuntimeOptions::admin_policy` (default
//! `admin_only`) with the descriptor evaluator. Unlike entity routes, this check does
//! NOT depend on `authorize`: loading code into a live host is never open by default.

use super::dispatch::check_policy;
use super::loader::ModuleSource;
use super::runtime::ModuleRuntime;
use crate::core::auth::AuthContext;
use crate::core::errors::ApiError;
use crate::descriptor::ContractError;
use crate::descriptor::framework::codes;
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadRequest {
    module: Option<String>,
    manifest: Option<Value>,
    store: Option<StoreRef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreRef {
    name: String,
    version: Option<String>,
}

fn bad(reason: impl Into<String>) -> ContractError {
    ContractError::framework(codes::TRANSPORT_BAD_REQUEST).with_param("reason", reason.into())
}

/// The admin layer function (`axum::middleware::from_fn_with_state`).
pub async fn admin_middleware(
    State(rt): State<Arc<ModuleRuntime>>,
    request: Request,
    next: Next,
) -> Response {
    let prefix = rt.options.admin_prefix.trim_end_matches('/').to_string();
    let path = request.uri().path().to_string();
    let rest = match path.strip_prefix(&prefix) {
        Some(r) if r.is_empty() || r.starts_with('/') => r.trim_matches('/').to_string(),
        _ => return next.run(request).await,
    };

    let ctx = request
        .extensions()
        .get::<AuthContext>()
        .cloned()
        .unwrap_or(AuthContext::Anonymous);
    if let Err(e) = check_policy(
        &rt.options.admin_policy,
        &ctx,
        &rt.parts().resolvers,
        None,
        "this.module.admin",
    ) {
        return ApiError(e).into_response();
    }

    let method = request.method().clone();
    let result = match (method, rest.as_str()) {
        (Method::GET, "") => Ok((StatusCode::OK, Json(serde_json::json!(rt.list().await)))),
        (Method::GET, name) if !name.contains('/') => rt
            .status(name)
            .await
            .map(|s| (StatusCode::OK, Json(serde_json::json!(s)))),
        (Method::DELETE, name) if !name.is_empty() && !name.contains('/') => rt
            .unload(name)
            .await
            .map(|s| (StatusCode::OK, Json(serde_json::json!(s)))),
        (Method::POST, "") => load(&rt, request)
            .await
            .map(|s| (StatusCode::CREATED, Json(serde_json::json!(s)))),
        _ => Err(ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", rest)),
    };
    match result {
        Ok(r) => r.into_response(),
        Err(e) => ApiError(e).into_response(),
    }
}

async fn load(
    rt: &Arc<ModuleRuntime>,
    request: Request,
) -> Result<super::loader::ModuleStatus, ContractError> {
    let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| bad(e.to_string()))?;
    let body: LoadRequest = serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
    match (body.module, body.manifest, body.store) {
        (Some(name), None, None) => rt.load_offered(&name).await,
        (None, Some(manifest), None) => rt.load(ModuleSource::Manifest(manifest.to_string())).await,
        (None, None, Some(s)) => {
            let store = rt
                .store()
                .cloned()
                .ok_or_else(|| bad("no ModuleStore attached to the runtime"))?;
            rt.load(ModuleSource::Stored {
                store,
                name: s.name,
                version: s.version,
            })
            .await
        }
        _ => Err(bad("exactly one of `module`, `manifest`, `store` is expected")),
    }
}
