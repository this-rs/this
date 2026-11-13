use crate::DeviceId;
use crate::envelope::{from_hex, to_hex};
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak};
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
    /// Où joindre ce pair, si on le sait. Facultatif : un pair admis par la
    /// seule clé (appairage SAS sans adresse) n'en a pas.
    pub adresse: Option<SocketAddr>,
}

/// Rappel exécuté quand la clé d'un pair est révoquée.
type Rappel = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inner {
    peers: BTreeMap<String, Peer>,
    /// Abonnés à la révocation, par identifiant, avec un numéro pour se désabonner.
    rappels: BTreeMap<String, Vec<(u64, Rappel)>>,
    prochain: u64,
}

/// Registre des pairs : identifiant → clé publique, état, rôle, date, adresse.
///
/// Le vocabulaire (`AnchorKeyState::{Active, Revoked}`, `AnchorKeyRole::{Ownership,
/// Shared}`, la forme sérialisable [`AnchorKey`]) est celui de `this-vfs`, déjà
/// utilisé par le `TrustAnchor` des modules : pas de second vocabulaire. On ne
/// dépend pas de `this-rs` (axum, tokio, bases de données) — seulement du contrat
/// VFS, pur. Refus par défaut : tout ce qui n'est pas `Active` ici n'obtient rien.
///
/// # Partage
///
/// `Peers` est un **handle** : `clone()` ne copie pas le registre, il partage son
/// état. Le listener qui a reçu un clone, la porte qui le tient, et l'application
/// qui révoque (`peers.revoke(id)`) voient donc **le même** registre : une
/// révocation ou un ajout prend effet sur le prochain message, sans rouvrir le
/// listener (ce qui couperait tous les pairs et remettrait à zéro la mémoire
/// anti-rejeu). Les méthodes prennent `&self` ; un état mutable derrière un
/// `RwLock` ne se corrompt pas par partage : chaque opération est atomique.
#[derive(Clone, Default)]
pub struct Peers {
    inner: Arc<RwLock<Inner>>,
}

impl std::fmt::Debug for Peers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peers")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

/// Abonnement à la révocation d'un pair. Le rappel est retiré quand ce jeton est
/// lâché : un abonnement ne survit pas à sa session.
#[must_use = "le rappel n'est actif que tant que ce jeton vit"]
pub struct Surveillance {
    inner: Weak<RwLock<Inner>>,
    id: String,
    numero: u64,
}

impl Drop for Surveillance {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut g = ecrire(&inner);
            if let Some(liste) = g.rappels.get_mut(&self.id) {
                liste.retain(|(n, _)| *n != self.numero);
                if liste.is_empty() {
                    g.rappels.remove(&self.id);
                }
            }
        }
    }
}

fn lire(inner: &Arc<RwLock<Inner>>) -> RwLockReadGuard<'_, Inner> {
    // Un verrou empoisonné ne rend pas le registre faux : ses opérations sont
    // atomiques. On reprend l'état plutôt que de paniquer dans la porte.
    inner.read().unwrap_or_else(|e| e.into_inner())
}

fn ecrire(inner: &Arc<RwLock<Inner>>) -> RwLockWriteGuard<'_, Inner> {
    inner.write().unwrap_or_else(|e| e.into_inner())
}

impl Peers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admet (ou remplace) une clé, `Active`. L'adresse déjà connue est conservée
    /// si le pair existait ; un nouveau pair n'en a pas (voir [`Peers::definir_adresse`]).
    pub fn admit(
        &self,
        id: &DeviceId,
        key: VerifyingKey,
        role: AnchorKeyRole,
        added_at: DateTime<Utc>,
    ) {
        let mut g = ecrire(&self.inner);
        let adresse = g.peers.get(id.as_str()).and_then(|p| p.adresse);
        g.peers.insert(
            id.as_str().to_owned(),
            Peer {
                key,
                state: AnchorKeyState::Active,
                role,
                added_at,
                adresse,
            },
        );
    }

    /// Révoque une clé : effet immédiat sur tous les clones, et coupure de **ses**
    /// sessions seulement (les rappels de [`Peers::surveiller_revocation`]).
    /// `false` si le pair est inconnu.
    pub fn revoke(&self, id: &DeviceId) -> bool {
        let rappels: Vec<Rappel> = {
            let mut g = ecrire(&self.inner);
            match g.peers.get_mut(id.as_str()) {
                Some(p) => p.state = AnchorKeyState::Revoked,
                None => return false,
            }
            g.rappels
                .get(id.as_str())
                .map(|liste| liste.iter().map(|(_, r)| r.clone()).collect())
                .unwrap_or_default()
        };
        // Hors du verrou : un rappel peut lire le registre sans bloquer.
        for rappel in rappels {
            rappel();
        }
        true
    }

    /// Demande à être prévenu quand la clé de `id` est révoquée. Le rappel est
    /// exécuté dans le fil de l'appelant de [`Peers::revoke`] ; il doit rester
    /// court (typiquement : réveiller la tâche de session).
    pub fn surveiller_revocation(
        &self,
        id: &DeviceId,
        rappel: impl Fn() + Send + Sync + 'static,
    ) -> Surveillance {
        let mut g = ecrire(&self.inner);
        let numero = g.prochain;
        g.prochain += 1;
        g.rappels
            .entry(id.as_str().to_owned())
            .or_default()
            .push((numero, Arc::new(rappel)));
        Surveillance {
            inner: Arc::downgrade(&self.inner),
            id: id.as_str().to_owned(),
            numero,
        }
    }

    /// Fixe l'adresse où joindre ce pair. `false` si le pair est inconnu.
    pub fn definir_adresse(&self, id: &DeviceId, adresse: SocketAddr) -> bool {
        match ecrire(&self.inner).peers.get_mut(id.as_str()) {
            Some(p) => {
                p.adresse = Some(adresse);
                true
            }
            None => false,
        }
    }

    /// L'adresse où joindre ce pair, si elle est connue.
    pub fn adresse_de(&self, id: &DeviceId) -> Option<SocketAddr> {
        lire(&self.inner)
            .peers
            .get(id.as_str())
            .and_then(|p| p.adresse)
    }

    pub fn state(&self, id: &DeviceId) -> Option<AnchorKeyState> {
        lire(&self.inner).peers.get(id.as_str()).map(|p| p.state)
    }

    pub fn role(&self, id: &DeviceId) -> Option<AnchorKeyRole> {
        lire(&self.inner).peers.get(id.as_str()).map(|p| p.role)
    }

    pub(crate) fn get(&self, id: &DeviceId) -> Option<Peer> {
        lire(&self.inner).peers.get(id.as_str()).cloned()
    }

    /// L'identifiant dont la clé publique est exactement `cle`, quel que soit
    /// l'état. Sert au transport à relier la clé authentifiée par le canal TLS
    /// à l'identifiant attendu par [`crate::Gate`]. La clé est supposée unique :
    /// un même identifiant ne peut pas porter deux clés, et l'appairage n'inscrit
    /// une clé que sous un identifiant.
    pub fn id_de_cle(&self, cle: &VerifyingKey) -> Option<DeviceId> {
        lire(&self.inner)
            .peers
            .iter()
            .find(|(_, p)| p.key == *cle)
            .map(|(id, _)| DeviceId::parse(id).expect("identifiant du registre toujours valide"))
    }

    pub fn len(&self) -> usize {
        lire(&self.inner).peers.len()
    }

    pub fn is_empty(&self) -> bool {
        lire(&self.inner).peers.is_empty()
    }

    /// Forme persistable (celle du contrat VFS), triée par identifiant. L'adresse
    /// n'en fait pas partie : elle est portée par [`Peers::to_json`].
    pub fn to_keys(&self) -> Vec<AnchorKey> {
        lire(&self.inner)
            .peers
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

    /// Recharge depuis la forme persistée. Sans adresse : elles sont absentes.
    pub fn from_keys(keys: &[AnchorKey]) -> Result<Self, PeerError> {
        let mut peers = BTreeMap::new();
        for k in keys {
            peers.insert(k.key_id.clone(), peer_de_cle(k, None)?);
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                peers,
                ..Inner::default()
            })),
        })
    }

    /// JSON du registre : la forme de [`AnchorKey`], plus `adresse` quand elle est connue.
    pub fn to_json(&self) -> String {
        let fiches: Vec<FicheJson> = {
            let g = lire(&self.inner);
            g.peers
                .iter()
                .map(|(id, p)| FicheJson {
                    cle: AnchorKey {
                        key_id: id.clone(),
                        public_key: to_hex(p.key.as_bytes()),
                        state: p.state,
                        added_at: p.added_at,
                        role: p.role,
                    },
                    adresse: p.adresse,
                })
                .collect()
        };
        serde_json::to_string(&fiches).expect("keys serialize")
    }

    /// Recharge depuis [`Peers::to_json`]. Accepte aussi l'ancienne forme (sans
    /// `adresse`) : le pair revient alors sans adresse.
    pub fn from_json(s: &str) -> Result<Self, PeerError> {
        let fiches: Vec<FicheJson> =
            serde_json::from_str(s).map_err(|e| PeerError::BadKey(e.to_string()))?;
        let mut peers = BTreeMap::new();
        for f in &fiches {
            peers.insert(f.cle.key_id.clone(), peer_de_cle(&f.cle, f.adresse)?);
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                peers,
                ..Inner::default()
            })),
        })
    }
}

/// Une fiche du registre sur le fil : la clé (forme VFS) et l'adresse facultative.
#[derive(Serialize, Deserialize)]
struct FicheJson {
    #[serde(flatten)]
    cle: AnchorKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adresse: Option<SocketAddr>,
}

fn peer_de_cle(k: &AnchorKey, adresse: Option<SocketAddr>) -> Result<Peer, PeerError> {
    DeviceId::parse(&k.key_id).ok_or_else(|| PeerError::BadId(k.key_id.clone()))?;
    let raw = from_hex(&k.public_key).ok_or_else(|| PeerError::BadKey(k.key_id.clone()))?;
    let arr =
        <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| PeerError::BadKey(k.key_id.clone()))?;
    let key = VerifyingKey::from_bytes(&arr).map_err(|_| PeerError::BadKey(k.key_id.clone()))?;
    Ok(Peer {
        key,
        state: k.state,
        role: k.role,
        added_at: k.added_at,
        adresse,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn dev(s: &str) -> DeviceId {
        DeviceId::parse(s).unwrap()
    }
    fn cle(n: u8) -> VerifyingKey {
        SigningKey::from_bytes(&[n; 32]).verifying_key()
    }

    #[test]
    fn le_clone_partage_le_registre() {
        let a = Peers::new();
        let b = a.clone();
        a.admit(
            &dev("device-aaaaaaaa"),
            cle(1),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        assert_eq!(
            b.state(&dev("device-aaaaaaaa")),
            Some(AnchorKeyState::Active)
        );
        b.revoke(&dev("device-aaaaaaaa"));
        assert_eq!(
            a.state(&dev("device-aaaaaaaa")),
            Some(AnchorKeyState::Revoked)
        );
    }

    // (d) L'adresse fait l'aller-retour par to_json/from_json, absente comprise.
    #[test]
    fn l_adresse_fait_l_aller_retour_json_absente_comprise() {
        let peers = Peers::new();
        peers.admit(
            &dev("device-aaaaaaaa"),
            cle(1),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        peers.admit(
            &dev("device-bbbbbbbb"),
            cle(2),
            AnchorKeyRole::Ownership,
            Utc::now(),
        );
        let adresse: SocketAddr = "192.0.2.7:4433".parse().unwrap();
        assert!(peers.definir_adresse(&dev("device-aaaaaaaa"), adresse));
        assert!(!peers.definir_adresse(&dev("device-99999999"), adresse));

        let back = Peers::from_json(&peers.to_json()).unwrap();
        assert_eq!(back.adresse_de(&dev("device-aaaaaaaa")), Some(adresse));
        assert_eq!(back.adresse_de(&dev("device-bbbbbbbb")), None);
        assert_eq!(
            back.state(&dev("device-aaaaaaaa")),
            Some(AnchorKeyState::Active)
        );
        assert_eq!(
            back.role(&dev("device-bbbbbbbb")),
            Some(AnchorKeyRole::Ownership)
        );
    }

    #[test]
    fn l_ancienne_forme_sans_adresse_reste_lisible() {
        let peers = Peers::new();
        peers.admit(
            &dev("device-aaaaaaaa"),
            cle(1),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        // Forme VFS brute, telle qu'écrite avant l'ajout de l'adresse.
        let ancien = serde_json::to_string(&peers.to_keys()).unwrap();
        let back = Peers::from_json(&ancien).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.adresse_de(&dev("device-aaaaaaaa")), None);
    }

    #[test]
    fn surveillance_previent_a_la_revocation_puis_se_desabonne() {
        let peers = Peers::new();
        peers.admit(
            &dev("device-aaaaaaaa"),
            cle(1),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        let compte = Arc::new(AtomicUsize::new(0));
        let c = compte.clone();
        let surveillance = peers.surveiller_revocation(&dev("device-aaaaaaaa"), move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        // Un autre pair n'éveille pas ce rappel.
        peers.admit(
            &dev("device-bbbbbbbb"),
            cle(2),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        peers.revoke(&dev("device-bbbbbbbb"));
        assert_eq!(compte.load(Ordering::SeqCst), 0);

        peers.revoke(&dev("device-aaaaaaaa"));
        assert_eq!(compte.load(Ordering::SeqCst), 1);

        drop(surveillance);
        peers.admit(
            &dev("device-aaaaaaaa"),
            cle(1),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        peers.revoke(&dev("device-aaaaaaaa"));
        assert_eq!(compte.load(Ordering::SeqCst), 1, "désabonné : plus d'appel");
    }

    #[test]
    fn revoquer_un_inconnu_rend_faux() {
        assert!(!Peers::new().revoke(&dev("device-aaaaaaaa")));
    }
}
