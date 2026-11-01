//! WAMI Auth Provider — Ed25519 JWT verification
//!
//! Feature-gated behind `wami`. Implements the `AuthProvider` trait using
//! `jsonwebtoken` with EdDSA (Ed25519) algorithm for JWT verification.
//!
//! ## Token flow
//!
//! 1. Client obtains a JWT from the STS (Security Token Service)
//! 2. JWT is signed with Ed25519 private key
//! 3. This provider verifies the JWT using the corresponding public key
//! 4. Claims are mapped to `AuthContext` (user_id, tenant_id, roles, etc.)

use crate::config::auth::AuthConfig;
use crate::core::auth::{AuthContext, AuthProvider};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use axum::http::HeaderMap;
use ed25519_dalek::VerifyingKey;
use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// JWT claims expected from a WAMI STS token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StsClaims {
    /// Subject — usually the user_id or principal ARN
    pub sub: String,

    /// Issuer — must match configured issuer
    #[serde(default)]
    pub iss: Option<String>,

    /// Audience — must match configured audience
    #[serde(default)]
    pub aud: Option<String>,

    /// Expiration time (Unix timestamp)
    pub exp: u64,

    /// Issued at (Unix timestamp)
    #[serde(default)]
    pub iat: Option<u64>,

    /// Not before (Unix timestamp)
    #[serde(default)]
    pub nbf: Option<u64>,

    /// Tenant ID
    #[serde(default)]
    pub tenant_id: Option<Uuid>,

    /// User ID (may differ from sub if sub is an ARN)
    #[serde(default)]
    pub user_id: Option<Uuid>,

    /// Roles granted to the user
    #[serde(default)]
    pub roles: Vec<String>,

    /// Principal ARN (WAMI-specific, e.g. "arn:wami:iam::user/alice")
    #[serde(default)]
    pub principal_arn: Option<String>,

    /// Scoped policy statements (serialized as strings)
    #[serde(default)]
    pub scoped_policies: Vec<String>,

    /// Service name (for service-to-service tokens)
    #[serde(default)]
    pub service_name: Option<String>,

    /// Whether this is an admin token
    #[serde(default)]
    pub is_admin: bool,
}

/// Auth provider that verifies Ed25519 JWT tokens from WAMI STS
pub struct WamiAuthProvider {
    /// The Ed25519 public key for JWT verification
    decoding_key: DecodingKey,

    /// JWT validation parameters
    validation: Validation,

    /// Original config for reference
    config: AuthConfig,

    /// ADR D4: accept tokens without `aud` when an audience is configured.
    accept_legacy_tokens: bool,
}

/// The `token_use` claim, read from the payload of an already-verified token.
///
/// Only call this after the signature has been checked: the payload is data
/// until then.
fn token_use_of(token: &str) -> Option<String> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("token_use")?.as_str().map(str::to_string)
}

impl std::fmt::Debug for WamiAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WamiAuthProvider")
            .field("config", &self.config)
            .finish()
    }
}

impl WamiAuthProvider {
    /// Create a new WamiAuthProvider from an AuthConfig
    ///
    /// The config must contain wami-specific fields (public_key at minimum).
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No WAMI config is present
    /// - The public key is invalid
    /// - The key cannot be decoded from base64/PEM
    pub fn from_config(config: &AuthConfig) -> Result<Self> {
        let wami_config = config
            .wami
            .as_ref()
            .ok_or_else(|| anyhow!("AuthConfig.wami section is required for WamiAuthProvider"))?;

        let public_key = wami_config
            .public_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| anyhow!("wami.public_key is required (cannot be empty)"))?;

        let decoding_key =
            Self::parse_public_key(public_key).context("Failed to parse Ed25519 public key")?;

        let mut validation = Validation::new(Algorithm::EdDSA);

        // Configure issuer validation
        if let Some(ref issuer) = wami_config.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        } else {
            // Don't require issuer if not configured
            validation.set_issuer::<&str>(&[]);
        }

        // Audience: required outside bootstrap (ADR D9). A token for another
        // service, signed by a shared key, would otherwise open every door here.
        let strict = !matches!(wami_config.mode, crate::config::auth::AuthMode::Bootstrap);
        match &wami_config.audience {
            Some(audience) => validation.set_audience(&[audience.as_str()]),
            None if strict => bail!(
                "wami.audience is required in {:?} mode: without it a token issued for \
                 another service would be accepted. Set it, or use bootstrap mode for development.",
                wami_config.mode
            ),
            None => {
                // Bootstrap (development): accept any `aud`, including none.
                // jsonwebtoken would refuse an `aud` present with an empty expected set.
                validation.validate_aud = false;
                tracing::warn!(
                    "wami.audience is not set (bootstrap mode): tokens signed by this key are \
                     accepted whatever their audience."
                );
            }
        }

        // Always validate expiration
        validation.validate_exp = true;
        // Validate nbf if present
        validation.validate_nbf = true;

        Ok(Self {
            decoding_key,
            validation,
            config: config.clone(),
            // ADR D4/D9: the legacy window is open in bootstrap only, unless the
            // operator opens it explicitly for a migration.
            accept_legacy_tokens: wami_config.accept_legacy_tokens.unwrap_or(!strict),
        })
    }

    /// Whether tokens without `aud` are accepted when an audience is configured.
    ///
    /// The ADR D4 window, during which tokens issued before the audience existed
    /// still verify. From config: `wami.accept_legacy_tokens`, default `true` in
    /// bootstrap mode only.
    #[must_use]
    pub fn with_legacy_tokens(mut self, accept: bool) -> Self {
        self.accept_legacy_tokens = accept;
        self
    }

    /// Parse an Ed25519 public key from base64 or PEM format
    fn parse_public_key(key_str: &str) -> Result<DecodingKey> {
        let key_str = key_str.trim();

        if key_str.starts_with("-----BEGIN") {
            // PEM format
            DecodingKey::from_ed_pem(key_str.as_bytes()).context("Invalid Ed25519 PEM public key")
        } else {
            // Raw base64 — decode to 32-byte Ed25519 public key
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(key_str)
                .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(key_str))
                .context("Public key is not valid base64")?;

            if bytes.len() != 32 {
                bail!("Ed25519 public key must be 32 bytes, got {}", bytes.len());
            }

            // Validate the key is a valid Ed25519 point
            let key_bytes: [u8; 32] = bytes.as_slice().try_into().unwrap();
            let verify_key =
                VerifyingKey::from_bytes(&key_bytes).context("Invalid Ed25519 public key bytes")?;

            // jsonwebtoken expects SPKI DER format, not raw bytes
            use ed25519_dalek::pkcs8::EncodePublicKey;
            let spki_der = verify_key
                .to_public_key_der()
                .context("Failed to encode public key as SPKI DER")?;
            Ok(DecodingKey::from_ed_der(spki_der.as_bytes()))
        }
    }

    /// Verify a JWT token and return the decoded claims
    fn verify_token(&self, token: &str) -> Result<TokenData<StsClaims>> {
        // Basic structure validation before sending to jsonwebtoken
        let dot_count = token.chars().filter(|c| *c == '.').count();
        if dot_count != 2 {
            bail!(
                "Malformed JWT: expected 3 segments separated by dots, got {}",
                dot_count + 1
            );
        }

        let data = jsonwebtoken::decode::<StsClaims>(token, &self.decoding_key, &self.validation)
            .context("JWT verification failed")?;

        // Signature and expiry are checked; now the claims can be trusted.
        if token_use_of(token).as_deref() == Some(crate::core::auth::sts::TOKEN_USE_REFRESH) {
            bail!("a refresh token cannot authenticate a request");
        }
        // With an audience configured, a token that names none is only accepted
        // inside the legacy window (ADR D4). jsonwebtoken alone would accept it.
        let audience_configured = self
            .config
            .wami
            .as_ref()
            .is_some_and(|w| w.audience.is_some());
        if audience_configured && data.claims.aud.is_none() && !self.accept_legacy_tokens {
            bail!("token carries no audience and legacy tokens are refused");
        }
        Ok(data)
    }

    /// Extract Bearer token from Authorization header
    fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
        headers
            .get(axum::http::header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")
    }

    /// Map verified StsClaims into an AuthContext.
    ///
    /// A user token must name both a user and a tenant. Without them the claims
    /// are refused, never mapped to the nil UUID: a nil identity is a real
    /// identifier that the identity registry would then give a segment to.
    fn claims_to_context(claims: &StsClaims) -> Result<AuthContext> {
        // Admin token
        if claims.is_admin {
            if let Some(user_id) = claims.user_id {
                return Ok(AuthContext::Admin { admin_id: user_id });
            }
            // Try to parse sub as UUID for admin
            if let Ok(admin_id) = Uuid::parse_str(&claims.sub) {
                return Ok(AuthContext::Admin { admin_id });
            }
        }

        // Service-to-service token
        if let Some(ref service_name) = claims.service_name {
            return Ok(AuthContext::Service {
                service_name: service_name.clone(),
                tenant_id: claims.tenant_id,
            });
        }

        // Regular user token
        let user_id = claims
            .user_id
            .or_else(|| Uuid::parse_str(&claims.sub).ok())
            .ok_or_else(|| anyhow!("token names no user (sub is not a UUID)"))?;

        let tenant_id = claims
            .tenant_id
            .ok_or_else(|| anyhow!("token names no tenant"))?;

        Ok(AuthContext::User {
            user_id,
            tenant_id,
            roles: claims.roles.clone(),
        })
    }
}

#[async_trait]
impl AuthProvider for WamiAuthProvider {
    async fn extract_context(&self, headers: &HeaderMap) -> Result<AuthContext> {
        let token = match Self::extract_bearer_token(headers) {
            Some(t) if !t.is_empty() => t,
            _ => {
                // No token — check if default policy allows anonymous
                if self.config.default_policy == "public" {
                    return Ok(AuthContext::Anonymous);
                }
                bail!("Missing or empty Authorization Bearer token");
            }
        };

        let token_data = self.verify_token(token)?;
        Self::claims_to_context(&token_data.claims)
    }

    async fn is_owner(
        &self,
        _user_id: &Uuid,
        _resource_id: &Uuid,
        _resource_type: &str,
    ) -> Result<bool> {
        // Ownership checks are typically done at the application level
        // by querying the data store. This is a placeholder.
        Ok(false)
    }

    async fn has_role(&self, _user_id: &Uuid, _role: &str) -> Result<bool> {
        // Role checks are done via the AuthContext extracted from the JWT.
        // This method would be used for out-of-band role checks (e.g., from DB).
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::auth::{AuthConfig, AuthProviderType, WamiConfig};
    use chrono::Utc;
    use ed25519_dalek::SigningKey;
    use jsonwebtoken::{EncodingKey, Header};

    /// Generate a fresh Ed25519 key pair for testing
    fn generate_test_keys() -> (SigningKey, VerifyingKey) {
        // Use two UUIDs to get 32 random bytes for the secret key
        let u1 = Uuid::new_v4();
        let u2 = Uuid::new_v4();
        let mut secret = [0u8; 32];
        secret[..16].copy_from_slice(u1.as_bytes());
        secret[16..].copy_from_slice(u2.as_bytes());
        let signing_key = SigningKey::from_bytes(&secret);
        let verifying_key = signing_key.verifying_key();
        (signing_key, verifying_key)
    }

    /// Create a PEM-encoded public key string from a VerifyingKey
    fn pub_key_to_pem(vk: &VerifyingKey) -> String {
        use ed25519_dalek::pkcs8::EncodePublicKey;
        vk.to_public_key_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
            .expect("failed to encode public key as PEM")
    }

    /// A token the STS issues must verify here, and audience must be enforced.
    /// Covers ADR D4: new tokens (with `aud`) and legacy tokens (without) both
    /// pass in the window; a foreign audience is refused once one is configured.
    #[test]
    fn sts_issued_token_verifies_here_and_audience_is_enforced() {
        use crate::core::auth::sts::{DEFAULT_AUDIENCE, StsService};
        use ed25519_dalek::pkcs8::EncodePrivateKey;

        let signing = SigningKey::from_bytes(&[9u8; 32]);
        let priv_pem = signing
            .to_pkcs8_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
            .expect("pkcs8 pem");
        let sts = StsService::new(&priv_pem, "test-issuer".to_string()).expect("sts");
        let pair = sts
            .issue_token_pair(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), vec![])
            .expect("issue");
        let pub_pem = pub_key_to_pem(&signing.verifying_key());

        // Audience not configured: any `aud` passes.
        let mut cfg = test_config(&pub_pem);
        cfg.wami.as_mut().unwrap().audience = None;
        // Sts mode refuses a missing audience; bootstrap tolerates it.
        assert!(WamiAuthProvider::from_config(&cfg).is_err());
        cfg.wami.as_mut().unwrap().mode = crate::config::auth::AuthMode::Bootstrap;
        let open = WamiAuthProvider::from_config(&cfg).expect("provider");
        assert!(open.verify_token(&pair.access_token).is_ok());

        // Audience configured to the STS audience: accepted.
        let mut cfg = test_config(&pub_pem);
        cfg.wami.as_mut().unwrap().audience = Some(DEFAULT_AUDIENCE.to_string());
        let matching = WamiAuthProvider::from_config(&cfg).expect("provider");
        assert!(matching.verify_token(&pair.access_token).is_ok());

        // Audience configured to another service: refused.
        let other = WamiAuthProvider::from_config(&test_config(&pub_pem)).expect("provider");
        assert!(other.verify_token(&pair.access_token).is_err());
    }

    /// ADR D4 on the verifier: a refresh token never authenticates a request;
    /// a token with no `aud` is accepted in the window and refused once it closes.
    #[test]
    fn legacy_window_and_refresh_tokens_on_the_verifier() {
        use crate::core::auth::sts::{DEFAULT_AUDIENCE, StsService};
        use ed25519_dalek::pkcs8::EncodePrivateKey;

        let signing = SigningKey::from_bytes(&[11u8; 32]);
        let priv_pem = signing
            .to_pkcs8_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
            .expect("pkcs8 pem");
        let sts = StsService::new(&priv_pem, "test-issuer".to_string()).expect("sts");
        let pair = sts
            .issue_token_pair(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), vec![])
            .expect("issue");
        let pub_pem = pub_key_to_pem(&signing.verifying_key());

        let mut cfg = test_config(&pub_pem);
        cfg.wami.as_mut().unwrap().audience = Some(DEFAULT_AUDIENCE.to_string());
        // Sts mode: the window is closed by default; a migration opens it explicitly.
        let strict = WamiAuthProvider::from_config(&cfg).expect("provider");
        assert!(!strict.accept_legacy_tokens);
        cfg.wami.as_mut().unwrap().accept_legacy_tokens = Some(true);
        let lenient = WamiAuthProvider::from_config(&cfg).expect("provider");
        assert!(lenient.accept_legacy_tokens);

        // Refresh tokens never authenticate, whatever the window.
        assert!(lenient.verify_token(&pair.refresh_token).is_err());
        assert!(strict.verify_token(&pair.refresh_token).is_err());
        assert!(lenient.verify_token(&pair.access_token).is_ok());

        // A token with no aud: accepted in the window, refused once it closes.
        let mut legacy = valid_claims();
        legacy.aud = None;
        let legacy_token = sign_token(&legacy, &signing);
        assert!(lenient.verify_token(&legacy_token).is_ok());
        assert!(strict.verify_token(&legacy_token).is_err());
    }

    /// Create an AuthConfig with the given public key
    fn test_config(pub_key_b64: &str) -> AuthConfig {
        AuthConfig {
            provider: AuthProviderType::Wami,
            wami: Some(WamiConfig {
                public_key: Some(pub_key_b64.to_string()),
                issuer: Some("test-issuer".to_string()),
                audience: Some("test-audience".to_string()),
                mode: Default::default(),
                private_key: None,
                token_ttl_secs: 3600,
                refresh_ttl_secs: 86400,
                accept_legacy_tokens: None,
                policies_path: None,
                identity_path: None,
            }),
            ..Default::default()
        }
    }

    /// Sign claims into a JWT using an Ed25519 signing key
    fn sign_token(claims: &StsClaims, signing_key: &SigningKey) -> String {
        use ed25519_dalek::pkcs8::EncodePrivateKey;

        let header = Header::new(Algorithm::EdDSA);
        // jsonwebtoken expects PKCS8 DER format for Ed25519
        let pkcs8_der = signing_key
            .to_pkcs8_der()
            .expect("failed to encode signing key as PKCS8 DER");
        let encoding_key = EncodingKey::from_ed_der(pkcs8_der.as_bytes());
        jsonwebtoken::encode(&header, claims, &encoding_key).expect("failed to sign token")
    }

    /// Create standard valid claims
    fn valid_claims() -> StsClaims {
        let now = Utc::now().timestamp() as u64;
        StsClaims {
            sub: Uuid::new_v4().to_string(),
            iss: Some("test-issuer".to_string()),
            aud: Some("test-audience".to_string()),
            exp: now + 3600,
            iat: Some(now),
            nbf: Some(now - 10),
            tenant_id: Some(Uuid::new_v4()),
            user_id: Some(Uuid::new_v4()),
            roles: vec!["editor".to_string(), "viewer".to_string()],
            principal_arn: Some("arn:wami:iam::user/alice".to_string()),
            scoped_policies: vec![],
            service_name: None,
            is_admin: false,
        }
    }

    // --- Construction ---

    #[test]
    fn test_from_config_valid() {
        let (_sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_from_config_missing_wami_section() {
        let config = AuthConfig {
            provider: AuthProviderType::Wami,
            wami: None,
            ..Default::default()
        };
        let err = WamiAuthProvider::from_config(&config).unwrap_err();
        assert!(err.to_string().contains("wami section is required"));
    }

    #[test]
    fn test_from_config_empty_public_key() {
        let config = AuthConfig {
            provider: AuthProviderType::Wami,
            wami: Some(WamiConfig {
                public_key: Some(String::new()),
                issuer: None,
                audience: None,
                mode: Default::default(),
                private_key: None,
                token_ttl_secs: 3600,
                refresh_ttl_secs: 86400,
                accept_legacy_tokens: None,
                policies_path: None,
                identity_path: None,
            }),
            ..Default::default()
        };
        let err = WamiAuthProvider::from_config(&config).unwrap_err();
        assert!(err.to_string().contains("public_key is required"));
    }

    #[test]
    fn test_from_config_invalid_key_string() {
        let config = test_config("not-valid-pem-or-base64!!!");
        let err = WamiAuthProvider::from_config(&config).unwrap_err();
        assert!(err.to_string().contains("public key") || err.to_string().contains("base64"));
    }

    #[test]
    fn test_from_config_wrong_key_length() {
        use base64::Engine;
        let short_key = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        let config = test_config(&short_key);
        let err = WamiAuthProvider::from_config(&config).unwrap_err();
        // Should fail because 16 bytes != 32 bytes expected for Ed25519
        assert!(
            err.to_string().contains("32 bytes") || err.to_string().contains("public key"),
            "Unexpected error: {}",
            err
        );
    }

    // --- Token verification ---

    #[test]
    fn test_verify_valid_token() {
        let (sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let claims = valid_claims();
        let token = sign_token(&claims, &sk);

        let result = provider.verify_token(&token);
        assert!(result.is_ok(), "verify_token failed: {:?}", result.err());
        let decoded = result.unwrap();
        assert_eq!(decoded.claims.sub, claims.sub);
        assert_eq!(decoded.claims.tenant_id, claims.tenant_id);
        assert_eq!(decoded.claims.roles, claims.roles);
    }

    #[test]
    fn test_verify_expired_token() {
        let (sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let mut claims = valid_claims();
        claims.exp = 1000; // Expired in 1970
        let token = sign_token(&claims, &sk);

        let err = provider.verify_token(&token).unwrap_err();
        let err_msg = err.to_string().to_lowercase();
        assert!(
            err_msg.contains("exp")
                || err_msg.contains("expired")
                || err_msg.contains("verification failed"),
            "Expected expiration error, got: {}",
            err
        );
    }

    #[test]
    fn test_verify_wrong_signature() {
        let (sk, _vk) = generate_test_keys();
        let (_sk2, vk2) = generate_test_keys(); // Different key pair

        let config = test_config(&pub_key_to_pem(&vk2));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let claims = valid_claims();
        let token = sign_token(&claims, &sk); // Signed with sk, verifying with vk2

        let err = provider.verify_token(&token).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn test_verify_wrong_issuer() {
        let (sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let mut claims = valid_claims();
        claims.iss = Some("wrong-issuer".to_string());
        let token = sign_token(&claims, &sk);

        let err = provider.verify_token(&token).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn test_verify_wrong_audience() {
        let (sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let mut claims = valid_claims();
        claims.aud = Some("wrong-audience".to_string());
        let token = sign_token(&claims, &sk);

        let err = provider.verify_token(&token).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn test_verify_nbf_future() {
        let (sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let mut claims = valid_claims();
        claims.nbf = Some(Utc::now().timestamp() as u64 + 9999);
        let token = sign_token(&claims, &sk);

        let err = provider.verify_token(&token).unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    // --- Malformed tokens ---

    #[test]
    fn test_verify_empty_token() {
        let (_sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let err = provider.verify_token("").unwrap_err();
        assert!(err.to_string().contains("Malformed JWT"));
    }

    #[test]
    fn test_verify_single_segment() {
        let (_sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let err = provider.verify_token("just-one-segment").unwrap_err();
        assert!(err.to_string().contains("Malformed JWT"));
    }

    #[test]
    fn test_verify_two_segments() {
        let (_sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let err = provider.verify_token("two.segments").unwrap_err();
        assert!(err.to_string().contains("Malformed JWT"));
    }

    #[test]
    fn test_verify_garbage_three_segments() {
        let (_sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let err = provider.verify_token("not.valid.jwt").unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }

    // --- Claims to AuthContext mapping ---

    #[test]
    fn test_claims_to_context_user() {
        let user_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4();
        let claims = StsClaims {
            sub: user_id.to_string(),
            user_id: Some(user_id),
            tenant_id: Some(tenant_id),
            roles: vec!["admin".to_string()],
            is_admin: false,
            service_name: None,
            ..valid_claims()
        };

        let ctx = WamiAuthProvider::claims_to_context(&claims).unwrap();
        match ctx {
            AuthContext::User {
                user_id: uid,
                tenant_id: tid,
                roles,
            } => {
                assert_eq!(uid, user_id);
                assert_eq!(tid, tenant_id);
                assert_eq!(roles, vec!["admin".to_string()]);
            }
            other => panic!("Expected User context, got {:?}", other),
        }
    }

    #[test]
    fn test_claims_to_context_admin() {
        let admin_id = Uuid::new_v4();
        let claims = StsClaims {
            sub: admin_id.to_string(),
            user_id: Some(admin_id),
            is_admin: true,
            ..valid_claims()
        };

        let ctx = WamiAuthProvider::claims_to_context(&claims).unwrap();
        match ctx {
            AuthContext::Admin { admin_id: aid } => {
                assert_eq!(aid, admin_id);
            }
            other => panic!("Expected Admin context, got {:?}", other),
        }
    }

    #[test]
    fn test_claims_to_context_service() {
        let tenant_id = Uuid::new_v4();
        let claims = StsClaims {
            sub: "billing-service".to_string(),
            service_name: Some("billing".to_string()),
            tenant_id: Some(tenant_id),
            is_admin: false,
            ..valid_claims()
        };

        let ctx = WamiAuthProvider::claims_to_context(&claims).unwrap();
        match ctx {
            AuthContext::Service {
                service_name,
                tenant_id: tid,
            } => {
                assert_eq!(service_name, "billing");
                assert_eq!(tid, Some(tenant_id));
            }
            other => panic!("Expected Service context, got {:?}", other),
        }
    }

    #[test]
    fn test_claims_to_context_missing_user_id_falls_back_to_sub() {
        let sub_uuid = Uuid::new_v4();
        let claims = StsClaims {
            sub: sub_uuid.to_string(),
            user_id: None,
            tenant_id: Some(Uuid::new_v4()),
            is_admin: false,
            service_name: None,
            ..valid_claims()
        };

        let ctx = WamiAuthProvider::claims_to_context(&claims).unwrap();
        match ctx {
            AuthContext::User { user_id, .. } => {
                assert_eq!(user_id, sub_uuid);
            }
            other => panic!("Expected User context, got {:?}", other),
        }
    }

    /// A token that names no tenant is refused. It is never mapped to the nil
    /// UUID, which the identity registry would give a real segment (ADR D1).
    #[test]
    fn test_claims_to_context_missing_tenant_id_is_refused() {
        let claims = StsClaims {
            sub: Uuid::new_v4().to_string(),
            user_id: Some(Uuid::new_v4()),
            tenant_id: None,
            is_admin: false,
            service_name: None,
            ..valid_claims()
        };
        let err = WamiAuthProvider::claims_to_context(&claims).unwrap_err();
        assert!(err.to_string().contains("no tenant"), "got: {err}");
    }

    #[test]
    fn test_claims_to_context_unnamed_user_is_refused() {
        let claims = StsClaims {
            sub: "not-a-uuid".to_string(),
            user_id: None,
            tenant_id: Some(Uuid::new_v4()),
            is_admin: false,
            service_name: None,
            ..valid_claims()
        };
        assert!(WamiAuthProvider::claims_to_context(&claims).is_err());
    }

    // --- extract_context via AuthProvider trait ---

    #[tokio::test]
    async fn test_extract_context_no_token_public_policy() {
        let (_sk, vk) = generate_test_keys();
        let mut config = test_config(&pub_key_to_pem(&vk));
        config.default_policy = "public".to_string();
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let headers = HeaderMap::new();
        let ctx = provider.extract_context(&headers).await.unwrap();
        assert!(matches!(ctx, AuthContext::Anonymous));
    }

    #[tokio::test]
    async fn test_extract_context_no_token_authenticated_policy() {
        let (_sk, vk) = generate_test_keys();
        let mut config = test_config(&pub_key_to_pem(&vk));
        config.default_policy = "authenticated".to_string();
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let headers = HeaderMap::new();
        let err = provider.extract_context(&headers).await.unwrap_err();
        assert!(err.to_string().contains("Missing"));
    }

    #[tokio::test]
    async fn test_extract_context_valid_bearer() {
        let (sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let claims = valid_claims();
        let user_id = claims.user_id.unwrap();
        let token = sign_token(&claims, &sk);

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());

        let ctx = provider.extract_context(&headers).await.unwrap();
        match ctx {
            AuthContext::User {
                user_id: uid,
                roles,
                ..
            } => {
                assert_eq!(uid, user_id);
                assert_eq!(roles, vec!["editor".to_string(), "viewer".to_string()]);
            }
            other => panic!("Expected User context, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_extract_context_empty_bearer() {
        let (_sk, vk) = generate_test_keys();
        let mut config = test_config(&pub_key_to_pem(&vk));
        config.default_policy = "authenticated".to_string();
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer ".parse().unwrap());

        let err = provider.extract_context(&headers).await.unwrap_err();
        assert!(err.to_string().contains("Missing"));
    }

    #[tokio::test]
    async fn test_extract_context_invalid_token() {
        let (_sk, vk) = generate_test_keys();
        let config = test_config(&pub_key_to_pem(&vk));
        let provider = WamiAuthProvider::from_config(&config).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            "Bearer invalid.token.here".parse().unwrap(),
        );

        let err = provider.extract_context(&headers).await.unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }
}
