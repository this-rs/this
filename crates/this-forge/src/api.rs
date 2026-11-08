//! Les routes HTTP de la forge — exactement le §10 du contrat (docs/modules-wasm-abi.md).
//!
//! Format d'erreur unique : `{"error":{"code":"…","message":"…"}}`, avec un `code` stable
//! (un agent branche dessus) et un `message` en français lisible.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{
    hash_password, issue_token, new_refresh_token, now_secs, verify_password, verify_token,
};
use crate::entities::{Account, Build, Session};
use crate::runner::{Diagnostic, SourceFile, valid_module_name, valid_version, validate_files};
use crate::service::Forge;
use crate::sign::{sha256_hex, source_digest};

/// Une erreur d'API : statut HTTP, code stable, message.
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!("erreur interne : {e}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "erreur interne de la forge",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": {"code": self.code, "message": self.message}})),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(format!("{e:#}"))
    }
}

type Res<T> = Result<T, ApiError>;
type St = State<Arc<Forge>>;

fn unauthorized(msg: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", msg)
}

/// Le compte authentifié par `Authorization: Bearer <jwt>`.
pub struct Authed(pub Account);

impl FromRequestParts<Arc<Forge>> for Authed {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, forge: &Arc<Forge>) -> Res<Self> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| unauthorized("jeton manquant : Authorization: Bearer <jwt>"))?;
        let sub = verify_token(&forge.keys, token)
            .ok_or_else(|| unauthorized("jeton invalide ou expiré"))?;
        let id = Uuid::parse_str(&sub).map_err(|_| unauthorized("jeton invalide"))?;
        let acct = forge
            .store
            .accounts
            .get(&id)
            .await?
            .ok_or_else(|| unauthorized("compte inconnu"))?;
        if acct.status != "active" {
            return Err(unauthorized("compte désactivé"));
        }
        Ok(Authed(acct))
    }
}

/// Le routeur de la forge, monté sous `/v1`.
pub fn router(forge: Arc<Forge>) -> Router {
    let v1 = Router::new()
        .route("/auth/signup", post(signup))
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/forge/key", get(forge_key))
        .route("/builds", post(create_build))
        .route("/builds/{id}", get(get_build))
        .route("/builds/{id}/events", get(build_events))
        .route("/builds/{id}/artifact", get(artifact));
    Router::new()
        .nest("/v1", v1)
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(forge)
}

// ── auth ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SignupBody {
    invitation: String,
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct LoginBody {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct RefreshBody {
    refresh: String,
}

fn normalize_email(e: &str) -> Res<String> {
    let e = e.trim().to_lowercase();
    let ok = e.len() <= 254
        && e.matches('@').count() == 1
        && !e.starts_with('@')
        && !e.ends_with('@')
        && !e.contains(char::is_whitespace);
    if ok {
        Ok(e)
    } else {
        Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_email",
            "adresse e-mail invalide",
        ))
    }
}

async fn new_session(forge: &Forge, acct: &Account) -> Res<(String, String)> {
    let token = issue_token(
        &forge.keys,
        &acct.id.to_string(),
        forge.cfg.token_ttl.as_secs(),
    )?;
    let refresh = new_refresh_token();
    let s = Session::new(
        sha256_hex(refresh.as_bytes()),
        "active".into(),
        acct.id.to_string(),
        (now_secs() + forge.cfg.refresh_ttl.as_secs()) as i64,
    );
    forge.store.sessions.create(s).await?;
    Ok((token, refresh))
}

fn account_json(a: &Account) -> Value {
    json!({"id": a.id, "email": a.name})
}

async fn signup(State(forge): St, Json(b): Json<SignupBody>) -> Res<Json<Value>> {
    let email = normalize_email(&b.email)?;
    if b.password.chars().count() < 10 || b.password.len() > 256 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "weak_password",
            "le mot de passe doit faire au moins 10 caractères (256 au plus)",
        ));
    }
    let _guard = forge.signup_lock.lock().await;
    let code_hash = sha256_hex(b.invitation.trim().as_bytes());
    let inv = forge
        .store
        .invitations
        .list()
        .await?
        .into_iter()
        .find(|i| i.name == code_hash && i.uses_left > 0);
    let Some(mut inv) = inv else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "invalid_invitation",
            "code d'invitation inconnu ou déjà utilisé : demandez-en un nouveau",
        ));
    };
    if forge
        .store
        .accounts
        .list()
        .await?
        .iter()
        .any(|a| a.name == email)
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "email_taken",
            "un compte existe déjà avec cette adresse",
        ));
    }
    let pw = b.password.clone();
    let phc = tokio::task::spawn_blocking(move || hash_password(&pw))
        .await
        .map_err(ApiError::internal)??;
    let acct = forge
        .store
        .accounts
        .create(Account::new(email, "active".into(), phc))
        .await?;
    inv.uses_left -= 1;
    inv.updated_at = Utc::now();
    forge.store.invitations.update(&inv.id.clone(), inv).await?;
    let (token, refresh) = new_session(&forge, &acct).await?;
    Ok(Json(
        json!({"token": token, "refresh": refresh, "account": account_json(&acct)}),
    ))
}

async fn login(State(forge): St, Json(b): Json<LoginBody>) -> Res<Json<Value>> {
    let email = b.email.trim().to_lowercase();
    let acct = forge
        .store
        .accounts
        .list()
        .await?
        .into_iter()
        .find(|a| a.name == email);
    let phc = acct
        .as_ref()
        .map(|a| a.password_hash.clone())
        .unwrap_or_else(|| forge.dummy_hash.clone());
    let pw = b.password;
    let ok = tokio::task::spawn_blocking(move || verify_password(&pw, &phc))
        .await
        .map_err(ApiError::internal)?;
    match acct {
        Some(a) if ok && a.status == "active" => {
            let (token, refresh) = new_session(&forge, &a).await?;
            Ok(Json(
                json!({"token": token, "refresh": refresh, "account": account_json(&a)}),
            ))
        }
        _ => Err(unauthorized("e-mail ou mot de passe incorrect")),
    }
}

async fn refresh(State(forge): St, Json(b): Json<RefreshBody>) -> Res<Json<Value>> {
    let h = sha256_hex(b.refresh.as_bytes());
    let sess = forge
        .store
        .sessions
        .list()
        .await?
        .into_iter()
        .find(|s| s.name == h);
    let Some(sess) = sess else {
        return Err(unauthorized("jeton de rafraîchissement inconnu"));
    };
    // Rotation : un jeton de rafraîchissement ne sert qu'une fois.
    forge.store.sessions.delete(&sess.id).await?;
    if sess.expires_at < now_secs() as i64 {
        return Err(unauthorized(
            "jeton de rafraîchissement expiré : reconnectez-vous",
        ));
    }
    let id = Uuid::parse_str(&sess.account_id).map_err(ApiError::internal)?;
    let acct = forge
        .store
        .accounts
        .get(&id)
        .await?
        .filter(|a| a.status == "active")
        .ok_or_else(|| unauthorized("compte inconnu"))?;
    let (token, refresh) = new_session(&forge, &acct).await?;
    Ok(Json(json!({"token": token, "refresh": refresh})))
}

/// Révoque tous les jetons de rafraîchissement du compte. Le jeton d'accès, lui, reste
/// valable jusqu'à son expiration (15 min par défaut) : il est sans état.
async fn logout(State(forge): St, Authed(acct): Authed) -> Res<StatusCode> {
    let mine = acct.id.to_string();
    for s in forge
        .store
        .sessions
        .list()
        .await?
        .into_iter()
        .filter(|s| s.account_id == mine)
    {
        forge.store.sessions.delete(&s.id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── compte et quotas ────────────────────────────────────────────────────────

struct Usage {
    builds_today: u32,
    cpu_used_secs: u64,
    pending: u32,
}

async fn usage(forge: &Forge, acct: &Account) -> Res<Usage> {
    let day = Utc::now().date_naive();
    let mine = acct.id.to_string();
    let mut u = Usage {
        builds_today: 0,
        cpu_used_secs: 0,
        pending: 0,
    };
    for b in forge
        .store
        .builds
        .list()
        .await?
        .into_iter()
        .filter(|b| b.account_id == mine)
    {
        if matches!(b.status.as_str(), "queued" | "building") {
            u.pending += 1;
        }
        if b.created_at.date_naive() == day {
            u.builds_today += 1;
            u.cpu_used_secs += (b.duration_ms.max(0) as u64).div_ceil(1000);
        }
    }
    Ok(u)
}

async fn me(State(forge): St, Authed(acct): Authed) -> Res<Json<Value>> {
    let u = usage(&forge, &acct).await?;
    Ok(Json(json!({
        "account": account_json(&acct),
        "quota": {
            "builds_today": u.builds_today,
            "builds_limit": forge.cfg.builds_per_day,
            "cpu_seconds_left": forge.cfg.cpu_seconds_per_day.saturating_sub(u.cpu_used_secs),
        }
    })))
}

async fn forge_key(State(forge): St) -> Json<Value> {
    Json(json!({
        "algorithm": "ed25519",
        "public_key": forge.keys.public_hex(),
        "fingerprint": forge.keys.fingerprint(),
    }))
}

// ── builds ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BuildBody {
    name: String,
    version: String,
    files: Vec<SourceFile>,
    #[serde(default)]
    toolchain: Option<String>,
}

async fn create_build(
    State(forge): St,
    Authed(acct): Authed,
    Json(b): Json<BuildBody>,
) -> Res<(StatusCode, Json<Value>)> {
    let bad = |code: &'static str, m: String| ApiError::new(StatusCode::BAD_REQUEST, code, m);
    if !valid_module_name(&b.name) {
        return Err(bad("invalid_name", "nom invalide : minuscules, chiffres, `-` et `_`, 64 caractères au plus, commence par une lettre".into()));
    }
    if !valid_version(&b.version) {
        return Err(bad(
            "invalid_version",
            "version invalide : MAJOR.MINOR.PATCH attendu".into(),
        ));
    }
    if let Some(t) = &b.toolchain {
        let tc = forge.runner.toolchain().await;
        let want = tc.strip_prefix("rustc ").unwrap_or(&tc).to_string();
        if t != "stable" && *t != want {
            return Err(bad(
                "unsupported_toolchain",
                format!(
                    "chaîne d'outils `{t}` non offerte : cette forge compile avec rustc {want}"
                ),
            ));
        }
    }
    validate_files(&b.files, &forge.cfg).map_err(|(c, m)| bad(c, m))?;

    let u = usage(&forge, &acct).await?;
    let too_many =
        |code: &'static str, m: String| ApiError::new(StatusCode::TOO_MANY_REQUESTS, code, m);
    if u.builds_today >= forge.cfg.builds_per_day {
        return Err(too_many(
            "quota_builds_exceeded",
            format!(
                "quota atteint : {} builds par jour (UTC). Réessayez demain",
                forge.cfg.builds_per_day
            ),
        ));
    }
    if u.cpu_used_secs >= forge.cfg.cpu_seconds_per_day {
        return Err(too_many(
            "quota_cpu_exceeded",
            format!(
                "budget de calcul épuisé : {} s par jour (UTC)",
                forge.cfg.cpu_seconds_per_day
            ),
        ));
    }
    if u.pending >= forge.cfg.max_pending_per_account {
        return Err(too_many(
            "too_many_pending",
            format!(
                "{} builds déjà en attente ou en cours : attendez qu'un se termine",
                forge.cfg.max_pending_per_account
            ),
        ));
    }
    let pairs: Vec<(String, String)> = b
        .files
        .iter()
        .map(|f| (f.path.clone(), f.content.clone()))
        .collect();
    let build = Build::new(
        b.name,
        "queued".into(),
        acct.id.to_string(),
        b.version,
        String::new(),
        source_digest(&pairs),
        None,
        None,
        0,
        "[]".into(),
        None,
        None,
        None,
        None,
    );
    let build = forge.store.builds.create(build).await?;
    forge.enqueue(build.id, &b.files).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id": build.id, "status": "queued"})),
    ))
}

/// Un build n'existe que pour son propriétaire : pour tout autre, c'est un 404 (on ne
/// confirme pas qu'un identifiant existe).
async fn owned_build(forge: &Forge, acct: &Account, id: &str) -> Res<Build> {
    let nf = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "build inconnu");
    let id = Uuid::parse_str(id).map_err(|_| nf())?;
    let b = forge.store.builds.get(&id).await?.ok_or_else(nf)?;
    if b.account_id != acct.id.to_string() {
        return Err(nf());
    }
    Ok(b)
}

fn diagnostics_of(b: &Build) -> Vec<Diagnostic> {
    serde_json::from_str(&b.diagnostics_json).unwrap_or_default()
}

fn artifact_json(b: &Build) -> Value {
    match (&b.wasm_sha256, b.wasm_size, &b.signature) {
        (Some(h), Some(n), Some(s)) => json!({"sha256": h, "size": n, "signature": s}),
        _ => Value::Null,
    }
}

async fn get_build(
    State(forge): St,
    Authed(acct): Authed,
    Path(id): Path<String>,
) -> Res<Json<Value>> {
    let b = owned_build(&forge, &acct, &id).await?;
    Ok(Json(json!({
        "id": b.id,
        "status": b.status,
        "started_at": b.started_at,
        "finished_at": b.finished_at,
        "diagnostics": diagnostics_of(&b),
        "artifact": artifact_json(&b),
    })))
}

fn terminal(status: &str) -> bool {
    matches!(status, "signed" | "failed")
}

async fn build_events(
    State(forge): St,
    Authed(acct): Authed,
    Path(id): Path<String>,
) -> Res<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let first = owned_build(&forge, &acct, &id).await?;
    let bid = first.id;
    // Interroge l'état toutes les 250 ms : chaque changement de statut devient un événement.
    let stream = futures::stream::unfold(
        (forge, bid, None::<String>, false),
        |(forge, bid, last, done)| async move {
            if done {
                return None;
            }
            loop {
                let b = match forge.store.builds.get(&bid).await {
                    Ok(Some(b)) => b,
                    _ => return None,
                };
                if last.as_deref() != Some(b.status.as_str()) {
                    let mut evs: Vec<Result<Event, Infallible>> = vec![Ok(Event::default()
                        .event("status")
                        .data(json!({"status": b.status}).to_string()))];
                    let end = terminal(&b.status);
                    if end {
                        for d in diagnostics_of(&b) {
                            evs.push(Ok(Event::default()
                                .event("diagnostic")
                                .data(serde_json::to_string(&d).unwrap_or_default())));
                        }
                        if b.status == "signed" {
                            evs.push(Ok(Event::default()
                                .event("artifact")
                                .data(artifact_json(&b).to_string())));
                        }
                        evs.push(Ok(Event::default()
                            .event("done")
                            .data(json!({"status": b.status}).to_string())));
                    }
                    let st = b.status.clone();
                    return Some((futures::stream::iter(evs), (forge, bid, Some(st), end)));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        },
    )
    .flatten();
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn artifact(State(forge): St, Authed(acct): Authed, Path(id): Path<String>) -> Res<Response> {
    let b = owned_build(&forge, &acct, &id).await?;
    if b.status != "signed" {
        let (code, msg) = if b.status == "failed" {
            (
                "build_failed",
                "le build a échoué : aucun artefact (voir les diagnostics)",
            )
        } else {
            ("not_ready", "le build n'est pas terminé")
        };
        return Err(ApiError::new(StatusCode::CONFLICT, code, msg));
    }
    let bytes = std::fs::read(forge.artifact_path(&b.id)).map_err(ApiError::internal)?;
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/wasm"),
    );
    for (name, val) in [
        ("x-forge-signature", b.signature.as_deref()),
        ("x-forge-manifest", b.manifest_b64.as_deref()),
    ] {
        if let Some(v) = val {
            h.insert(name, HeaderValue::from_str(v).map_err(ApiError::internal)?);
        }
    }
    Ok((h, bytes).into_response())
}
