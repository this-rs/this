//! Jetons et mots de passe.
//!
//! ÉCART AU « RIEN N'EST RÉIMPLÉMENTÉ » : le STS du framework (`this::core::auth::sts`) et
//! l'annuaire argon2 (`storage::annuaire_identites`) vivent derrière la feature `wami`,
//! qui tire trois dépôts git et l'IAM complet. La forge n'en a besoin que de deux
//! choses, écrites ici : un JWT Ed25519 (`jsonwebtoken` + `ed25519-dalek`, les crates que
//! le framework utilise) et un hachage argon2id aux paramètres OWASP par défaut du crate.

use std::time::{SystemTime, UNIX_EPOCH};

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use jsonwebtoken::{Algorithm, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::sign::{ForgeKeys, b64url};

pub const ISSUER: &str = "this-forge";

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Identifiant du compte.
    pub sub: String,
    pub iss: String,
    pub iat: u64,
    pub exp: u64,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn issue_token(keys: &ForgeKeys, account_id: &str, ttl_secs: u64) -> anyhow::Result<String> {
    let now = now_secs();
    let claims = Claims {
        sub: account_id.into(),
        iss: ISSUER.into(),
        iat: now,
        exp: now + ttl_secs,
    };
    Ok(encode(
        &Header::new(Algorithm::EdDSA),
        &claims,
        keys.jwt_encoding(),
    )?)
}

/// Vérifie signature, expiration et émetteur ; rend l'identifiant du compte.
pub fn verify_token(keys: &ForgeKeys, token: &str) -> Option<String> {
    let mut v = Validation::new(Algorithm::EdDSA);
    v.leeway = 0;
    v.set_issuer(&[ISSUER]);
    v.set_required_spec_claims(&["exp", "sub", "iss"]);
    decode::<Claims>(token, keys.jwt_decoding(), &v)
        .ok()
        .map(|d| d.claims.sub)
}

/// Jeton de rafraîchissement : 32 octets aléatoires, base64url. Seul son sha256 est stocké.
pub fn new_refresh_token() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    b64url(&b)
}

/// argon2id, paramètres par défaut du crate (OWASP : m=19456 KiB, t=2, p=1).
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hachage : {e}"))
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}
