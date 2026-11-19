//! The session of a person — a token that expires, unlike a key.
//!
//! A static key (`env:` or `vault:`) answers the same thing forever. A session obtained
//! through a sign-in flow (OAuth 2.0 / OpenID Connect) answers something that goes stale,
//! so a provider configured with one needs two things a key never needed: a clock to know
//! when it goes stale, and a way to be renewed.
//!
//! ## Why renewal is not here
//!
//! Renewing means calling the token endpoint with a `client_id` and a refresh token. Those
//! belong to the host that ran the flow: the framework was never given them, and inventing
//! them would mean hard-coding someone else's provider. So this module **names** the state
//! (`usable`, `expires_at`, [`SessionError`]) and the host **acts** on it. Naming and acting
//! are two different jobs, and mixing them is how a library ends up owning a vendor's
//! private protocol.

use std::fmt;

use crate::secret::Secret;

/// How long before expiry a session is considered spent.
///
/// Sixty seconds: a request that starts on a token valid for another few seconds can still
/// be refused *during* the stream, and a refusal mid-stream reads like a wrong key — not
/// like a session to renew. Renewing a minute early costs nothing.
pub const MARGE_MS: i64 = 60_000;

/// An access token, the optional token that renews it, and when it dies.
pub struct Session {
    access: Secret,
    refresh: Option<Secret>,
    /// Milliseconds since the Unix epoch. `None` when the host did not receive one: the
    /// session is then used until the engine refuses it, which is honest — we cannot renew
    /// what we cannot date, but we can still try it once.
    expires_at: Option<i64>,
}

impl Session {
    pub fn new(access: Secret, refresh: Option<Secret>, expires_at: Option<i64>) -> Self {
        Self {
            access,
            refresh,
            expires_at,
        }
    }

    /// The value that goes on the wire. Held by a client, and nowhere else.
    pub fn access(&self) -> &Secret {
        &self.access
    }

    // A refresh token is deliberately left unreadable from outside this crate until a host
    // needs it: nothing in the framework renews a session, so nothing here has a use for it.

    pub fn expires_at(&self) -> Option<i64> {
        self.expires_at
    }

    /// Can it be sent right now?
    ///
    /// `now_ms <= 0` means "the host does not know the time": the session is then assumed
    /// usable rather than refused. Refusing blindly would make a clock-less host unusable;
    /// sending it and letting the engine answer `401` costs one round trip and no lie.
    pub fn usable(&self, now_ms: i64) -> bool {
        match self.expires_at {
            None => true,
            Some(_) if now_ms <= 0 => true,
            Some(expiry) => now_ms + MARGE_MS < expiry,
        }
    }

    /// True when the session is still valid but close enough to expiry that the host should
    /// renew it before the next request. Renewing in the background is cheaper than failing.
    pub fn renew_soon(&self, now_ms: i64) -> bool {
        match self.expires_at {
            Some(expiry) if now_ms > 0 => now_ms + MARGE_MS * 5 >= expiry,
            _ => false,
        }
    }
}

// The tokens never print. A `Debug` that showed them would put them in a log the moment
// someone debug-prints a config.
impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("access", &"***")
            .field("refresh", &self.refresh.as_ref().map(|_| "***"))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The host holds no session under that name: the person has not signed in, or the
    /// session was revoked.
    Missing(String),
    /// The host cannot answer here (no sign-in flow is plugged in on this target).
    Unavailable(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(
                formatter,
                "session introuvable : {name} (la personne ne s'est pas connectée, ou la session a été révoquée)"
            ),
            Self::Unavailable(name) => {
                write!(formatter, "session non résolvable ici : {name}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

/// Turns a session name into tokens. Implemented by the host, which ran the sign-in flow.
///
/// Synchronous on purpose: reading a token the host already holds costs nothing. Renewing
/// it is the host's business, triggered by [`crate::ProviderError::SessionExpired`] or by
/// [`Session::renew_soon`] — not by a call from inside a request.
pub trait SessionSource: Send + Sync {
    fn session(&self, name: &str) -> Result<Option<Session>, SessionError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(expires_at: Option<i64>) -> Session {
        Session::new(
            Secret::new("jeton"),
            Some(Secret::new("refresh")),
            expires_at,
        )
    }

    #[test]
    fn une_session_sans_expiration_est_utilisable() {
        assert!(session(None).usable(1_000_000));
    }

    #[test]
    fn une_session_expiree_n_est_pas_utilisable() {
        assert!(!session(Some(1_000_000)).usable(1_000_000));
        assert!(!session(Some(1_000_000)).usable(999_000));
    }

    #[test]
    fn la_marge_evite_d_envoyer_un_jeton_mourant() {
        // Valide dans 30 s : dans la marge, donc considérée comme dépensée.
        assert!(!session(Some(1_000_000)).usable(1_000_000 - 30_000));
        // Valide dans 5 minutes : utilisable.
        assert!(session(Some(1_000_000)).usable(1_000_000 - 300_000));
    }

    #[test]
    fn une_horloge_inconnue_ne_refuse_rien() {
        // `now_ms == 0` : l'hôte ne sait pas l'heure. On envoie, le moteur tranchera.
        assert!(session(Some(1)).usable(0));
    }

    #[test]
    fn le_renouvellement_est_annonce_avant_l_expiration() {
        let s = session(Some(1_000_000));
        assert!(s.renew_soon(1_000_000 - MARGE_MS * 4));
        assert!(!s.renew_soon(1_000_000 - MARGE_MS * 100));
        assert!(!session(None).renew_soon(1));
    }

    #[test]
    fn le_debug_ne_montre_jamais_le_jeton() {
        // On cherche les VALEURS, pas les noms de champs : « refresh » est le nom du champ
        // et doit rester lisible. C'est le contenu qui ne doit jamais sortir.
        let s = Session::new(
            Secret::new("JETON-123"),
            Some(Secret::new("REFRESH-456")),
            Some(1_000_000),
        );
        let texte = format!("{s:?}");
        assert!(!texte.contains("JETON-123"), "{texte}");
        assert!(!texte.contains("REFRESH-456"), "{texte}");
        assert!(texte.contains("***"));
    }
}
