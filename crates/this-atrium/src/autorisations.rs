//! Autorisations inter-comptes : un compte (`de`) accorde à un autre (`a`) une capacité
//! (par exemple `"annuaire"`) sur tout ou partie de ses appareils.
//!
//! Aucune I/O. L'authenticité repose sur la signature Ed25519 de la clé du compte `de`
//! sur des octets canoniques à domaine séparé : personne d'autre ne peut produire une
//! autorisation valide. Deny by default : toute entrée invalide rend une [`Erreur`].

use crate::{AccountId, DeviceId, Erreur};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Préfixe de domaine des octets signés d'une autorisation.
pub const DOMAINE_AUTORISATION: &[u8] = b"this-atrium/v1/autorisation\n";

/// Les appareils couverts par une autorisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Portee {
    /// Tous les appareils du compte `de`.
    Tous,
    /// Un seul appareil.
    Appareil(DeviceId),
}

impl Portee {
    fn wire(&self) -> String {
        match self {
            Portee::Tous => "tous".to_string(),
            Portee::Appareil(id) => format!("appareil:{}", id.as_str()),
        }
    }

    fn couvre(&self, appareil: &DeviceId) -> bool {
        match self {
            Portee::Tous => true,
            Portee::Appareil(id) => id == appareil,
        }
    }
}

/// Une autorisation signée par le compte `de`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Autorisation {
    pub de: AccountId,
    pub a: AccountId,
    pub capacite: String,
    pub portee: Portee,
    pub cree_ms: u64,
    pub expire_ms: Option<u64>,
    pub signature: Vec<u8>,
}

impl Autorisation {
    /// Les octets signés (canoniques) :
    /// `DOMAINE || de \n a \n capacite \n portee \n cree_ms \n expire_ms|0`.
    pub fn octets_signes(&self) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(DOMAINE_AUTORISATION);
        m.extend_from_slice(self.de.as_str().as_bytes());
        m.push(b'\n');
        m.extend_from_slice(self.a.as_str().as_bytes());
        m.push(b'\n');
        m.extend_from_slice(self.capacite.as_bytes());
        m.push(b'\n');
        m.extend_from_slice(self.portee.wire().as_bytes());
        m.push(b'\n');
        m.extend_from_slice(self.cree_ms.to_string().as_bytes());
        m.push(b'\n');
        m.extend_from_slice(
            self.expire_ms
                .map(|v| v.to_string())
                .unwrap_or_else(|| "0".into())
                .as_bytes(),
        );
        m
    }

    fn verifier(&self, cle_de: &VerifyingKey) -> Result<(), Erreur> {
        let signature =
            Signature::from_slice(&self.signature).map_err(|_| Erreur::SignatureInvalide)?;
        cle_de
            .verify_strict(&self.octets_signes(), &signature)
            .map_err(|_| Erreur::SignatureInvalide)
    }

    /// Même autorisation (champs signés), signature mise à part.
    fn meme_que(&self, autre: &Autorisation) -> bool {
        self.de == autre.de
            && self.a == autre.a
            && self.capacite == autre.capacite
            && self.portee == autre.portee
    }
}

/// L'ensemble des autorisations octroyées.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Autorisations {
    pub octroyees: Vec<Autorisation>,
}

impl Autorisations {
    /// Enregistre une autorisation après vérification de la signature de `cle_de`.
    /// Idempotent : une autorisation de mêmes champs remplace la précédente.
    pub fn octroyer(
        &mut self,
        autorisation: Autorisation,
        cle_de: &VerifyingKey,
    ) -> Result<(), Erreur> {
        autorisation.verifier(cle_de)?;
        match self
            .octroyees
            .iter_mut()
            .find(|x| x.meme_que(&autorisation))
        {
            Some(existante) => *existante = autorisation,
            None => self.octroyees.push(autorisation),
        }
        Ok(())
    }

    /// Retire l'autorisation correspondante, sur présentation d'une signature valide de `de`.
    pub fn retirer(
        &mut self,
        autorisation: &Autorisation,
        cle_de: &VerifyingKey,
    ) -> Result<(), Erreur> {
        autorisation.verifier(cle_de)?;
        self.octroyees.retain(|x| !x.meme_que(autorisation));
        Ok(())
    }

    /// Vrai si `de` a accordé `capacite` à `a`, non expirée, couvrant `appareil_cible`.
    pub fn est_autorise(
        &self,
        de: &AccountId,
        a: &AccountId,
        capacite: &str,
        appareil_cible: &DeviceId,
        maintenant_ms: u64,
    ) -> bool {
        self.octroyees.iter().any(|x| {
            &x.de == de
                && &x.a == a
                && x.capacite == capacite
                && x.expire_ms.is_none_or(|e| e > maintenant_ms)
                && x.portee.couvre(appareil_cible)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn cle(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn signee(
        de: &SigningKey,
        a: &SigningKey,
        portee: Portee,
        expire_ms: Option<u64>,
    ) -> Autorisation {
        let mut x = Autorisation {
            de: AccountId::de_cle(&de.verifying_key()),
            a: AccountId::de_cle(&a.verifying_key()),
            capacite: "annuaire".into(),
            portee,
            cree_ms: 1_000,
            expire_ms,
            signature: vec![],
        };
        x.signature = de.sign(&x.octets_signes()).to_bytes().to_vec();
        x
    }

    fn dev(n: u8) -> DeviceId {
        DeviceId::de_cle(&cle(n).verifying_key())
    }

    #[test]
    fn octroi_valide_est_autorise_et_idempotent() {
        let (de, a) = (cle(1), cle(2));
        let x = signee(&de, &a, Portee::Tous, None);
        let mut s = Autorisations::default();
        s.octroyer(x.clone(), &de.verifying_key()).unwrap();
        s.octroyer(x.clone(), &de.verifying_key()).unwrap();
        assert_eq!(s.octroyees.len(), 1);
        assert!(s.est_autorise(&x.de, &x.a, "annuaire", &dev(9), 5_000));
        assert!(!s.est_autorise(&x.a, &x.de, "annuaire", &dev(9), 5_000));
        assert!(!s.est_autorise(&x.de, &x.a, "autre", &dev(9), 5_000));
    }

    #[test]
    fn signature_fausse_refusee_sans_effet() {
        let (de, a) = (cle(1), cle(2));
        let mut x = signee(&de, &a, Portee::Tous, None);
        x.signature[0] ^= 0xff;
        let mut s = Autorisations::default();
        assert_eq!(
            s.octroyer(x, &de.verifying_key()),
            Err(Erreur::SignatureInvalide)
        );
        assert!(s.octroyees.is_empty());
    }

    #[test]
    fn signature_d_un_autre_compte_refusee() {
        let (de, a, intrus) = (cle(1), cle(2), cle(3));
        let x = signee(&intrus, &a, Portee::Tous, None);
        let mut s = Autorisations::default();
        assert!(s.octroyer(x, &de.verifying_key()).is_err());
        assert!(s.octroyees.is_empty());
    }

    #[test]
    fn portee_appareil_ne_couvre_pas_un_autre() {
        let (de, a) = (cle(1), cle(2));
        let x = signee(&de, &a, Portee::Appareil(dev(5)), None);
        let mut s = Autorisations::default();
        s.octroyer(x.clone(), &de.verifying_key()).unwrap();
        assert!(s.est_autorise(&x.de, &x.a, "annuaire", &dev(5), 5_000));
        assert!(!s.est_autorise(&x.de, &x.a, "annuaire", &dev(6), 5_000));
    }

    #[test]
    fn autorisation_expiree_est_fausse() {
        let (de, a) = (cle(1), cle(2));
        let x = signee(&de, &a, Portee::Tous, Some(10_000));
        let mut s = Autorisations::default();
        s.octroyer(x.clone(), &de.verifying_key()).unwrap();
        assert!(s.est_autorise(&x.de, &x.a, "annuaire", &dev(5), 9_999));
        assert!(!s.est_autorise(&x.de, &x.a, "annuaire", &dev(5), 10_000));
    }

    #[test]
    fn retirer_apres_octroyer_est_faux() {
        let (de, a) = (cle(1), cle(2));
        let x = signee(&de, &a, Portee::Tous, None);
        let mut s = Autorisations::default();
        s.octroyer(x.clone(), &de.verifying_key()).unwrap();
        let mut fausse = x.clone();
        fausse.signature[0] ^= 0xff;
        assert!(s.retirer(&fausse, &de.verifying_key()).is_err());
        assert_eq!(s.octroyees.len(), 1);
        s.retirer(&x, &de.verifying_key()).unwrap();
        assert!(!s.est_autorise(&x.de, &x.a, "annuaire", &dev(5), 5_000));
    }

    #[test]
    fn portee_serde() {
        assert_eq!(serde_json::to_string(&Portee::Tous).unwrap(), "\"tous\"");
        let p: Portee = serde_json::from_str("{\"appareil\":\"device-00000000\"}").unwrap();
        assert!(matches!(p, Portee::Appareil(_)));
    }
}
