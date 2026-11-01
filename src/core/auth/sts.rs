//! STS (Security Token Service) for JWT token issuance and management
//!
//! Provides token generation, refresh, and revocation.
//! Uses Ed25519 keys for JWT signing (same keys as WamiAuthProvider verification).

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
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
    /// Audience the token is addressed to (RFC 7519 §4.1.3).
    ///
    /// Absent on tokens issued before the audience was introduced: those are
    /// accepted only while [`StsService::accepts_legacy_tokens`] holds (ADR D4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// `"access"` or `"refresh"`: what the token may be used for (ADR D4).
    ///
    /// A refresh token must not authenticate a request, and an access token must
    /// not be exchanged for a new pair. Absent on tokens issued before this claim
    /// existed, which are accepted only while the legacy window holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_use: Option<String>,
}

/// The `token_use` value of an access token.
pub const TOKEN_USE_ACCESS: &str = "access";
/// The `token_use` value of a refresh token.
pub const TOKEN_USE_REFRESH: &str = "refresh";

/// Audience this STS issues for when none is configured.
pub const DEFAULT_AUDIENCE: &str = "this-rs";

/// RFC 7638 thumbprint of an Ed25519 public key: the `kid` wami's `KeyManager`
/// gives the same key. Computed from the public bytes alone so a verifier that
/// holds only the public key can name the key a token was signed with.
pub fn kid_for_public_key(public: &[u8; 32]) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let x = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public);
    let canonical = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
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

/// Une identité vérifiée par un annuaire externe — ce que `login` a besoin
/// de savoir pour émettre un jeton, rien de plus.
#[derive(Debug, Clone)]
pub struct IdentiteVerifiee {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub roles: Vec<String>,
}

/// Contrat de vérification d'identités déléguée (chantier STS-PERSISTANT).
///
/// Quand un annuaire est branché sur le STS ([`StsService::set_annuaire`]),
/// il devient la **seule** source de vérité du login : l'annuaire en mémoire
/// historique (comparaison en clair, réservé aux tests) n'est plus consulté.
/// Implémenté par `storage::annuaire_identites::AnnuaireIdentites` (store
/// obrain dédié, hash argon2id).
pub trait AnnuaireVerification: Send + Sync {
    /// Rend l'identité si — et seulement si — le couple login/mot de passe
    /// est valide. `None` couvre indistinctement « inconnu » et « mauvais
    /// mot de passe » (on ne renseigne pas un attaquant).
    fn verifier_identite(&self, username: &str, mot_de_passe: &str) -> Option<IdentiteVerifiee>;
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
    /// Signs every token; its active `kid` goes in the JOSE header.
    keys: wami::wami::sts::jwt::KeyManager,
    issuer: String,
    audience: String,
    /// ADR D4: tokens without `aud` (issued before the audience existed) are
    /// accepted while this holds. Flip to `false` once every client reissued.
    accept_legacy_tokens: bool,
    access_token_ttl: Duration,
    refresh_token_ttl: Duration,
    revoked_tokens: Arc<RwLock<HashSet<String>>>,
    users: Arc<RwLock<HashMap<String, UserRecord>>>,
    /// L'annuaire persisté, quand il est branché : il fait alors foi seul.
    ///
    /// `OnceLock` et non `Option` : il se pose à travers une référence partagée
    /// (voir `set_annuaire`), une fois, au démarrage.
    annuaire: std::sync::OnceLock<Arc<dyn AnnuaireVerification>>,
}

impl StsService {
    /// Create a new STS service.
    ///
    /// `private_key_pem` must be a PEM-encoded Ed25519 private key (PKCS8).
    pub fn new(private_key_pem: &str, issuer: String) -> Result<Self> {
        use ed25519_dalek::SigningKey;
        use ed25519_dalek::pkcs8::DecodePrivateKey;

        // PKCS8 is parsed once here, into the bytes `KeyManager` derives its
        // `kid` from. Both the STS and wami's verifiers therefore name the key
        // the same way, which the rotation path depends on.
        let signing = SigningKey::from_pkcs8_pem(private_key_pem)
            .context("Failed to parse Ed25519 private key PEM (PKCS8)")?;
        let keys = wami::wami::sts::jwt::KeyManager::from_bytes(&signing.to_bytes());

        Ok(Self {
            keys,
            issuer,
            audience: DEFAULT_AUDIENCE.to_string(),
            accept_legacy_tokens: true,
            access_token_ttl: Duration::minutes(15),
            refresh_token_ttl: Duration::hours(24),
            revoked_tokens: Arc::new(RwLock::new(HashSet::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
            annuaire: std::sync::OnceLock::new(),
        })
    }

    /// Set the audience this STS issues for and accepts on refresh.
    #[must_use]
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = audience.into();
        self
    }

    /// Whether tokens without `aud` are still accepted (ADR D4 window).
    #[must_use]
    pub fn with_legacy_tokens(mut self, accept: bool) -> Self {
        self.accept_legacy_tokens = accept;
        self
    }

    /// The audience this STS issues for.
    pub fn audience(&self) -> &str {
        &self.audience
    }

    /// Whether a token with no `aud` is accepted. See ADR D4.
    pub fn accepts_legacy_tokens(&self) -> bool {
        self.accept_legacy_tokens
    }

    /// Check a token's `aud` against this STS.
    ///
    /// Done by hand because `jsonwebtoken` refuses any `aud` when no expected
    /// audience is set, which would reject every token this STS issues.
    pub fn check_audience(&self, aud: Option<&str>) -> Result<()> {
        match aud {
            Some(a) if a == self.audience => Ok(()),
            None if self.accept_legacy_tokens => Ok(()),
            Some(a) => bail!("token audience `{a}` is not `{}`", self.audience),
            None => bail!("token carries no audience and legacy tokens are refused"),
        }
    }

    /// The public keys as a JWK Set, with the active `kid`.
    pub fn jwks(&self) -> serde_json::Value {
        serde_json::to_value(self.keys.jwks()).unwrap_or_else(|_| serde_json::json!({ "keys": [] }))
    }

    /// Branche l'annuaire persisté : à partir d'ici, `login` ne consulte plus
    /// que lui (fail-closed : pas de repli sur l'annuaire en mémoire).
    ///
    /// ── POURQUOI `&self` ET NON `&mut self` (2026-09-20) ────────────────────
    ///
    /// Le host distribue le STS sous `Arc<StsService>`
    /// (`StsState::sts: Arc<StsService>`) : un appelant qui construit son serveur
    /// par `ServerBuilder` ne dispose donc **plus jamais** d'un `&mut` après le
    /// `build_host()`, et ne pouvait de ce fait pas brancher d'annuaire — il
    /// restait sur l'annuaire en mémoire, **en clair**, « réservé aux tests »
    /// (voir le commentaire de `UserRecord`). Le magasin de mots de passe hachés
    /// existait ; il était inatteignable depuis ce chemin.
    ///
    /// Le champ est donc un `OnceLock` : on le pose **une fois**, au démarrage, à
    /// travers la référence partagée. Poser deux annuaires serait un changement
    /// d'autorité à chaud, ce qu'aucun appelant ne demande et qu'il vaut mieux
    /// refuser bruyamment que subir en silence.
    ///
    /// Rend `true` si l'annuaire a été installé, `false` s'il y en avait déjà un
    /// (l'appelant peut alors le DIRE, plutôt que de croire avoir branché).
    #[must_use]
    pub fn set_annuaire(&self, annuaire: Arc<dyn AnnuaireVerification>) -> bool {
        self.annuaire.set(annuaire).is_ok()
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
    ///
    /// Avec un annuaire branché ([`Self::set_annuaire`]), c'est lui qui
    /// vérifie (argon2id sur store persisté) — l'annuaire en mémoire
    /// ci-dessous ne sert plus qu'aux tests et aux démos sans persistance.
    pub fn login(&self, credentials: &LoginCredentials) -> Result<TokenPair> {
        if let Some(annuaire) = self.annuaire.get() {
            let identite = annuaire
                .verifier_identite(&credentials.username, &credentials.password)
                .ok_or_else(|| anyhow::anyhow!("invalid credentials"))?;
            return self.issue_token_pair(identite.user_id, identite.tenant_id, identite.roles);
        }

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
        // Audience is checked by `check_audience` below, not by jsonwebtoken.
        validation.validate_aud = false;
        validation.validate_exp = true;

        let token_data =
            jsonwebtoken::decode::<StsClaims>(refresh_token, decoding_key, &validation)
                .context("refresh token verification failed")?;

        let claims = token_data.claims;
        self.check_audience(claims.aud.as_deref())
            .context("refresh token audience refused")?;
        // An access token must not buy a new pair (ADR D4). Legacy tokens carry no
        // `token_use`; they are accepted only while the legacy window is open.
        match claims.token_use.as_deref() {
            Some(TOKEN_USE_REFRESH) => {}
            None if self.accept_legacy_tokens => {}
            _ => bail!("token is not a refresh token"),
        }

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

    /// L'émetteur que la vérification doit **exiger**.
    ///
    /// Un jeton signé par un autre émetteur ne doit pas ouvrir une porte : la
    /// signature seule ne dit pas *qui* a signé. Exposer l'émetteur permet à
    /// [`verify_handler`](crate::server::exposure::rest::auth_routes) de le
    /// vérifier au lieu de le supposer.
    pub fn issuer(&self) -> &str {
        &self.issuer
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
        let kid = kid_for_public_key(vk.as_bytes());

        Ok(serde_json::json!({
            "keys": [
                {
                    "kty": "OKP",
                    "crv": "Ed25519",
                    "use": "sig",
                    "alg": "EdDSA",
                    "kid": kid,
                    "x": x,
                }
            ]
        }))
    }

    // ---- private helpers ----

    pub(crate) fn issue_token_pair(
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

        let access_claims = StsClaims {
            sub: user_id.to_string(),
            tenant_id,
            roles: roles.clone(),
            exp: access_exp.timestamp(),
            iat: now.timestamp(),
            jti: access_jti,
            iss: self.issuer.clone(),
            aud: Some(self.audience.clone()),
            token_use: Some(TOKEN_USE_ACCESS.to_string()),
        };

        let refresh_claims = StsClaims {
            sub: user_id.to_string(),
            tenant_id,
            roles,
            exp: refresh_exp.timestamp(),
            iat: now.timestamp(),
            jti: refresh_jti,
            iss: self.issuer.clone(),
            aud: Some(self.audience.clone()),
            token_use: Some(TOKEN_USE_REFRESH.to_string()),
        };

        // Signed through `KeyManager`: the header carries the active `kid`.
        let access_token = self
            .keys
            .sign_claims(&access_claims)
            .context("failed to sign access token")?;

        let refresh_token = self
            .keys
            .sign_claims(&refresh_claims)
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

    // ---- annuaire délégué (chantier STS-PERSISTANT) ----

    /// Un annuaire jouet : un seul couple valide, comptabilise les appels.
    struct AnnuaireJouet {
        tenant: Uuid,
        user: Uuid,
    }

    impl AnnuaireVerification for AnnuaireJouet {
        fn verifier_identite(
            &self,
            username: &str,
            mot_de_passe: &str,
        ) -> Option<IdentiteVerifiee> {
            (username == "persistee" && mot_de_passe == "bon-mdp").then(|| IdentiteVerifiee {
                user_id: self.user,
                tenant_id: self.tenant,
                roles: vec!["lecteur".into()],
            })
        }
    }

    #[test]
    fn annuaire_branche_fait_foi_seul() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let (user, tenant) = (Uuid::new_v4(), Uuid::new_v4());

        // Un utilisateur en mémoire préexiste — il doit cesser de compter.
        sts.register_user(
            "fantome".into(),
            "clair".into(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            vec![],
        );
        // `set_annuaire` pose l'annuaire UNE fois : il rend `false` si un autre
        // était déjà posé — auquel cas celui-ci serait ignoré en silence, et le
        // test mesurerait l'ancien. Le constat est donc exigé ici.
        assert!(
            sts.set_annuaire(Arc::new(AnnuaireJouet { tenant, user })),
            "l'annuaire du test doit être celui qui fait foi (aucun autre n'était posé)"
        );

        // Le couple de l'annuaire passe, et le jeton porte SON identité.
        let pair = sts
            .login(&LoginCredentials {
                username: "persistee".into(),
                password: "bon-mdp".into(),
            })
            .expect("login via l'annuaire");
        let dk = decoding_key(&pub_pem);
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&["test-issuer"]);
        validation.validate_aud = false;
        let claims = jsonwebtoken::decode::<StsClaims>(&pair.access_token, &dk, &validation)
            .unwrap()
            .claims;
        assert_eq!(claims.sub, user.to_string());
        assert_eq!(claims.tenant_id, tenant);
        assert_eq!(claims.roles, vec!["lecteur".to_string()]);

        // Mauvais mot de passe : refus.
        assert!(
            sts.login(&LoginCredentials {
                username: "persistee".into(),
                password: "faux".into(),
            })
            .is_err()
        );

        // Fail-closed : l'annuaire en mémoire n'est PLUS un repli.
        assert!(
            sts.login(&LoginCredentials {
                username: "fantome".into(),
                password: "clair".into(),
            })
            .is_err(),
            "l'annuaire branché fait foi seul — pas de repli sur le clair"
        );
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
        validation.validate_aud = false;

        let token_data = jsonwebtoken::decode::<StsClaims>(&pair.access_token, &dk, &validation)
            .expect("access token should verify");

        assert_eq!(token_data.claims.sub, uid.to_string());
        assert_eq!(token_data.claims.tenant_id, tid);
        assert_eq!(token_data.claims.roles, vec!["editor".to_string()]);
        assert_eq!(token_data.claims.iss, "test-issuer");
        assert!(!token_data.claims.jti.is_empty());
    }

    // ---- audience, kid, and agreement with wami's KeyManager (ADR D4) ----

    fn secret_of(private_pem: &str) -> [u8; 32] {
        use ed25519_dalek::pkcs8::DecodePrivateKey;
        SigningKey::from_pkcs8_pem(private_pem)
            .expect("pkcs8")
            .to_bytes()
    }

    fn public_bytes_of(public_pem: &str) -> [u8; 32] {
        use ed25519_dalek::pkcs8::DecodePublicKey;
        ed25519_dalek::VerifyingKey::from_public_key_pem(public_pem)
            .expect("spki")
            .to_bytes()
    }

    #[test]
    fn issued_token_carries_default_audience_and_active_kid() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let pair = sts
            .issue_token_pair(Uuid::new_v4(), Uuid::new_v4(), vec![])
            .unwrap();

        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.validate_aud = false;
        validation.set_issuer(&["test-issuer"]);
        let data = jsonwebtoken::decode::<StsClaims>(
            &pair.access_token,
            &decoding_key(&pub_pem),
            &validation,
        )
        .unwrap();
        assert_eq!(data.claims.aud.as_deref(), Some(DEFAULT_AUDIENCE));

        let public = public_bytes_of(&pub_pem);
        let kid = jsonwebtoken::decode_header(&pair.access_token)
            .unwrap()
            .kid
            .unwrap();
        assert_eq!(kid, kid_for_public_key(&public));
        // The same key, named the same way by wami: the two sides agree.
        let km = wami::wami::sts::jwt::KeyManager::from_bytes(&secret_of(&priv_pem));
        assert_eq!(km.active().kid(), kid);
    }

    #[test]
    fn jwks_names_the_same_kid_as_the_tokens() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let expected = kid_for_public_key(&public_bytes_of(&pub_pem));
        let from_sts = sts.jwks();
        assert_eq!(from_sts["keys"][0]["kid"], expected);
        let from_pem = StsService::jwks_response(&pub_pem).unwrap();
        assert_eq!(from_pem["keys"][0]["kid"], expected);
    }

    #[test]
    fn check_audience_enforces_match_and_legacy_window() {
        let (priv_pem, _) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        assert!(sts.check_audience(Some(DEFAULT_AUDIENCE)).is_ok());
        assert!(sts.check_audience(Some("other-service")).is_err());
        assert!(
            sts.check_audience(None).is_ok(),
            "legacy window is open by default"
        );

        let strict = build_sts(&priv_pem).with_legacy_tokens(false);
        assert!(strict.check_audience(None).is_err());
        assert!(strict.check_audience(Some(DEFAULT_AUDIENCE)).is_ok());
    }

    #[test]
    fn access_token_cannot_be_exchanged_for_a_new_pair() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem).with_legacy_tokens(false);
        let pair = sts
            .issue_token_pair(Uuid::new_v4(), Uuid::new_v4(), vec![])
            .unwrap();
        let dk = decoding_key(&pub_pem);
        let err = sts.refresh(&pair.access_token, &dk).unwrap_err();
        assert!(
            err.to_string().contains("not a refresh token"),
            "got: {err}"
        );
        // The refresh token of the same pair still works.
        assert!(sts.refresh(&pair.refresh_token, &dk).is_ok());
    }

    #[test]
    fn refresh_refuses_token_addressed_to_another_audience() {
        let (priv_pem, pub_pem) = generate_test_keypair();
        let sts = build_sts(&priv_pem);
        let now = Utc::now();
        let foreign = StsClaims {
            sub: Uuid::new_v4().to_string(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
            exp: (now + Duration::hours(1)).timestamp(),
            iat: now.timestamp(),
            jti: Uuid::new_v4().to_string(),
            iss: "test-issuer".to_string(),
            aud: Some("some-other-service".to_string()),
            token_use: Some(TOKEN_USE_REFRESH.to_string()),
        };
        let token = sts.keys.sign_claims(&foreign).unwrap();
        let err = sts.refresh(&token, &decoding_key(&pub_pem)).unwrap_err();
        assert!(err.to_string().contains("audience"), "got: {err}");
    }
}
