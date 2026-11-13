use crate::DeviceId;
use crate::envelope::{from_hex, to_hex};
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use std::collections::BTreeMap;
use this_vfs::layout::{AnchorKey, AnchorKeyRole, AnchorKeyState};

/// Erreur du registre.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PeerError {
    #[error("identifiant d'appareil invalide : {0}")]
    BadId(String),
    #[error("clé publique invalide pour {0}")]
    BadKey(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Peer {
    pub key: VerifyingKey,
    pub state: AnchorKeyState,
    pub role: AnchorKeyRole,
    pub added_at: DateTime<Utc>,
}

/// Registre des pairs : identifiant → clé publique, état, rôle, date.
///
/// Le vocabulaire (`AnchorKeyState::{Active, Revoked}`, `AnchorKeyRole::{Ownership,
/// Shared}`, la forme sérialisable [`AnchorKey`]) est celui de `this-vfs`, déjà
/// utilisé par le `TrustAnchor` des modules : pas de second vocabulaire. On ne
/// dépend pas de `this-rs` (axum, tokio, bases de données) — seulement du contrat
/// VFS, pur. Refus par défaut : tout ce qui n'est pas `Active` ici n'obtient rien.
#[derive(Debug, Clone, Default)]
pub struct Peers {
    peers: BTreeMap<String, Peer>,
}

impl Peers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admet (ou remplace) une clé, `Active`.
    pub fn admit(
        &mut self,
        id: &DeviceId,
        key: VerifyingKey,
        role: AnchorKeyRole,
        added_at: DateTime<Utc>,
    ) {
        self.peers.insert(
            id.as_str().to_owned(),
            Peer {
                key,
                state: AnchorKeyState::Active,
                role,
                added_at,
            },
        );
    }

    /// Révoque une clé : effet immédiat. `false` si le pair est inconnu.
    pub fn revoke(&mut self, id: &DeviceId) -> bool {
        match self.peers.get_mut(id.as_str()) {
            Some(p) => {
                p.state = AnchorKeyState::Revoked;
                true
            }
            None => false,
        }
    }

    pub fn state(&self, id: &DeviceId) -> Option<AnchorKeyState> {
        self.peers.get(id.as_str()).map(|p| p.state)
    }

    pub fn role(&self, id: &DeviceId) -> Option<AnchorKeyRole> {
        self.peers.get(id.as_str()).map(|p| p.role)
    }

    pub(crate) fn get(&self, id: &DeviceId) -> Option<&Peer> {
        self.peers.get(id.as_str())
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Forme persistable (celle du contrat VFS), triée par identifiant.
    pub fn to_keys(&self) -> Vec<AnchorKey> {
        self.peers
            .iter()
            .map(|(id, p)| AnchorKey {
                key_id: id.clone(),
                public_key: to_hex(p.key.as_bytes()),
                state: p.state,
                added_at: p.added_at,
                role: p.role,
            })
            .collect()
    }

    /// Recharge depuis la forme persistée.
    pub fn from_keys(keys: &[AnchorKey]) -> Result<Self, PeerError> {
        let mut peers = BTreeMap::new();
        for k in keys {
            DeviceId::parse(&k.key_id).ok_or_else(|| PeerError::BadId(k.key_id.clone()))?;
            let raw = from_hex(&k.public_key).ok_or_else(|| PeerError::BadKey(k.key_id.clone()))?;
            let arr = <[u8; 32]>::try_from(raw.as_slice())
                .map_err(|_| PeerError::BadKey(k.key_id.clone()))?;
            let key =
                VerifyingKey::from_bytes(&arr).map_err(|_| PeerError::BadKey(k.key_id.clone()))?;
            peers.insert(
                k.key_id.clone(),
                Peer {
                    key,
                    state: k.state,
                    role: k.role,
                    added_at: k.added_at,
                },
            );
        }
        Ok(Self { peers })
    }

    /// JSON du registre.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.to_keys()).expect("keys serialize")
    }

    /// Recharge depuis [`Peers::to_json`].
    pub fn from_json(s: &str) -> Result<Self, PeerError> {
        let keys: Vec<AnchorKey> =
            serde_json::from_str(s).map_err(|e| PeerError::BadKey(e.to_string()))?;
        Self::from_keys(&keys)
    }
}
