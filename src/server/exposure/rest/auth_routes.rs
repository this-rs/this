//! STS authentication endpoints
//!
//! - POST /auth/token  -- Login with credentials, get JWT + refresh token
//! - GET  /auth/keys   -- JWKS public key distribution
//! - POST /auth/refresh -- Refresh access token
//! - POST /auth/revoke  -- Revoke a token by JTI
//! - GET|POST /auth/verify -- **la porte** : ce qu'un proxy interroge avant de
//!   laisser passer une requête vers un projet rattaché (Caddy `forward_auth`)

use crate::core::auth::sts::{
    LoginCredentials, RefreshRequest, RevokeRequest, StsClaims, StsService,
};
use crate::server::middleware::auth::{COOKIE_JETON, jeton_de_la_requete};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing,
};
use jsonwebtoken::{Algorithm, Validation};
use std::sync::Arc;

/// Le nom du cookie — un seul, partagé avec la couche d'authentification.
pub use crate::server::middleware::auth::COOKIE_JETON as COOKIE_NAME;

/// Shared state for the STS auth routes
pub struct StsState {
    pub sts: Arc<StsService>,
    pub public_key_pem: String,
    /// Decoding key for refresh-token verification
    pub decoding_key: jsonwebtoken::DecodingKey,
}

/// POST /auth/token
///
/// Émet la paire de jetons, **et pose le cookie de session**.
///
/// Le cookie est ce qui rend la structure utilisable depuis un **navigateur** :
/// une page ne peut pas poser `Authorization: Bearer …` sur une navigation, et
/// sans cookie il ne reste que deux issues pour une surface rattachée — la
/// rendre publique, ou réécrire son authentification. Aucune n'est acceptable
/// pour rattacher un projet qui existe déjà.
///
/// Attributs, et pourquoi : `HttpOnly` (un script ne lit pas le jeton),
/// `SameSite=Lax` (une page tierce ne déclenche pas la navigation
/// authentifiée), `Path=/`, `Max-Age` = durée de vie du jeton d'accès — et
/// `Secure` **seulement** quand la requête est arrivée en HTTPS
/// (`X-Forwarded-Proto`), sinon un déploiement derrière un proxy TLS perdrait
/// la session au premier clic.
pub async fn login_handler(
    State(state): State<Arc<StsState>>,
    headers: HeaderMap,
    Json(credentials): Json<LoginCredentials>,
) -> Response {
    match state.sts.login(&credentials) {
        Ok(pair) => {
            let cookie = cookie_de_session(&pair.access_token, pair.expires_in, &headers);
            (
                StatusCode::OK,
                [(header::SET_COOKIE, cookie)],
                Json(serde_json::to_value(pair).unwrap()),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_credentials"})),
        )
            .into_response(),
    }
}

/// Le cookie de session, tel qu'il part vers le client.
fn cookie_de_session(jeton: &str, duree_secs: i64, requete: &HeaderMap) -> HeaderValue {
    let en_https = requete
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|proto| proto.eq_ignore_ascii_case("https"));
    let mut cookie =
        format!("{COOKIE_JETON}={jeton}; Path=/; HttpOnly; SameSite=Lax; Max-Age={duree_secs}");
    if en_https {
        cookie.push_str("; Secure");
    }
    HeaderValue::from_str(&cookie).expect("un jeton JWT est un en-tête valide")
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

/// **La porte** : `GET|POST /auth/verify`.
///
/// C'est l'endpoint qu'un reverse proxy interroge **avant** de laisser passer une
/// requête vers un projet rattaché (le `forward_auth` de Caddy) : 2xx = laisser
/// passer, 401 = refuser. Sans lui, un projet qui existe déjà ne peut pas être
/// mis sous la garde du gateway sans réécrire sa propre authentification — le
/// proxy n'a **rien** à qui demander.
///
/// # Ce qu'elle vérifie, et pourquoi dans cet ordre
///
/// 1. la **signature** EdDSA (clé publique de l'émetteur) ;
/// 2. l'**échéance** (`exp`) ;
/// 3. l'**émetteur** (`iss`) — une signature valide ne dit pas *qui* a signé ;
/// 4. la **révocation** par `jti` — un jeton révoqué qui ouvrirait encore la
///    porte rendrait `POST /auth/revoke` décoratif, sur le seul chemin qui garde
///    un projet qu'on n'a pas réécrit.
///
/// # Où elle lit le jeton
///
/// `Authorization: Bearer …`, et **à défaut** le cookie [`COOKIE_NAME`] : un
/// navigateur ne peut pas poser d'en-tête sur une navigation, et une garde
/// inutilisable devant une page ne garde rien.
///
/// # Ce qu'elle rend
///
/// L'identité en en-têtes — `x-this-subject`, `x-this-tenant`, `x-this-roles` —
/// que `forward_auth` recopie vers la requête d'origine : le projet rattaché
/// reçoit **qui** appelle, sans rien réimplémenter.
///
/// **Franchise** : ces en-têtes ne sont dignes de confiance que parce que la
/// requête vient de la porte. Le projet rattaché ne doit donc être joignable
/// **que** par elle ; exposé en direct, il accepte des en-têtes forgés.
pub async fn verify_handler(State(state): State<Arc<StsState>>, headers: HeaderMap) -> Response {
    let Some(token) = jeton_de_la_requete(&headers) else {
        return refusal("missing_token");
    };

    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_issuer(&[state.sts.issuer()]);
    // L'audience est vérifiée par le STS (`check_audience`), pas par jsonwebtoken :
    // celui-ci refuse tout `aud` présent quand aucune audience n'est attendue.
    validation.validate_aud = false;

    let claims = match jsonwebtoken::decode::<StsClaims>(&token, &state.decoding_key, &validation) {
        Ok(data) => data.claims,
        Err(error) => {
            tracing::debug!(%error, "porte : jeton refusé (signature, échéance ou émetteur)");
            return refusal("invalid_token");
        }
    };

    if let Err(error) = state.sts.check_audience(claims.aud.as_deref()) {
        tracing::debug!(%error, "porte : jeton refusé (audience)");
        return refusal("invalid_token");
    }

    // Un jeton de rafraîchissement n'authentifie pas une requête (ADR D4).
    if claims.token_use.as_deref() == Some(crate::core::auth::sts::TOKEN_USE_REFRESH) {
        tracing::debug!("porte : jeton de rafraîchissement refusé comme bearer");
        return refusal("invalid_token");
    }

    if state.sts.is_revoked(&claims.jti) {
        tracing::debug!(jti = %claims.jti, "porte : jeton révoqué");
        return refusal("revoked_token");
    }

    let identity = [
        (
            HeaderName::from_static("x-this-subject"),
            claims.sub.clone(),
        ),
        (
            HeaderName::from_static("x-this-tenant"),
            claims.tenant_id.to_string(),
        ),
        (
            HeaderName::from_static("x-this-roles"),
            claims.roles.join(","),
        ),
    ];

    (
        StatusCode::OK,
        identity,
        Json(serde_json::json!({
            "sub": claims.sub,
            "tenant_id": claims.tenant_id,
            "roles": claims.roles,
            "exp": claims.exp,
            "iss": claims.iss,
        })),
    )
        .into_response()
}

/// Un refus, toujours explicable.
fn refusal(reason: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
        Json(serde_json::json!({ "error": reason })),
    )
        .into_response()
}

/// Build the auth router with all STS endpoints
pub fn auth_router(sts_state: Arc<StsState>) -> Router {
    Router::new()
        .route("/auth/token", routing::post(login_handler))
        .route("/auth/keys", routing::get(jwks_handler))
        .route("/auth/refresh", routing::post(refresh_handler))
        .route("/auth/revoke", routing::post(revoke_handler))
        .route(
            "/auth/verify",
            routing::get(verify_handler).post(verify_handler),
        )
        .with_state(sts_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
    use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use tower::ServiceExt;
    use uuid::Uuid;

    struct Setup {
        state: Arc<StsState>,
        token: String,
        subject: Uuid,
        tenant: Uuid,
    }

    /// Les revendications d'un jeton, **sans vérifier la signature**.
    ///
    /// C'est ce que fait un attaquant, et c'est ce dont ces tests ont besoin :
    /// le `jti` à révoquer, et les revendications à rejouer sous une autre clé.
    fn claims_without_verifying(token: &str) -> StsClaims {
        use base64::Engine;
        let payload = token.split('.').nth(1).expect("un JWT a trois segments");
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .expect("charge utile en base64url");
        serde_json::from_slice(&bytes).expect("revendications lisibles")
    }

    /// Un STS réel, en mode bootstrap : clé générée, un utilisateur, un jeton
    /// obtenu par le **vrai** `POST /auth/token` — pas un jeton fabriqué pour le
    /// test.
    fn setup() -> Setup {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public_pem = signing
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("clé publique en PEM");
        let private_pem = signing
            .to_pkcs8_pem(LineEnding::LF)
            .expect("clé privée en PEM")
            .to_string();

        let sts = StsService::new(&private_pem, "test-issuer".to_string()).expect("STS");
        let subject = Uuid::new_v4();
        let tenant = Uuid::new_v4();
        sts.register_user(
            "dave".into(),
            "pw".into(),
            subject,
            tenant,
            vec!["editor".into()],
        );
        let pair = sts
            .login(&LoginCredentials {
                username: "dave".into(),
                password: "pw".into(),
            })
            .expect("jeton émis");

        let decoding_key =
            jsonwebtoken::DecodingKey::from_ed_pem(public_pem.as_bytes()).expect("clé de décodage");

        Setup {
            state: Arc::new(StsState {
                sts: Arc::new(sts),
                public_key_pem: public_pem,
                decoding_key,
            }),
            token: pair.access_token,
            subject,
            tenant,
        }
    }

    async fn verify(
        state: Arc<StsState>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, serde_json::Value, HeaderMap) {
        let mut request = Request::builder().method("GET").uri("/auth/verify");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = auth_router(state)
            .oneshot(request.body(Body::empty()).expect("requête"))
            .await
            .expect("réponse");
        let status = response.status();
        let response_headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .expect("corps");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json, response_headers)
    }

    #[tokio::test]
    async fn the_gate_refuses_a_request_without_a_token() {
        let setup = setup();
        let (status, body, headers) = verify(setup.state, &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "missing_token");
        assert_eq!(
            headers
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer"),
            "un refus doit dire comment s'authentifier"
        );
    }

    #[tokio::test]
    async fn the_gate_opens_a_token_emitted_by_the_sts_and_publishes_the_identity() {
        let setup = setup();
        let bearer = format!("Bearer {}", setup.token);
        let (status, body, headers) = verify(setup.state, &[("authorization", &bearer)]).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sub"], setup.subject.to_string());
        // L'identité que `forward_auth` recopie vers le projet rattaché.
        assert_eq!(
            headers.get("x-this-subject").and_then(|v| v.to_str().ok()),
            Some(setup.subject.to_string().as_str())
        );
        assert_eq!(
            headers.get("x-this-tenant").and_then(|v| v.to_str().ok()),
            Some(setup.tenant.to_string().as_str())
        );
        assert_eq!(
            headers.get("x-this-roles").and_then(|v| v.to_str().ok()),
            Some("editor")
        );
    }

    #[tokio::test]
    async fn the_cookie_carries_the_token_too() {
        let setup = setup();
        let cookie = format!("{}={}", COOKIE_NAME, setup.token);
        let (status, _, _) = verify(setup.state, &[("cookie", &cookie)]).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "une page ne peut pas poser d'en-tête : le cookie doit ouvrir la porte"
        );
    }

    #[tokio::test]
    async fn a_revoked_token_no_longer_opens_the_gate() {
        let setup = setup();
        // Le `jti` se lit dans le jeton, comme le ferait un attaquant : on ne
        // révoque pas « quelque chose », on révoque CE jeton.
        let decoded = claims_without_verifying(&setup.token);
        setup.state.sts.revoke(&decoded.jti);

        let bearer = format!("Bearer {}", setup.token);
        let (status, body, _) = verify(setup.state, &[("authorization", &bearer)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            body["error"], "revoked_token",
            "révoquer doit fermer la porte, sinon `POST /auth/revoke` est décoratif"
        );
    }

    #[tokio::test]
    async fn a_token_signed_by_someone_else_does_not_open_the_gate() {
        let setup = setup();
        let claims = claims_without_verifying(&setup.token);

        // Même contenu, autre clé : c'est l'attaque que la signature doit arrêter.
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let other_pem = other
            .to_pkcs8_pem(LineEnding::LF)
            .expect("clé privée en PEM")
            .to_string();
        let forged = encode(
            &Header::new(Algorithm::EdDSA),
            &claims,
            &EncodingKey::from_ed_pem(other_pem.as_bytes()).expect("clé d'encodage"),
        )
        .expect("jeton forgé");

        let bearer = format!("Bearer {forged}");
        let (status, body, _) = verify(setup.state, &[("authorization", &bearer)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "invalid_token");
    }
}
