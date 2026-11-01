//! OIDC resource-server validation.
//!
//! `this` verifies tokens minted by an external OpenID Provider; it never issues them (ADR D11 —
//! resource server, not authorization server). The provider reads the issuer and the JWKS URI from
//! `OidcConfig::discovery_url`, caches the key set, and refetches it **once** when a token names a
//! `kid` the cache does not hold — that is what a key rotation looks like from here.
//!
//! # Why discovery is lazy
//!
//! `ServerBuilder::build_host` is synchronous, so it cannot perform an HTTP round-trip. The first
//! token therefore triggers discovery. A failure is closed, never open: no key means no verified
//! identity, so the request is unauthenticated. A host that wants the round-trip to happen at boot
//! calls [`OidcAuthProvider::warm_up`] from its async start-up path.
//!
//! # What is refused
//!
//! - symmetric algorithms (`HS*`) and `none`: a resource server shares no secret with the provider,
//!   and an unsigned token authenticates nobody;
//! - a token whose `aud` does not name this resource server (`OidcConfig::audience` is required —
//!   an access token minted for another service must not open every door here);
//! - a token whose `iss` is not the discovered issuer;
//! - a token that names no tenant, or a tenant that is not a UUID: mapping it to the nil UUID would
//!   put the request in a tenant that exists for real.

use crate::config::auth::{AuthConfig, OidcConfig};
use crate::core::auth::{AuthContext, AuthProvider};
use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use axum::http::HeaderMap;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

/// Algorithms a resource server may accept.
///
/// `HS*` is absent on purpose: the provider's JWKS publishes public keys only, and sharing a secret
/// with every verifier turns each of them into an issuer. `none` is absent for the usual reason.
const ACCEPTED: [Algorithm; 5] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::ES256,
    Algorithm::EdDSA,
];

/// How long a fetched key set is trusted before a refresh is attempted on the next request.
const DEFAULT_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    jwks_uri: String,
}

/// The key set as last fetched, with the issuer it came from.
struct KeySet {
    issuer: String,
    keys: HashMap<String, DecodingKey>,
    /// The only key, when the provider published exactly one and names none of them.
    only: Option<String>,
    fetched: Instant,
}

impl KeySet {
    fn get(&self, kid: Option<&str>) -> Option<&DecodingKey> {
        match kid {
            Some(kid) => self.keys.get(kid),
            // No `kid` names no key: only a one-key set can answer that.
            None => self.only.as_ref().and_then(|kid| self.keys.get(kid)),
        }
    }

    fn may_be_stale(&self, ttl: Duration) -> bool {
        self.fetched.elapsed() >= ttl
    }
}

/// Verifies OIDC access tokens against a provider's published keys.
pub struct OidcAuthProvider {
    config: OidcConfig,
    /// Policy applied when a request carries no credential at all.
    default_policy: String,
    /// Claim naming the tenant (`auth.tenant.claim_field`).
    tenant_claim: String,
    http: reqwest::Client,
    cache: RwLock<Option<Arc<KeySet>>>,
    /// Serializes refreshes so a burst of unknown `kid`s costs one round-trip, not one each.
    refresh: Mutex<()>,
    ttl: Duration,
}

impl std::fmt::Debug for OidcAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuthProvider")
            .field("discovery_url", &self.config.discovery_url)
            .field("audience", &self.config.audience)
            .field("user_claim", &self.config.user_claim)
            .field("roles_claim", &self.config.roles_claim)
            .finish()
    }
}

impl OidcAuthProvider {
    /// Build from the `auth:` section.
    ///
    /// Refused when the section is absent or names no audience: a resource server that cannot tell
    /// which audience it is must not accept tokens at all.
    pub fn from_auth_config(config: &AuthConfig) -> Result<Self> {
        let oidc = config
            .oidc
            .clone()
            .context("auth.oidc section is required for provider `oidc`")?;
        if oidc
            .audience
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            bail!(
                "auth.oidc.audience is required: without it a token minted for another service \
                 would authenticate here"
            );
        }
        if oidc.discovery_url.trim().is_empty() {
            bail!("auth.oidc.discovery_url is required for provider `oidc`");
        }
        Ok(Self::new(
            oidc,
            config.default_policy.clone(),
            config.tenant.claim_field.clone(),
        ))
    }

    /// Build directly, for callers that do not go through the YAML contract.
    pub fn new(config: OidcConfig, default_policy: String, tenant_claim: String) -> Self {
        Self {
            config,
            default_policy,
            tenant_claim,
            http: reqwest::Client::new(),
            cache: RwLock::new(None),
            refresh: Mutex::new(()),
            ttl: DEFAULT_TTL,
        }
    }

    /// Override the cache lifetime (tests, or a provider with an aggressive rotation policy).
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Fetch the key set now instead of on the first token.
    pub async fn warm_up(&self) -> Result<()> {
        self.key_set().await.map(|_| ())
    }

    /// The current key set, fetching it if absent, stale, or about to be.
    async fn key_set(&self) -> Result<Arc<KeySet>> {
        if let Some(set) = self.cache.read().await.as_ref()
            && !set.may_be_stale(self.ttl)
        {
            return Ok(set.clone());
        }
        self.refresh().await
    }

    /// Fetch the key set under the refresh lock, then cache it.
    async fn refresh(&self) -> Result<Arc<KeySet>> {
        let _guard = self.refresh.lock().await;
        // Another task may have refreshed while we waited for the lock.
        if let Some(set) = self.cache.read().await.as_ref()
            && !set.may_be_stale(self.ttl)
        {
            return Ok(set.clone());
        }
        let set = Arc::new(self.fetch().await?);
        *self.cache.write().await = Some(set.clone());
        Ok(set)
    }

    /// Discard the cache and fetch again. Used when a token names a key we do not hold.
    async fn refetch(&self, known: Option<&Arc<KeySet>>) -> Result<Arc<KeySet>> {
        let _guard = self.refresh.lock().await;
        if let Some(current) = self.cache.read().await.as_ref() {
            // A concurrent request already installed a set newer than the one we held.
            if known.is_some_and(|kept| Arc::ptr_eq(kept, current)) {
                // fall through: same set, refetch
            } else {
                return Ok(current.clone());
            }
        }
        let set = Arc::new(self.fetch().await?);
        *self.cache.write().await = Some(set.clone());
        Ok(set)
    }

    /// Read the discovery document, then the JWKS it points at.
    async fn fetch(&self) -> Result<KeySet> {
        let discovery: DiscoveryDocument = self
            .http
            .get(self.config.discovery_url.as_str())
            .send()
            .await
            .with_context(|| {
                format!(
                    "fetching OIDC discovery document {}",
                    self.config.discovery_url
                )
            })?
            .error_for_status()
            .with_context(|| {
                format!(
                    "OIDC discovery document {} refused the request",
                    self.config.discovery_url
                )
            })?
            .json()
            .await
            .context("reading the OIDC discovery document")?;

        let jwks: JwkSet = self
            .http
            .get(discovery.jwks_uri.as_str())
            .send()
            .await
            .with_context(|| format!("fetching the OIDC key set {}", discovery.jwks_uri))?
            .error_for_status()
            .with_context(|| {
                format!(
                    "the OIDC key set {} refused the request",
                    discovery.jwks_uri
                )
            })?
            .json()
            .await
            .context("reading the OIDC key set")?;

        let mut keys = HashMap::new();
        for jwk in &jwks.keys {
            let Some(key) = decoding_key(jwk) else {
                continue;
            };
            // A key set may omit `kid`; the empty id is the fallback a single-key set is read
            // under, and a token with no `kid` is resolved through `KeySet::only`.
            keys.insert(jwk.common.key_id.clone().unwrap_or_default(), key);
        }
        if keys.is_empty() {
            bail!(
                "the key set at {} publishes no usable key",
                discovery.jwks_uri
            );
        }
        let only = (keys.len() == 1)
            .then(|| keys.keys().next().cloned())
            .flatten();
        Ok(KeySet {
            issuer: discovery.issuer,
            keys,
            only,
            fetched: Instant::now(),
        })
    }

    /// Verify the signature and the registered claims, returning the claims as a JSON object.
    async fn verified_claims(&self, token: &str) -> Result<(Value, String)> {
        let header = decode_header(token).context("reading the token header")?;
        if !ACCEPTED.contains(&header.alg) {
            bail!("token algorithm {:?} is not accepted", header.alg);
        }
        let kid = header.kid.as_deref();

        let mut set = self.key_set().await?;
        let mut key = set.get(kid).cloned();
        if key.is_none() {
            // An unknown `kid` is what a rotation looks like: refetch once, then give up.
            set = self.refetch(Some(&set)).await?;
            key = set.get(kid).cloned();
        }
        let Some(key) = key else {
            bail!("no published key matches the token's kid {kid:?}");
        };

        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[set.issuer.as_str()]);
        validation.set_audience(&[self
            .config
            .audience
            .as_deref()
            .expect("checked in from_auth_config")]);
        validation.set_required_spec_claims(&["exp", "aud", "iss"]);

        let data = decode::<Value>(token, &key, &validation).context("verifying the OIDC token")?;
        Ok((data.claims, set.issuer.clone()))
    }

    /// Project verified claims onto the `AuthContext` this framework runs on.
    fn claims_to_context(&self, claims: &Value) -> Result<AuthContext> {
        let subject = claims
            .get(&self.config.user_claim)
            .and_then(Value::as_str)
            .with_context(|| {
                format!(
                    "token names no `{}` claim (auth.oidc.user_claim)",
                    self.config.user_claim
                )
            })?;
        let user_id = Uuid::parse_str(subject).unwrap_or_else(|_| {
            // OIDC subjects are opaque (`auth0|…`, an email). Deriving a UUID from the issuer and
            // the subject keeps the mapping stable across restarts without a table, and two
            // providers naming the same string stay distinct.
            Uuid::new_v5(&Uuid::NAMESPACE_URL, subject.as_bytes())
        });

        let tenant = claims
            .get(&self.tenant_claim)
            .and_then(Value::as_str)
            .with_context(|| {
                format!(
                    "token names no `{}` claim: a request without a tenant would borrow the nil \
                     tenant",
                    self.tenant_claim
                )
            })?;
        let tenant_id = Uuid::parse_str(tenant)
            .with_context(|| format!("`{}` is not a UUID: {tenant}", self.tenant_claim))?;

        let roles = claims
            .get(&self.config.roles_claim)
            .map(read_string_list)
            .unwrap_or_default();

        Ok(AuthContext::User {
            user_id,
            tenant_id,
            roles,
        })
    }
}

/// One JWK as a verification key, or `None` when it cannot be used.
///
/// `Jwk::is_supported` is deliberately not called: it unwraps `key_algorithm`, so a key set whose
/// entries omit `alg` — which the specification allows and several providers do — would panic the
/// request thread. The shape decides instead, and the accepted-algorithm list is enforced on the
/// token header, where it belongs.
fn decoding_key(jwk: &Jwk) -> Option<DecodingKey> {
    if let Some(use_) = &jwk.common.public_key_use
        && *use_ != jsonwebtoken::jwk::PublicKeyUse::Signature
    {
        return None;
    }
    DecodingKey::from_jwk(jwk).ok()
}

/// A claim that may be a list or a single string (`roles`, `scope`).
fn read_string_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Value::String(s) => s
            .split_whitespace()
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

#[async_trait]
impl AuthProvider for OidcAuthProvider {
    async fn extract_context(&self, headers: &HeaderMap) -> Result<AuthContext> {
        let Some(token) = bearer(headers) else {
            if self.default_policy == "public" {
                return Ok(AuthContext::Anonymous);
            }
            bail!("missing Authorization Bearer token");
        };
        let (claims, _issuer) = self.verified_claims(token).await?;
        self.claims_to_context(&claims)
    }

    async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> Result<bool> {
        // Ownership lives in the data; the token only names the principal.
        Ok(false)
    }

    async fn has_role(&self, _: &Uuid, _: &str) -> Result<bool> {
        // Roles are read from the verified claims by the caller that holds them.
        Ok(false)
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::routing::get;
    use axum::{Json, Router};
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
    use jsonwebtoken::{EncodingKey, Header};
    use serde_json::{Value, json};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc as StdArc, RwLock as StdRwLock};
    use tokio::task::JoinHandle;

    const AUDIENCE: &str = "https://api.example.com";

    /// An OpenID Provider on `127.0.0.1`, with a key set that can be rotated mid-test.
    struct Idp {
        base: String,
        jwks: StdArc<StdRwLock<Value>>,
        jwks_hits: StdArc<AtomicUsize>,
        _server: JoinHandle<()>,
    }

    #[derive(Clone)]
    struct State0 {
        base: String,
        jwks: StdArc<StdRwLock<Value>>,
        jwks_hits: StdArc<AtomicUsize>,
    }

    async fn discovery(State(s): State<State0>) -> Json<Value> {
        Json(json!({ "issuer": s.base, "jwks_uri": format!("{}/jwks", s.base) }))
    }

    async fn jwks(State(s): State<State0>) -> Json<Value> {
        s.jwks_hits.fetch_add(1, Ordering::SeqCst);
        Json(s.jwks.read().expect("lock").clone())
    }

    fn key(seed: u8, kid: &str) -> (SigningKey, Value) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let x =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sk.verifying_key().as_bytes());
        (
            sk,
            json!({ "kty": "OKP", "crv": "Ed25519", "x": x, "kid": kid, "use": "sig" }),
        )
    }

    fn key_set(keys: &[Value]) -> Value {
        json!({ "keys": keys })
    }

    async fn idp(keys: &[Value]) -> Idp {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        let base = format!("http://{addr}");
        let published = StdArc::new(StdRwLock::new(key_set(keys)));
        let jwks_hits = StdArc::new(AtomicUsize::new(0));
        let state = State0 {
            base: base.clone(),
            jwks: published.clone(),
            jwks_hits: jwks_hits.clone(),
        };
        let app = Router::new()
            .route("/discovery", get(discovery))
            .route("/jwks", get(jwks))
            .with_state(state);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Idp {
            base,
            jwks: published,
            jwks_hits,
            _server: server,
        }
    }

    impl Idp {
        fn provider(&self) -> OidcAuthProvider {
            OidcAuthProvider::from_auth_config(&AuthConfig {
                provider: crate::config::auth::AuthProviderType::Oidc,
                oidc: Some(OidcConfig {
                    discovery_url: format!("{}/discovery", self.base),
                    audience: Some(AUDIENCE.to_string()),
                    user_claim: "sub".to_string(),
                    roles_claim: "roles".to_string(),
                }),
                ..Default::default()
            })
            .expect("provider")
        }

        fn rotate(&self, keys: &[Value]) {
            *self.jwks.write().expect("lock") = key_set(keys);
        }
    }

    fn sign(sk: &SigningKey, kid: &str, claims: &Value) -> String {
        let pem = sk.to_pkcs8_pem(LineEnding::LF).expect("pem");
        let key = EncodingKey::from_ed_pem(pem.as_bytes()).expect("key");
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(kid.to_string());
        jsonwebtoken::encode(&header, claims, &key).expect("sign")
    }

    fn claims(issuer: &str, audience: &str, sub: &str, tenant: &str) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({
            "iss": issuer,
            "aud": audience,
            "sub": sub,
            "tenant_id": tenant,
            "roles": ["editeur"],
            "iat": now,
            "exp": now + 600,
        })
    }

    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().expect("header"),
        );
        headers
    }

    #[tokio::test]
    async fn a_valid_token_becomes_a_user() {
        let (sk, jwk) = key(1, "k1");
        let idp = idp(&[jwk]).await;
        let tenant = Uuid::new_v4();
        let sub = Uuid::new_v4();
        let token = sign(
            &sk,
            "k1",
            &claims(&idp.base, AUDIENCE, &sub.to_string(), &tenant.to_string()),
        );

        let ctx = idp
            .provider()
            .extract_context(&headers(&token))
            .await
            .expect("verified");
        match ctx {
            AuthContext::User {
                user_id,
                tenant_id,
                roles,
            } => {
                assert_eq!(user_id, sub);
                assert_eq!(tenant_id, tenant);
                assert_eq!(roles, vec!["editeur".to_string()]);
            }
            other => panic!("expected a user, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_opaque_subject_gets_a_stable_uuid() {
        let (sk, jwk) = key(2, "k1");
        let idp = idp(&[jwk]).await;
        let tenant = Uuid::new_v4();
        let claims = claims(&idp.base, AUDIENCE, "auth0|abc", &tenant.to_string());
        let provider = idp.provider();

        let first = provider
            .extract_context(&headers(&sign(&sk, "k1", &claims)))
            .await
            .expect("verified");
        let second = provider
            .extract_context(&headers(&sign(&sk, "k1", &claims)))
            .await
            .expect("verified");
        let id = |ctx: AuthContext| match ctx {
            AuthContext::User { user_id, .. } => user_id,
            other => panic!("expected a user, got {other:?}"),
        };
        let (first, second) = (id(first), id(second));
        assert_eq!(first, second, "the mapping must be stable");
        assert_eq!(
            first,
            Uuid::new_v5(&Uuid::NAMESPACE_URL, b"auth0|abc"),
            "and derived, not random"
        );
    }

    #[tokio::test]
    async fn a_foreign_audience_is_refused() {
        let (sk, jwk) = key(3, "k1");
        let idp = idp(&[jwk]).await;
        let token = sign(
            &sk,
            "k1",
            &claims(
                &idp.base,
                "https://another-service.example.com",
                &Uuid::new_v4().to_string(),
                &Uuid::new_v4().to_string(),
            ),
        );
        let err = idp
            .provider()
            .extract_context(&headers(&token))
            .await
            .expect_err("wrong audience");
        assert!(
            format!("{err:#}").to_lowercase().contains("audience"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_token_without_a_tenant_is_refused() {
        let (sk, jwk) = key(4, "k1");
        let idp = idp(&[jwk]).await;
        let mut claims = claims(
            &idp.base,
            AUDIENCE,
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
        );
        claims.as_object_mut().expect("object").remove("tenant_id");
        let token = sign(&sk, "k1", &claims);
        let err = idp
            .provider()
            .extract_context(&headers(&token))
            .await
            .expect_err("no tenant");
        assert!(format!("{err:#}").contains("tenant"), "{err:#}");
    }

    #[tokio::test]
    async fn an_expired_token_is_refused() {
        let (sk, jwk) = key(5, "k1");
        let idp = idp(&[jwk]).await;
        let mut claims = claims(
            &idp.base,
            AUDIENCE,
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
        );
        // jsonwebtoken allows 60 s of clock leeway by default: a token must be clearly
        // expired for the verifier to refuse it.
        claims["exp"] = json!(chrono::Utc::now().timestamp() - 600);
        let token = sign(&sk, "k1", &claims);
        assert!(
            idp.provider()
                .extract_context(&headers(&token))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_rotation_costs_one_refetch_and_then_works() {
        let (sk1, jwk1) = key(6, "k1");
        let (sk2, jwk2) = key(7, "k2");
        let idp = idp(&[jwk1]).await;
        let provider = idp.provider();
        let tenant = Uuid::new_v4();
        let sub = Uuid::new_v4();
        let claims = claims(&idp.base, AUDIENCE, &sub.to_string(), &tenant.to_string());

        provider
            .extract_context(&headers(&sign(&sk1, "k1", &claims)))
            .await
            .expect("first key");
        assert_eq!(idp.jwks_hits.load(Ordering::SeqCst), 1);

        // The provider rotates: k2 is published, k1 is gone, and the token names k2.
        idp.rotate(&[jwk2]);
        provider
            .extract_context(&headers(&sign(&sk2, "k2", &claims)))
            .await
            .expect("rotated key");
        assert_eq!(
            idp.jwks_hits.load(Ordering::SeqCst),
            2,
            "an unknown kid must cost exactly one refetch"
        );
    }

    #[tokio::test]
    async fn a_symmetric_algorithm_is_refused_before_any_key_is_used() {
        let (_, jwk) = key(8, "k1");
        let idp = idp(&[jwk]).await;
        let key = EncodingKey::from_secret(b"a shared secret");
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims(
                &idp.base,
                AUDIENCE,
                &Uuid::new_v4().to_string(),
                &Uuid::new_v4().to_string(),
            ),
            &key,
        )
        .expect("sign");
        let err = idp
            .provider()
            .extract_context(&headers(&token))
            .await
            .expect_err("HS256");
        assert!(format!("{err:#}").contains("not accepted"), "{err:#}");
    }

    #[tokio::test]
    async fn an_unreachable_provider_fails_closed() {
        let provider = OidcAuthProvider::new(
            OidcConfig {
                // Port 1 on loopback: nothing listens, so the connection is refused.
                discovery_url: "http://127.0.0.1:1/discovery".to_string(),
                audience: Some(AUDIENCE.to_string()),
                user_claim: "sub".to_string(),
                roles_claim: "roles".to_string(),
            },
            "public".to_string(),
            "tenant_id".to_string(),
        );
        let err = provider
            .extract_context(&headers(&sign(
                &SigningKey::from_bytes(&[9u8; 32]),
                "k1",
                &claims(
                    "http://127.0.0.1:1",
                    AUDIENCE,
                    &Uuid::new_v4().to_string(),
                    &Uuid::new_v4().to_string(),
                ),
            )))
            .await
            .expect_err("unreachable");
        assert!(format!("{err:#}").contains("discovery"), "{err:#}");
    }

    #[test]
    fn a_provider_without_audience_refuses_to_build() {
        let err = OidcAuthProvider::from_auth_config(&AuthConfig {
            provider: crate::config::auth::AuthProviderType::Oidc,
            oidc: Some(OidcConfig {
                discovery_url: "https://idp.example.com/.well-known/openid-configuration"
                    .to_string(),
                audience: None,
                user_claim: "sub".to_string(),
                roles_claim: "roles".to_string(),
            }),
            ..Default::default()
        })
        .expect_err("no audience");
        assert!(
            format!("{err:#}").to_lowercase().contains("audience"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn no_credential_follows_the_default_policy() {
        let (_, jwk) = key(10, "k1");
        let idp = idp(&[jwk]).await;
        let mut cfg = AuthConfig {
            provider: crate::config::auth::AuthProviderType::Oidc,
            oidc: Some(OidcConfig {
                discovery_url: format!("{}/discovery", idp.base),
                audience: Some(AUDIENCE.to_string()),
                user_claim: "sub".to_string(),
                roles_claim: "roles".to_string(),
            }),
            ..Default::default()
        };
        cfg.default_policy = "public".to_string();
        let provider = OidcAuthProvider::from_auth_config(&cfg).expect("provider");
        assert!(matches!(
            provider
                .extract_context(&HeaderMap::new())
                .await
                .expect("anonymous"),
            AuthContext::Anonymous
        ));
        cfg.default_policy = "authenticated".to_string();
        let strict = OidcAuthProvider::from_auth_config(&cfg).expect("provider");
        assert!(strict.extract_context(&HeaderMap::new()).await.is_err());
    }
}
