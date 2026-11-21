//! Présence : rendez-vous signés avec TTL, en **Rust pur**.
//!
//! Un appareil encore `Actif` publie une annonce de présence — l'adresse où le
//! joindre — signée avec **sa propre clé** et bornée dans le temps. La signature
//! porte sur le domaine, l'adresse, l'échéance et l'identifiant de l'appareil :
//! personne ne peut faire vivre une adresse au nom d'un autre appareil, ni
//! prolonger une annonce au-delà de son échéance (l'échéance est dans les octets
//! signés). Règle générale du socle : deny by default, jamais de panique.

use crate::{AccountId, DeviceId, Erreur, Registre};
use ed25519_dalek::Signature;
use std::collections::BTreeMap;

/// Préfixe de domaine des octets signés lors d'une publication de présence : une
/// signature de présence ne vaut rien ailleurs (appareil, compte, espace…).
pub const DOMAINE_PRESENCE: &[u8] = b"this-atrium/v1/presence\n";

/// Durée maximale d'une annonce : une heure. Toute durée supérieure est bornée à
/// cette valeur (`duree_ms.min(DUREE_MAX_MS)`).
pub const DUREE_MAX_MS: u64 = 3_600_000;

/// Une annonce de présence : où joindre l'appareil, jusqu'à quand, et la signature
/// (par la clé de l'appareil) qui l'autorise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendezVous {
    pub adresse: String,
    pub expire_le_ms: u64,
    pub signature: Vec<u8>,
}

/// Les annonces de présence en cours, indexées par appareil.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Presence {
    rendez_vous: BTreeMap<DeviceId, RendezVous>,
}

impl Presence {
    /// Publie (ou remplace) l'annonce de présence d'un appareil.
    ///
    /// 1. L'appareil doit être `Actif` dans l'annuaire du compte (un appareil
    ///    révoqué en est absent) ; sinon `Err(AppareilInconnu)`.
    /// 2. L'échéance est `maintenant_ms + duree_ms.min(DUREE_MAX_MS)`, bornée.
    /// 3. La signature est vérifiée **avant** toute écriture ; invalide →
    ///    `Err(SignatureInvalide)`, rien n'est inséré.
    /// 4. L'annonce est insérée (et remplace l'éventuelle précédente).
    // Matière signée explicite (adresse, échéance, signature) : la regrouper en struct
    // cacherait ce qui est réellement signé. 8 arguments assumés.
    #[allow(clippy::too_many_arguments)]
    pub fn publier(
        &mut self,
        registre: &Registre,
        compte: &AccountId,
        appareil: &DeviceId,
        adresse: &str,
        duree_ms: u64,
        maintenant_ms: u64,
        signature: &[u8],
    ) -> Result<(), Erreur> {
        // 1. Appareil actif (l'annuaire n'expose que les `Actif`).
        let cle = registre
            .annuaire(compte)?
            .into_iter()
            .find(|(id, _)| id == appareil)
            .map(|(_, cle)| cle)
            .ok_or(Erreur::AppareilInconnu)?;

        // 2. Échéance bornée.
        let expire_le_ms = maintenant_ms.saturating_add(duree_ms.min(DUREE_MAX_MS));

        // 3. Signature vérifiée avant d'écrire.
        let signature = Signature::from_slice(signature).map_err(|_| Erreur::SignatureInvalide)?;
        let mut message = Vec::new();
        message.extend_from_slice(DOMAINE_PRESENCE);
        message.extend_from_slice(adresse.as_bytes());
        message.extend_from_slice(&expire_le_ms.to_be_bytes());
        message.extend_from_slice(appareil.as_str().as_bytes());
        cle.verify_strict(&message, &signature)
            .map_err(|_| Erreur::SignatureInvalide)?;

        // 4. Écrire.
        self.rendez_vous.insert(
            appareil.clone(),
            RendezVous {
                adresse: adresse.to_string(),
                expire_le_ms,
                signature: signature.to_bytes().to_vec(),
            },
        );
        Ok(())
    }

    /// L'adresse encore valable d'un appareil, ou `None`. Une annonce périmée est
    /// purgée au passage : elle ne reste pas dans la carte.
    pub fn joindre(&mut self, appareil: &DeviceId, maintenant_ms: u64) -> Option<String> {
        match self.rendez_vous.get(appareil) {
            Some(rv) if rv.expire_le_ms > maintenant_ms => Some(rv.adresse.clone()),
            _ => {
                self.rendez_vous.remove(appareil);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Un registre avec un compte et un appareil actif enregistrés, plus les clés
    /// pour signer une présence.
    fn comptes() -> (Registre, SigningKey, SigningKey, AccountId, DeviceId) {
        let mut registre = Registre::nouveau();
        let cle_compte = SigningKey::from_bytes(&[7u8; 32]);
        let cle_appareil = SigningKey::from_bytes(&[9u8; 32]);
        let compte = registre.inscrire(cle_compte.verifying_key()).unwrap();

        let mut message = Vec::new();
        message.extend_from_slice(crate::DOMAINE_APPAREIL);
        message.extend_from_slice(&cle_appareil.verifying_key().to_bytes());
        let signature = cle_compte.sign(&message);
        let appareil = registre
            .enregistrer(&compte, cle_appareil.verifying_key(), &signature.to_bytes())
            .unwrap();

        (registre, cle_compte, cle_appareil, compte, appareil)
    }

    /// Signe correctement une annonce de présence avec la clé de l'appareil.
    fn signe_presence(
        cle: &SigningKey,
        adresse: &str,
        expire_ms: u64,
        appareil: &DeviceId,
    ) -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(DOMAINE_PRESENCE);
        message.extend_from_slice(adresse.as_bytes());
        message.extend_from_slice(&expire_ms.to_be_bytes());
        message.extend_from_slice(appareil.as_str().as_bytes());
        cle.sign(&message).to_bytes().to_vec()
    }

    #[test]
    fn publie_signe_et_frais_se_rejoint() {
        let (registre, _cle_compte, cle_appareil, compte, appareil) = comptes();
        let mut presence = Presence::default();
        let maintenant = 1_000_000;
        let duree = 60_000;
        let expire = maintenant + duree;
        let signature = signe_presence(&cle_appareil, "127.0.0.1:9000", expire, &appareil);

        presence
            .publier(
                &registre,
                &compte,
                &appareil,
                "127.0.0.1:9000",
                duree,
                maintenant,
                &signature,
            )
            .unwrap();

        assert_eq!(
            presence.joindre(&appareil, maintenant + 1),
            Some("127.0.0.1:9000".to_string())
        );
    }

    #[test]
    fn une_annonce_perimee_est_purgee() {
        let (registre, _cle_compte, cle_appareil, compte, appareil) = comptes();
        let mut presence = Presence::default();
        let maintenant = 1_000_000;
        let duree = 10_000;
        let expire = maintenant + duree;
        let signature = signe_presence(&cle_appareil, "127.0.0.1:9000", expire, &appareil);
        presence
            .publier(
                &registre,
                &compte,
                &appareil,
                "127.0.0.1:9000",
                duree,
                maintenant,
                &signature,
            )
            .unwrap();

        // À l'échéance exacte, l'annonce n'est plus valable et l'entrée est retirée.
        assert_eq!(presence.joindre(&appareil, expire), None);
        assert!(presence.rendez_vous.is_empty());
    }

    #[test]
    fn une_signature_fausse_est_refusee_sans_ecrire() {
        let (registre, _cle_compte, _cle_appareil, compte, appareil) = comptes();
        let mut presence = Presence::default();
        let maintenant = 1_000_000;
        let duree = 60_000;
        let signature_bidon = vec![0u8; 64];

        assert_eq!(
            presence.publier(
                &registre,
                &compte,
                &appareil,
                "127.0.0.1:9000",
                duree,
                maintenant,
                &signature_bidon
            ),
            Err(Erreur::SignatureInvalide)
        );
        assert!(presence.rendez_vous.is_empty());
    }

    #[test]
    fn un_appareil_revoque_est_inconnu() {
        let (mut registre, _cle_compte, cle_appareil, compte, appareil) = comptes();
        registre.revoquer(&compte, &appareil).unwrap();
        let mut presence = Presence::default();
        let maintenant = 1_000_000;
        let duree = 60_000;
        let expire = maintenant + duree;
        let signature = signe_presence(&cle_appareil, "127.0.0.1:9000", expire, &appareil);

        assert_eq!(
            presence.publier(
                &registre,
                &compte,
                &appareil,
                "127.0.0.1:9000",
                duree,
                maintenant,
                &signature
            ),
            Err(Erreur::AppareilInconnu)
        );
        assert!(presence.rendez_vous.is_empty());
    }
}
