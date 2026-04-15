//! STS (Security Token Service) for JWT token issuance and management
//!
//! Provides token generation, refresh, and revocation.
//! Uses Ed25519 keys for JWT signing (same keys as WamiAuthProvider verification).

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, encode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// JWT claims for this-rs STS tokens
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StsClaims {
    /// Subject (principal / user_id)
    pub sub: String,
    /// Tenant ID
    pub tenant_id: Uuid,
    /// Roles granted to the subject
    pub roles: Vec<String>,
    /// Expiration time (Unix timestamp)
    pub exp: i64,
    /// Issued-at time (Unix timestamp)
    pub iat: i64,
    /// Unique JWT ID (used for revocation)
    pub jti: String,
    /// Issuer
    pub iss: String,
}

/// Token pair returned by login / refresh
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
}

/// Login credentials submitted to POST /auth/token
#[derive(Debug, Deserialize)]
pub struct LoginCredentials {
    pub username: String,
    pub password: String,
}

/// Revoke request body for POST /auth/revoke
#[derive(Debug, Deserialize)]
pub struct RevokeRequest {
    pub jti: String,
}

/// Refresh request body for POST /auth/refresh
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Internal user record for the simple built-in store
struct UserRecord {
    user_id: Uuid,
    tenant_id: Uuid,
    /// For demo/testing: plaintext comparison (real deployments use WAMI IdP)
    password_hash: String,
    roles: Vec<String>,
}

/// Security Token Service
///
/// Issues, refreshes, and revokes Ed25519-signed JWT tokens.
pub struct StsService {
    encoding_key: EncodingKey,
    issuer: String,
    access_token_ttl: Duration,
    refresh_token_ttl: Duration,
    revoked_tokens: Arc<RwLock<HashSet<String>>>,
    users: Arc<RwLock<HashMap<String, UserRecord>>>,
}

impl StsService {
    /// Create a new STS service.
    ///
    /// `private_key_pem` must be a PEM-encoded Ed25519 private key (PKCS8).
    pub fn new(private_key_pem: &str, issuer: String) -> Result<Self> {
        let encoding_key = EncodingKey::from_ed_pem(private_key_pem.as_bytes())
            .context("Failed to parse Ed25519 private key PEM")?;

        Ok(Self {
            encoding_key,
            issuer,
            access_token_ttl: Duration::minutes(15),
            refresh_token_ttl: Duration::hours(24),
            revoked_tokens: Arc::new(RwLock::new(HashSet::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Set the access token TTL in seconds
    pub fn set_access_ttl_secs(&mut self, secs: u64) {
        self.access_token_ttl = Duration::seconds(secs as i64);
    }

    /// Set the refresh token TTL in seconds
    pub fn set_refresh_ttl_secs(&mut self, secs: u64) {
        self.refresh_token_ttl = Duration::seconds(secs as i64);
    }

    /// Issue a token pair for valid credentials.
    pub fn login(&self, credentials: &LoginCredentials) -> Result<TokenPair> {
        let users = self
            .users
            .read()
            .map_err(|_| anyhow::anyhow!("users lock poisoned"))?;

        let user = users
            .get(&credentials.username)
            .ok_or_else(|| anyhow::anyhow!("invalid credentials"))?;

        if user.password_hash != credentials.password {
            bail!("invalid credentials");
        }

        let user_id = user.user_id;
        let tenant_id = user.tenant_id;
        let roles = user.roles.clone();
        drop(users);

        self.issue_token_pair(user_id, tenant_id, roles)
    }

    /// Refresh an access token using a valid refresh token.
    ///
    /// The caller must supply the `DecodingKey` that corresponds to the
    /// signing key so the refresh token can be verified before re-issue.
    pub fn refresh(&self, refresh_token: &str, decoding_key: &DecodingKey) -> Result<TokenPair> {
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience::<&str>(&[]);
        validation.validate_exp = true;

        let token_data =
            jsonwebtoken::decode::<StsClaims>(refresh_token, decoding_key, &validation)
                .context("refresh token verification failed")?;

        let claims = token_data.claims;

        // Check revocation
        if self.is_revoked(&claims.jti) {
            bail!("refresh token has been revoked");
        }

        // Revoke the old refresh token (rotation)
        self.revoke(&claims.jti);

        let user_id = Uuid::parse_str(&claims.sub).context("invalid sub claim in refresh token")?;

        self.issue_token_pair(user_id, claims.tenant_id, claims.roles)
    }

    /// Revoke a token by its JTI.
    pub fn revoke(&self, jti: &str) {
        if let Ok(mut set) = self.revoked_tokens.write() {
            set.insert(jti.to_string());
        }
    }

    /// Check whether a token JTI has been revoked.
    pub fn is_revoked(&self, jti: &str) -> bool {
        self.revoked_tokens
            .read()
            .map(|set| set.contains(jti))
            .unwrap_or(false)
    }

    /// Register a user (for bootstrapping / testing).
    pub fn register_user(
        &self,
        username: String,
        password: String,
        user_id: Uuid,
        tenant_id: Uuid,
        roles: Vec<String>,
    ) {
        if let Ok(mut users) = self.users.write() {
            users.insert(
                username,
                UserRecord {
                    user_id,
                    tenant_id,
                    password_hash: password,
                    roles,
                },
            );
        }
    }

    /// Build a JWKS (JSON Web Key Set) response from a PEM-encoded Ed25519
    /// public key.
    ///
    /// Returns a JSON value with the standard `{ "keys": [ ... ] }` shape,
    /// using key type `OKP` and curve `Ed25519`.
    pub fn jwks_response(public_key_pem: &str) -> Result<serde_json::Value> {
        use base64::Engine;
        use ed25519_dalek::VerifyingKey;
        use ed25519_dalek::pkcs8::DecodePublicKey;

        let vk = VerifyingKey::from_public_key_pem(public_key_pem)
            .context("Failed to parse Ed25519 public key PEM for JWKS")?;

        let x = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(vk.as_bytes());

        Ok(serde_json::json!({
            "keys": [
                {
                    "kty": "OKP",
                    "crv": "Ed25519",
                    "use": "sig",
                    "alg": "EdDSA",
                    "x": x,
                }
            ]
        }))
    }

    // ---- private helpers ----

    fn issue_token_pair(
        &self,
        user_id: Uuid,
        tenant_id: Uuid,
        roles: Vec<String>,
    ) -> Result<TokenPair> {
        let now = Utc::now();
        let access_exp = now + self.access_token_ttl;
        let refresh_exp = now + self.refresh_token_ttl;

        let access_jti = Uuid::new_v4().to_string();
        let refresh_jti = Uuid::new_v4().to_string();

        let header = Header::new(Algorithm::EdDSA);

        let access_claims = StsClaims {
            sub: user_id.to_string(),
            tenant_id,
            roles: roles.clone(),
            exp: access_exp.timestamp(),
            iat: now.timestamp(),
            jti: access_jti,
            iss: self.issuer.clone(),
        };

        let refresh_claims = StsClaims {
            sub: user_id.to_string(),
            tenant_id,
            roles,
            exp: refresh_exp.timestamp(),
            iat: now.timestamp(),
            jti: refresh_jti,
            iss: self.issuer.clone(),
        };

        let access_token = encode(&header, &access_claims, &self.encoding_key)
            .context("failed to sign access token")?;

        let refresh_token = encode(&header, &refresh_claims, &self.encoding_key)
            .context("failed to sign refresh token")?;

        Ok(TokenPair {
            access_token,
            refresh_token,
            expires_in: self.access_token_ttl.num_seconds(),
            token_type: "Bearer".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
    use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};

    /// Generate a fresh Ed25519 key pair and return (private PEM, public PEM)
    fn generate_test_keypair() -> (String, String) {
        let u1 = Uuid::new_v4();
        let u2 = Uuid::new_v4();
        let mut secret = [0u8; 32];
        secret[..16].copy_from_slice(u1.as_bytes());
        secret[16..].copy_from_slice(u2.as_bytes());

        let signing_key = SigningKey::from_bytes(&secret);
        let verifying_key = signing_key.verifying_key();

        let private_pem = signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode private key PEM");
        let public_pem = verifying_key
            .to_public_key_pem(LineEnding::LF)
            .expect("encode public key PEM");

        (private_pem.to_string(), public_pem)
    }

    fn build_sts(private_pem: &str) -> StsService {
        StsService::new(private_pem, "test-issuer".to_string()).expect("StsService::new")
    }

    fn decoding_key(public_pem: &str) -> DecodingKey {
        DecodingKey::from_ed_pem(public_pem.as_bytes()).expect("DecodingKey from PEM")
    }

    // ---- login ----

    #[test]
    fn test_login_success() {
        let (priv_pem, _pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);

        let uid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        sts.register_user(
            "alice".into(),
            "secret".into(),
            uid,
            tid,
            vec!["admin".into()],
        );

        let pair = sts
            .login(&LoginCredentials {
                username: "alice".into(),
                password: "secret".into(),
            })
            .expect("login should succeed");

        assert_eq!(pair.token_type, "Bearer");
        assert!(!pair.access_token.is_empty());
        assert!(!pair.refresh_token.is_empty());
        assert!(pair.expires_in > 0);
    }

    #[test]
    fn test_login_wrong_password() {
        let (priv_pem, _) = generate_test_keypair();
        let sts = build_sts(&priv_pem);

        sts.register_user(
            "alice".into(),
            "secret".into(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            vec![],
        );

        let err = sts
            .login(&LoginCredentials {
                username: "alice".into(),
                password: "wrong".into(),
            })
            .unwrap_err();
        assert!(err.to_string().contains("invalid credentials"));
    }

    #[test]
    fn test_login_unknown_user() {
        let (priv_pem, _) = generate_test_keypair();
        let sts = build_sts(&priv_pem);

        let err = sts
            .login(&LoginCredentials {
                username: "nobody".into(),
                password: "x".into(),
            })
            .unwrap_err();
        assert!(err.to_string().contains("invalid credentials"));
    }

    // ---- refresh ----

    #[test]
    fn test_refresh_success() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let dk = decoding_key(&pub_pem);

        sts.register_user(
            "bob".into(),
            "pass".into(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            vec!["viewer".into()],
        );

        let pair = sts
            .login(&LoginCredentials {
                username: "bob".into(),
                password: "pass".into(),
            })
            .unwrap();

        let new_pair = sts
            .refresh(&pair.refresh_token, &dk)
            .expect("refresh should succeed");
        assert!(!new_pair.access_token.is_empty());
        assert_ne!(new_pair.access_token, pair.access_token);
    }

    #[test]
    fn test_refresh_rotates_token() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let dk = decoding_key(&pub_pem);

        sts.register_user(
            "carol".into(),
            "pw".into(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            vec![],
        );

        let pair = sts
            .login(&LoginCredentials {
                username: "carol".into(),
                password: "pw".into(),
            })
            .unwrap();

        // First refresh succeeds
        let _new = sts.refresh(&pair.refresh_token, &dk).unwrap();

        // Second refresh with same token fails (rotated / revoked)
        let err = sts.refresh(&pair.refresh_token, &dk).unwrap_err();
        assert!(err.to_string().contains("revoked"));
    }

    #[test]
    fn test_refresh_invalid_token() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let dk = decoding_key(&pub_pem);

        let err = sts.refresh("not.a.valid.jwt", &dk).unwrap_err();
        assert!(
            err.to_string().contains("verification failed") || err.to_string().contains("failed")
        );
    }

    // ---- revocation ----

    #[test]
    fn test_revoke_and_is_revoked() {
        let (priv_pem, _) = generate_test_keypair();
        let sts = build_sts(&priv_pem);

        let jti = "test-jti-123";
        assert!(!sts.is_revoked(jti));

        sts.revoke(jti);
        assert!(sts.is_revoked(jti));
    }

    #[test]
    fn test_is_revoked_unknown_jti() {
        let (priv_pem, _) = generate_test_keypair();
        let sts = build_sts(&priv_pem);

        assert!(!sts.is_revoked("nonexistent"));
    }

    // ---- JWKS ----

    #[test]
    fn test_jwks_response_format() {
        let (_priv_pem, pub_pem) = generate_test_keypair();
        let jwks = StsService::jwks_response(&pub_pem).expect("jwks_response should succeed");

        assert!(jwks.get("keys").is_some());
        let keys = jwks["keys"].as_array().expect("keys should be an array");
        assert_eq!(keys.len(), 1);

        let key = &keys[0];
        assert_eq!(key["kty"], "OKP");
        assert_eq!(key["crv"], "Ed25519");
        assert_eq!(key["use"], "sig");
        assert_eq!(key["alg"], "EdDSA");
        assert!(key["x"].as_str().is_some());
        // Ed25519 public key is 32 bytes -> 43 base64url chars (no padding)
        let x_len = key["x"].as_str().unwrap().len();
        assert_eq!(x_len, 43);
    }

    #[test]
    fn test_jwks_response_invalid_pem() {
        let err = StsService::jwks_response("not a pem").unwrap_err();
        assert!(err.to_string().contains("public key"));
    }

    // ---- token claims verification ----

    #[test]
    fn test_issued_token_verifiable() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let dk = decoding_key(&pub_pem);

        let uid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        sts.register_user("dave".into(), "pw".into(), uid, tid, vec!["editor".into()]);

        let pair = sts
            .login(&LoginCredentials {
                username: "dave".into(),
                password: "pw".into(),
            })
            .unwrap();

        // Verify the access token
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&["test-issuer"]);
        validation.set_audience::<&str>(&[]);

        let token_data = jsonwebtoken::decode::<StsClaims>(&pair.access_token, &dk, &validation)
            .expect("access token should verify");

        assert_eq!(token_data.claims.sub, uid.to_string());
        assert_eq!(token_data.claims.tenant_id, tid);
        assert_eq!(token_data.claims.roles, vec!["editor".to_string()]);
        assert_eq!(token_data.claims.iss, "test-issuer");
        assert!(!token_data.claims.jti.is_empty());
    }
}
