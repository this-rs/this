//! Socle du service « Atrium » : un registre de comptes et d'appareils, en **Rust pur**.
//!
//! Aucune I/O réseau, aucune tâche de fond : ce crate ne fait que tenir le registre
//! (quels comptes existent, quels appareils leur sont liés, lesquels sont révoqués) et
//! vérifier les signatures Ed25519 qui autorisent l'enregistrement d'un appareil. Le
//! serveur HTTP qui l'exposera viendra plus tard, au-dessus de ce socle.
//!
//! Règle générale : **deny by default**. Toute entrée invalide (compte inconnu, appareil
//! inconnu, signature absente ou fausse, collision d'identifiant) rend une [`Erreur`],
//! jamais une panique, et ne modifie rien.

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub mod presence;
pub mod relais;

/// Préfixe de domaine des octets signés lors de l'enregistrement d'un appareil : une
/// signature d'appareil ne vaut rien ailleurs (compte, espace, manifeste…). La signature
/// demandée est celle, par la clé de compte, de `DOMAINE_APPAREIL || clé_publique_appareil`.
pub const DOMAINE_APPAREIL: &[u8] = b"this-atrium/v1/device\n";

/// Identifiant de compte : `acct-` suivi de 16 hexadécimaux minuscules (les 8 premiers
/// octets de la clé publique de compte). Ce n'est qu'une étiquette : la confiance vient de
/// la clé enregistrée, jamais de la forme de l'identifiant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(String);

impl AccountId {
    /// L'identifiant dérivé d'une clé publique de compte : `acct-` + les 8 premiers octets
    /// de la clé en hexadécimal minuscule.
    pub fn de_cle(cle: &VerifyingKey) -> Self {
        Self(format!("acct-{}", hex(&cle.to_bytes()[..8])))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifiant d'appareil : `device-` suivi de 8 hexadécimaux minuscules (les 4 premiers
/// octets de la clé publique de l'appareil) — la même forme que `id_maillage` de
/// l'application, pour rester cohérent. Comme pour [`AccountId`], ce n'est qu'une étiquette.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    /// L'identifiant dérivé d'une clé publique d'appareil : `device-` + les 4 premiers
    /// octets de la clé en hexadécimal minuscule.
    pub fn de_cle(cle: &VerifyingKey) -> Self {
        Self(format!("device-{}", hex(&cle.to_bytes()[..4])))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// État d'un appareil dans le registre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Etat {
    Actif,
    Revoque,
}

/// Un appareil enregistré sous un compte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appareil {
    #[serde(with = "serde_cle")]
    pub cle: VerifyingKey,
    pub etat: Etat,
    /// Horodatage de l'enregistrement, en millisecondes depuis l'époque Unix.
    pub ajoute_le_ms: u64,
    /// La signature (par la clé du compte) qui a autorisé l'enregistrement, conservée.
    pub signature: Vec<u8>,
}

/// Un compte, sa clé publique et ses appareils.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compte {
    #[serde(with = "serde_cle")]
    pub cle: VerifyingKey,
    pub appareils: BTreeMap<DeviceId, Appareil>,
}

/// Le registre : comptes indexés par [`AccountId`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registre {
    pub comptes: BTreeMap<AccountId, Compte>,
}

/// Les erreurs du registre. Toutes les entrées invalides passent par ici, jamais par une panique.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Erreur {
    #[error("compte inconnu")]
    CompteInconnu,
    #[error("appareil inconnu")]
    AppareilInconnu,
    #[error("signature d'appareil absente ou invalide")]
    SignatureInvalide,
    #[error("collision : l'identifiant de compte est déjà porté par une autre clé")]
    CollisionCompte,
    #[error("collision : l'identifiant d'appareil est déjà porté par une autre clé")]
    CollisionAppareil,
    #[error("quota de relais dépassé")]
    QuotaDepasse,
}

impl Registre {
    pub fn nouveau() -> Self {
        Self {
            comptes: BTreeMap::new(),
        }
    }

    /// Inscrit un compte (ou le rend s'il existe déjà sous la même clé).
    ///
    /// Refuse (`Err(Erreur::CollisionCompte)`) quand l'identifiant dérivé de `cle_compte`
    /// est déjà porté par un compte dont la clé diffère — un même [`AccountId`] ne peut pas
    /// désigner deux clés différentes.
    pub fn inscrire(&mut self, cle_compte: VerifyingKey) -> Result<AccountId, Erreur> {
        let id = AccountId::de_cle(&cle_compte);
        match self.comptes.get(&id) {
            Some(existant) if existant.cle != cle_compte => Err(Erreur::CollisionCompte),
            Some(_) => Ok(id),
            None => {
                self.comptes.insert(
                    id.clone(),
                    Compte {
                        cle: cle_compte,
                        appareils: BTreeMap::new(),
                    },
                );
                Ok(id)
            }
        }
    }

    /// Enregistre un appareil sous un compte, après avoir vérifié que `signature` est la
    /// signature Ed25519, par la clé du compte, de `DOMAINE_APPAREIL || clé de l'appareil`.
    ///
    /// Signature absente ou invalide → `Err(Erreur::SignatureInvalide)`, rien n'est ajouté.
    /// Compte inconnu → `Err(Erreur::CompteInconnu)`.
    pub fn enregistrer(
        &mut self,
        compte: &AccountId,
        cle_appareil: VerifyingKey,
        signature: &[u8],
    ) -> Result<DeviceId, Erreur> {
        let id = DeviceId::de_cle(&cle_appareil);
        self.enregistrer_sous(compte, id, cle_appareil, signature)
    }

    /// Le cœur de [`Registre::enregistrer`], identifiant fourni : vérifie la signature, puis
    /// refuse une **collision d'identifiant** (une clé DIFFÉRENTE sous une `DeviceId` déjà
    /// prise), sinon insère. Une collision des 4 premiers octets (~2^32, atteignable) ne doit
    /// JAMAIS écraser l'appareil légitime. Idempotent pour la même clé. `pub(crate)` pour que
    /// les tests éprouvent la collision sans fabriquer 2^32 clés.
    pub(crate) fn enregistrer_sous(
        &mut self,
        compte: &AccountId,
        id: DeviceId,
        cle_appareil: VerifyingKey,
        signature: &[u8],
    ) -> Result<DeviceId, Erreur> {
        let signature = Signature::from_slice(signature).map_err(|_| Erreur::SignatureInvalide)?;

        let mut message = Vec::with_capacity(DOMAINE_APPAREIL.len() + 32);
        message.extend_from_slice(DOMAINE_APPAREIL);
        message.extend_from_slice(&cle_appareil.to_bytes());

        let compte_enr = self.comptes.get_mut(compte).ok_or(Erreur::CompteInconnu)?;
        compte_enr
            .cle
            .verify_strict(&message, &signature)
            .map_err(|_| Erreur::SignatureInvalide)?;

        if let Some(existant) = compte_enr.appareils.get(&id) {
            if existant.cle != cle_appareil {
                return Err(Erreur::CollisionAppareil);
            }
            return Ok(id);
        }

        let appareil = Appareil {
            cle: cle_appareil,
            etat: Etat::Actif,
            ajoute_le_ms: maintenant_ms(),
            signature: signature.to_bytes().to_vec(),
        };
        compte_enr.appareils.insert(id.clone(), appareil);
        Ok(id)
    }

    /// Passe un appareil à l'état `Revoque`, sans toucher aux autres appareils du compte.
    pub fn revoquer(&mut self, compte: &AccountId, appareil: &DeviceId) -> Result<(), Erreur> {
        let compte_enr = self.comptes.get_mut(compte).ok_or(Erreur::CompteInconnu)?;
        let ap = compte_enr
            .appareils
            .get_mut(appareil)
            .ok_or(Erreur::AppareilInconnu)?;
        ap.etat = Etat::Revoque;
        Ok(())
    }

    /// Les appareils encore `Actif` d'un compte (les révoqués sont exclus).
    pub fn annuaire(&self, compte: &AccountId) -> Result<Vec<(DeviceId, VerifyingKey)>, Erreur> {
        let compte_enr = self.comptes.get(compte).ok_or(Erreur::CompteInconnu)?;
        let mut appareils = Vec::new();
        for (id, ap) in &compte_enr.appareils {
            if ap.etat == Etat::Actif {
                appareils.push((id.clone(), ap.cle));
            }
        }
        Ok(appareils)
    }

    /// Vrai si un compte porte déjà cet identifiant.
    pub fn deja_inscrit(&self, compte: &AccountId) -> bool {
        self.comptes.contains_key(compte)
    }
}

/// Encodage hexadécimal minuscule, sans allocation superflue.
fn hex(octets: &[u8]) -> String {
    let mut s = String::with_capacity(octets.len() * 2);
    for octet in octets {
        s.push_str(&format!("{octet:02x}"));
    }
    s
}

/// Horloge en millisecondes depuis l'époque Unix. `0` si l'horloge système est en défaut
/// (avant l'époque) — deny by default, jamais de panique.
fn maintenant_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Sérialisation d'une clé Ed25519 par ses 32 octets, sans dépendre de la feature `serde`
/// d'ed25519-dalek (le socle garde les dépendances strictement minimales).
mod serde_cle {
    use ed25519_dalek::VerifyingKey;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(cle: &VerifyingKey, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&cle.to_bytes())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VerifyingKey, D::Error> {
        let octets = <[u8; 32]>::deserialize(d)?;
        VerifyingKey::from_bytes(&octets).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Signe correctement l'enregistrement d'un appareil, puis l'enregistre.
    fn enregistrer_appareil(
        registre: &mut Registre,
        compte: &AccountId,
        cle_compte: &SigningKey,
        cle_appareil: &SigningKey,
    ) -> DeviceId {
        let mut message = Vec::new();
        message.extend_from_slice(DOMAINE_APPAREIL);
        message.extend_from_slice(&cle_appareil.verifying_key().to_bytes());
        let signature = cle_compte.sign(&message);
        registre
            .enregistrer(compte, cle_appareil.verifying_key(), &signature.to_bytes())
            .unwrap()
    }

    #[test]
    fn la_collision_de_device_id_n_ecrase_pas() {
        let cle_compte = SigningKey::from_bytes(&[7u8; 32]);
        let mut registre = Registre::nouveau();
        let compte = registre.inscrire(cle_compte.verifying_key()).unwrap();

        // A s'enregistre normalement.
        let cle_a = SigningKey::from_bytes(&[1u8; 32]);
        let id_a = enregistrer_appareil(&mut registre, &compte, &cle_compte, &cle_a);

        // B est une clé DIFFÉRENTE, correctement signée, présentée sous l'id de A
        // (simule une collision des 4 premiers octets) : refusée, pas d'écrasement.
        let cle_b = SigningKey::from_bytes(&[2u8; 32]);
        let mut message = Vec::new();
        message.extend_from_slice(DOMAINE_APPAREIL);
        message.extend_from_slice(&cle_b.verifying_key().to_bytes());
        let signature_b = cle_compte.sign(&message);

        assert_eq!(
            registre.enregistrer_sous(
                &compte,
                id_a.clone(),
                cle_b.verifying_key(),
                &signature_b.to_bytes()
            ),
            Err(Erreur::CollisionAppareil)
        );
        // A est intact.
        assert_eq!(
            registre.annuaire(&compte).unwrap(),
            vec![(id_a, cle_a.verifying_key())]
        );
    }

    #[test]
    fn un_compte_s_inscrit_et_relit_ses_appareils() {
        let cle_compte = SigningKey::from_bytes(&[7u8; 32]);
        let mut registre = Registre::nouveau();
        let compte = registre.inscrire(cle_compte.verifying_key()).unwrap();

        let cle_appareil = SigningKey::from_bytes(&[9u8; 32]);
        let appareil = enregistrer_appareil(&mut registre, &compte, &cle_compte, &cle_appareil);

        assert_eq!(appareil, DeviceId::de_cle(&cle_appareil.verifying_key()));
        assert_eq!(
            registre.annuaire(&compte).unwrap(),
            vec![(appareil, cle_appareil.verifying_key())]
        );
    }

    #[test]
    fn un_appareil_non_signe_est_refuse() {
        let cle_compte = SigningKey::from_bytes(&[7u8; 32]);
        let mut registre = Registre::nouveau();
        let compte = registre.inscrire(cle_compte.verifying_key()).unwrap();

        let cle_appareil = SigningKey::from_bytes(&[9u8; 32]);
        let signature_bidon = [0u8; 64];

        assert_eq!(
            registre.enregistrer(&compte, cle_appareil.verifying_key(), &signature_bidon),
            Err(Erreur::SignatureInvalide)
        );
        assert!(registre.annuaire(&compte).unwrap().is_empty());
    }

    #[test]
    fn revoquer_retire_un_appareil_sans_toucher_aux_autres() {
        let cle_compte = SigningKey::from_bytes(&[7u8; 32]);
        let mut registre = Registre::nouveau();
        let compte = registre.inscrire(cle_compte.verifying_key()).unwrap();

        let cle_a = SigningKey::from_bytes(&[1u8; 32]);
        let cle_b = SigningKey::from_bytes(&[2u8; 32]);
        let a = enregistrer_appareil(&mut registre, &compte, &cle_compte, &cle_a);
        let b = enregistrer_appareil(&mut registre, &compte, &cle_compte, &cle_b);

        registre.revoquer(&compte, &a).unwrap();

        assert_eq!(
            registre.annuaire(&compte).unwrap(),
            vec![(b, cle_b.verifying_key())]
        );
    }

    #[test]
    fn un_compte_inconnu_est_refuse() {
        let cle_compte = SigningKey::from_bytes(&[7u8; 32]);
        let mut registre = Registre::nouveau();
        let inconnu = AccountId::de_cle(&SigningKey::from_bytes(&[42u8; 32]).verifying_key());

        let cle_appareil = SigningKey::from_bytes(&[9u8; 32]);
        let mut message = Vec::new();
        message.extend_from_slice(DOMAINE_APPAREIL);
        message.extend_from_slice(&cle_appareil.verifying_key().to_bytes());
        let signature = cle_compte.sign(&message);

        assert_eq!(
            registre.enregistrer(
                &inconnu,
                cle_appareil.verifying_key(),
                &signature.to_bytes()
            ),
            Err(Erreur::CompteInconnu)
        );
        assert_eq!(registre.annuaire(&inconnu), Err(Erreur::CompteInconnu));
    }

    #[test]
    fn une_collision_de_compte_est_refusee() {
        let mut registre = Registre::nouveau();
        let cle_a = SigningKey::from_bytes(&[7u8; 32]).verifying_key();
        let cle_b = SigningKey::from_bytes(&[8u8; 32]).verifying_key();
        let id_a = AccountId::de_cle(&cle_a);

        // On simule un registre où `id_a` est déjà porté par une AUTRE clé.
        registre.comptes.insert(
            id_a.clone(),
            Compte {
                cle: cle_b,
                appareils: BTreeMap::new(),
            },
        );

        assert_eq!(registre.inscrire(cle_a), Err(Erreur::CollisionCompte));
    }

    #[test]
    fn le_registre_se_relit_apres_serde() {
        let cle_compte = SigningKey::from_bytes(&[7u8; 32]);
        let mut registre = Registre::nouveau();
        let compte = registre.inscrire(cle_compte.verifying_key()).unwrap();
        let cle_appareil = SigningKey::from_bytes(&[9u8; 32]);
        enregistrer_appareil(&mut registre, &compte, &cle_compte, &cle_appareil);

        let json = serde_json::to_string(&registre).unwrap();
        let relu: Registre = serde_json::from_str(&json).unwrap();
        assert_eq!(registre, relu);
    }
}
