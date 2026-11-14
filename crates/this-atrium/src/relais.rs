//! Relais opaque : le cœur **pur** du comptage de quotas, en Rust pur.
//!
//! Le relais ne lit JAMAIS les octets qui le traversent : il ne fait que compter,
//! par compte et sur une fenêtre glissante d'une heure, combien d'octets ont été
//! relayés, et refuse de transmettre quand le quota est atteint. Aucune I/O, aucune
//! tâche de fond, jamais de panique : deny by default.

use crate::{AccountId, Erreur};
use std::collections::BTreeMap;

/// Longueur de la fenêtre glissante : une heure, en millisecondes.
pub const FENETRE_MS: u64 = 3_600_000;

/// Le quota d'un relais : combien d'octets un compte peut relayer par heure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quota {
    pub octets_par_heure: u64,
}

/// La fenêtre glissante courante d'un compte.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fenetre {
    pub debut_ms: u64,
    pub octets: u64,
}

/// Le relais : un compteur d'octets par compte, sur fenêtre glissante d'une heure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relais {
    fenetres: BTreeMap<AccountId, Fenetre>,
}

impl Relais {
    /// Compte `octets` relais pour `compte`, ou refuse si le quota horaire est atteint.
    ///
    /// Fenêtre glissante : si le compte n'a pas de fenêtre, ou si la fenêtre a une
    /// heure ou plus, elle repart (`debut_ms = maintenant_ms`, `octets = 0`). Si
    /// `octets_actuels + octets > quota.octets_par_heure`, rien n'est compté et
    /// `Err(Erreur::QuotaDepasse)` est rendu ; sinon le compteur est incrémenté.
    pub fn transmettre(
        &mut self,
        compte: &AccountId,
        maintenant_ms: u64,
        octets: usize,
        quota: &Quota,
    ) -> Result<(), Erreur> {
        let fenetre = self.fenetres.entry(compte.clone()).or_insert(Fenetre {
            debut_ms: maintenant_ms,
            octets: 0,
        });
        if maintenant_ms.saturating_sub(fenetre.debut_ms) >= FENETRE_MS {
            fenetre.debut_ms = maintenant_ms;
            fenetre.octets = 0;
        }
        if fenetre.octets.saturating_add(octets as u64) > quota.octets_par_heure {
            return Err(Erreur::QuotaDepasse);
        }
        fenetre.octets += octets as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn compte(n: u8) -> AccountId {
        AccountId::de_cle(&SigningKey::from_bytes(&[n; 32]).verifying_key())
    }

    #[test]
    fn sous_le_quota_ca_passe() {
        let mut relais = Relais::default();
        let quota = Quota {
            octets_par_heure: 100,
        };
        let c = compte(7);
        relais.transmettre(&c, 0, 40, &quota).unwrap();
        relais.transmettre(&c, 1_000, 40, &quota).unwrap();
    }

    #[test]
    fn depassement_refuse_sans_incrementer() {
        let mut relais = Relais::default();
        let quota = Quota {
            octets_par_heure: 100,
        };
        let c = compte(7);
        relais.transmettre(&c, 0, 60, &quota).unwrap();

        assert_eq!(
            relais.transmettre(&c, 1_000, 41, &quota),
            Err(Erreur::QuotaDepasse)
        );

        // Le compteur n'a pas bougé : les 40 octets restants passent encore.
        relais.transmettre(&c, 2_000, 40, &quota).unwrap();
    }

    #[test]
    fn une_heure_plus_tard_la_fenetre_repart() {
        let mut relais = Relais::default();
        let quota = Quota {
            octets_par_heure: 100,
        };
        let c = compte(7);
        relais.transmettre(&c, 0, 60, &quota).unwrap();
        assert_eq!(
            relais.transmettre(&c, 1_000, 50, &quota),
            Err(Erreur::QuotaDepasse)
        );

        // Une heure pile plus tard, la fenêtre glisse et repart à zéro.
        relais.transmettre(&c, 3_600_000, 50, &quota).unwrap();
    }
}
