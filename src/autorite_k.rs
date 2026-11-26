//! Autorité d'identité logique K — source DURABLE et INDÉPENDANTE de la
//! référence résolue, de la classe et du contenu.
//!
//! K est l'identité LOGIQUE de l'opération (taxonomie K/F/V/G/E/B), un **u128**
//! directement compatible avec `ObservationId` (`from_u128` / `as_u128`). Toute
//! représentation textuelle n'est qu'un ENCODAGE de ce u128, jamais l'identité.
//!
//!   - **persistée avant publication** : le compteur monotone est écrit DURABLEMENT
//!     (`reconciliation::ecrire_durable`, la garantie de B) AVANT que `emettre`
//!     ne rende le K ;
//!   - **stable entre tentatives d'une même opération** : le consommateur
//!     RÉUTILISE le K émis (il ne ré-émet pas) ; la génération G distingue les
//!     tentatives ;
//!   - **distincte entre opérations indépendantes** : compteur u128 monotone
//!     DURABLE (jamais un PID + compteur local, jamais un `DefaultHasher`) ;
//!   - **jamais réinitialisée silencieusement** : un état illisible ou corrompu
//!     est REFUSÉ nommément (quarantaine), il n'est jamais régénéré en aveugle.
//!
//! Cette autorité vit HORS du `SubstrateStore` et ne sait rien de la référence
//! qui sera résolue : aucune circularité entre K et la référence. C'est l'UNIQUE
//! définition de K et l'UNIQUE autorité : aucune seconde n'existe.

use std::path::PathBuf;

use crate::reconciliation::ecrire_durable;

/// Autorité d'identité logique K. `prochain` est un u128 monotone persisté
/// durablement ; la première valeur émise est 1 (0 = « jamais émise »).
pub struct AutoriteK {
    chemin: PathBuf,
    prochain: u128,
}

/// Taille de l'état durable : un u128 little-endian (16 octets).
const TAILLE_ETAT: usize = 16;

impl AutoriteK {
    /// Ouvre l'autorité : restaure le compteur durable, ou initialise à 1 si le
    /// fichier n'existe pas encore (et le persiste AVANT tout usage). Un état
    /// existant mais illisible/corrompu est REFUSÉ nommément — jamais de
    /// réinitialisation silencieuse (un K émis doit rester distinct à jamais).
    pub fn ouvrir(chemin: PathBuf) -> std::io::Result<AutoriteK> {
        match std::fs::read(&chemin) {
            Ok(octets) => {
                if octets.len() != TAILLE_ETAT {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "état K corrompu : {} octets (attendu {TAILLE_ETAT})",
                            octets.len()
                        ),
                    ));
                }
                let mut b = [0u8; TAILLE_ETAT];
                b.copy_from_slice(&octets);
                let prochain = u128::from_le_bytes(b);
                if prochain == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "état K corrompu : compteur nul (jamais émis)".to_string(),
                    ));
                }
                Ok(AutoriteK { chemin, prochain })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let ak = AutoriteK {
                    chemin,
                    prochain: 1,
                };
                ak.persister()?;
                Ok(ak)
            }
            Err(e) => Err(e),
        }
    }

    /// Émet un K pour une NOUVELLE opération : u128 monotone, persisté AVANT
    /// d'être rendu (durabilité avant publication). Pour un rejeu ou une
    /// re-tentative de la MÊME opération, le consommateur RÉUTILISE le K émis.
    pub fn emettre(&mut self) -> std::io::Result<u128> {
        let k = self.prochain;
        self.prochain = self.prochain.checked_add(1).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "saturation du compteur K (u128)")
        })?;
        self.persister()?; // durable AVANT publication
        Ok(k)
    }

    /// Le prochain K qui sera émis (le compteur durable courant).
    pub fn prochain(&self) -> u128 {
        self.prochain
    }

    fn persister(&self) -> std::io::Result<()> {
        if let Some(parent) = self.chemin.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Réutilise la garantie DURABLE de B (temporaire → octets → fsync fichier
        // → rename → fsync répertoire). Ce n'est pas un second registre ad hoc :
        // c'est LE même mécanisme que le record OBRR.
        ecrire_durable(&self.chemin, &self.prochain.to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "autorite_k_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        d
    }

    #[test]
    fn compteur_u128_durable_et_monotone() {
        let d = dir("durabilite");
        let chemin = d.join("autorite.k");
        let mut ak = AutoriteK::ouvrir(chemin.clone()).expect("ouverture");
        assert_eq!(ak.prochain(), 1, "premier K = 1 (0 = jamais émise)");
        let k0 = ak.emettre().expect("émission 0");
        let k1 = ak.emettre().expect("émission 1");
        assert_eq!(k0, 1, "premier K émis = 1");
        assert_eq!(k1, 2, "second K émis = 2");
        assert_ne!(k0, k1, "deux opérations → deux K distincts");

        // Réouverture : compteur RESTAURÉ (durable), jamais régénéré.
        let mut relu = AutoriteK::ouvrir(chemin).expect("réouverture");
        assert_eq!(relu.prochain(), 3, "compteur restauré après redémarrage");
        let k2 = relu.emettre().expect("émission après réouverture");
        assert_eq!(k2, 3);
        assert_ne!(k2, k0);
        assert_ne!(k2, k1);

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corruption_refusee_pas_de_reinitialisation_silencieuse() {
        let d = dir("corruption");
        let chemin = d.join("autorite.k");
        let mut ak = AutoriteK::ouvrir(chemin.clone()).expect("ouverture");
        let _ = ak.emettre().expect("émission");
        // Corrompt l'état (tronqué) : la réouverture doit REFUSER, pas repartir à 1.
        std::fs::write(&chemin, b"tronque").expect("corruption");
        let err = AutoriteK::ouvrir(chemin.clone()).expect_err("refus attendu");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        // Un compteur nul est aussi refusé (jamais réinitialisé silencieusement).
        std::fs::write(&chemin, 0u128.to_le_bytes()).expect("compteur nul");
        let err = AutoriteK::ouvrir(chemin).expect_err("refus attendu");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let _ = std::fs::remove_dir_all(&d);
    }
}
