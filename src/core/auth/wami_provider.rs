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

        if wami_config.public_key.is_empty() {
            bail!("wami.public_key is required (cannot be empty)");
        }

        let decoding_key = Self::parse_public_key(&wami_config.public_key)
            .context("Failed to parse Ed25519 public key")?;

        let mut validation = Validation::new(Algorithm::EdDSA);

        // Configure issuer validation
        if let Some(ref issuer) = wami_config.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        } else {
            // Don't require issuer if not configured
            validation.set_issuer::<&str>(&[]);
        }

        // Configure audience validation
        if let Some(ref audience) = wami_config.audience {
            validation.set_audience(&[audience.as_str()]);
        } else {
            // Don't require audience if not configured
            validation.set_audience::<&str>(&[]);
        }

        // Always validate expiration
        validation.validate_exp = true;
        // Validate nbf if present
        validation.validate_nbf = true;

        Ok(Self {
            decoding_key,
            validation,
            config: config.clone(),
        })
    }

    /// Parse an Ed25519 public key from base64 or PEM format
    fn parse_public_key(key_str: &str) -> Result<DecodingKey> {
        let key_str = key_str.trim();

        if key_str.starts_with("-----BEGIN") {
            // PEM format
            DecodingKey::from_ed_pem(key_str.as_bytes())
                .context("Invalid Ed25519 PEM public key")
        } else {
            // Raw base64 — decode to 32-byte Ed25519 public key
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(key_str)
                .or_else(|_| {
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(key_str)
                })
                .context("Public key is not valid base64")?;

            if bytes.len() != 32 {
                bail!(
                    "Ed25519 public key must be 32 bytes, got {}",
                    bytes.len()
                );
            }

            // Validate the key is a valid Ed25519 point
            let key_bytes: [u8; 32] = bytes.as_slice().try_into().unwrap();
            let verify_key = VerifyingKey::from_bytes(&key_bytes)
                .context("Invalid Ed25519 public key bytes")?;

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
            bail!("Malformed JWT: expected 3 segments separated by dots, got {}", dot_count + 1);
        }

        jsonwebtoken::decode::<StsClaims>(token, &self.decoding_key, &self.validation)
            .context("JWT verification failed")
    }

    /// Extract Bearer token from Authorization header
    fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
        headers
            .get(axum::http::header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")
    }

    /// Map verified StsClaims into an AuthContext
    fn claims_to_context(claims: &StsClaims) -> AuthContext {
        // Admin token
        if claims.is_admin {
            if let Some(user_id) = claims.user_id {
                return AuthContext::Admin { admin_id: user_id };
            }
            // Try to parse sub as UUID for admin
            if let Ok(admin_id) = Uuid::parse_str(&claims.sub) {
                return AuthContext::Admin { admin_id };
            }
        }

        // Service-to-service token
        if let Some(ref service_name) = claims.service_name {
            return AuthContext::Service {
                service_name: service_name.clone(),
                tenant_id: claims.tenant_id,
            };
        }

        // Regular user token
        let user_id = claims
            .user_id
            .or_else(|| Uuid::parse_str(&claims.sub).ok())
            .unwrap_or_else(Uuid::nil);

        let tenant_id = claims.tenant_id.unwrap_or_else(Uuid::nil);

        AuthContext::User {
            user_id,
            tenant_id,
            roles: claims.roles.clone(),
        }
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
        Ok(Self::claims_to_context(&token_data.claims))
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
    use ed25519_dalek::SigningKey;
    use jsonwebtoken::{EncodingKey, Header};
    use chrono::Utc;

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

    /// Create an AuthConfig with the given public key
    fn test_config(pub_key_b64: &str) -> AuthConfig {
        let mut config = AuthConfig::default();
        config.provider = AuthProviderType::Wami;
        config.wami = Some(WamiConfig {
            public_key: pub_key_b64.to_string(),
            issuer: Some("test-issuer".to_string()),
            audience: Some("test-audience".to_string()),
            mode: Default::default(),
        });
        config
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
        let mut config = AuthConfig::default();
        config.provider = AuthProviderType::Wami;
        config.wami = None;
        let err = WamiAuthProvider::from_config(&config).unwrap_err();
        assert!(err.to_string().contains("wami section is required"));
    }

    #[test]
    fn test_from_config_empty_public_key() {
        let mut config = AuthConfig::default();
        config.provider = AuthProviderType::Wami;
        config.wami = Some(WamiConfig {
            public_key: String::new(),
            issuer: None,
            audience: None,
            mode: Default::default(),
        });
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
        let short_key = base64::engine::general_purpose::STANDARD.encode(&[0u8; 16]);
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
            err_msg.contains("exp") || err_msg.contains("expired") || err_msg.contains("verification failed"),
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

        let ctx = WamiAuthProvider::claims_to_context(&claims);
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

        let ctx = WamiAuthProvider::claims_to_context(&claims);
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

        let ctx = WamiAuthProvider::claims_to_context(&claims);
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

        let ctx = WamiAuthProvider::claims_to_context(&claims);
        match ctx {
            AuthContext::User { user_id, .. } => {
                assert_eq!(user_id, sub_uuid);
            }
            other => panic!("Expected User context, got {:?}", other),
        }
    }

    #[test]
    fn test_claims_to_context_missing_tenant_id_defaults_to_nil() {
        let claims = StsClaims {
            sub: Uuid::new_v4().to_string(),
            user_id: Some(Uuid::new_v4()),
            tenant_id: None,
            is_admin: false,
            service_name: None,
            ..valid_claims()
        };

        let ctx = WamiAuthProvider::claims_to_context(&claims);
        match ctx {
            AuthContext::User { tenant_id, .. } => {
                assert!(tenant_id.is_nil());
            }
            other => panic!("Expected User context, got {:?}", other),
        }
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
        headers.insert("Authorization", "Bearer invalid.token.here".parse().unwrap());

        let err = provider.extract_context(&headers).await.unwrap_err();
        assert!(err.to_string().contains("verification failed"));
    }
}
