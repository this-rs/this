//! L'invitation d'appairage : ce qu'un QR code porte d'un écran à une caméra.
//!
//! Le SAS ([`crate::sas`]) fait comparer deux codes par une personne ; l'invitation
//! ajoute ce que l'œil transmet et que le Wi-Fi ne touche pas : la **clé publique
//! attendue**. Le scanneur l'épingle, et refuse d'afficher le moindre code si la
//! poignée de main présente une autre clé — l'homme du milieu est alors détecté par
//! la machine, pas seulement par une comparaison humaine.
//!
//! # Forme
//!
//! `atelier://appairer?k=<clé publique, 64 hex>&a=<ip:port>&j=<jeton, 32 hex>&e=<unix>`
//!
//! - `k` : clé publique Ed25519 de l'appareil qui affiche le QR (32 octets, hex).
//! - `a` : où le joindre (adresse littérale, donc aucune résolution DNS à faire).
//! - `j` : jeton tiré au hasard, à **usage unique**, valable [`SECS_JETON`] secondes.
//! - `e` : échéance en secondes depuis l'époque, pour refuser un QR périmé **sans
//!   réseau** — le pair revérifie de son côté, l'échéance n'est jamais une preuve.
//!
//! Le lien ne porte **ni nom de personne ni secret durable** : la clé publique est
//! publique, le jeton meurt au premier usage.
//!
//! # Lecture stricte
//!
//! Un paramètre inconnu, un doublon, un schéma étranger, une clé de la mauvaise
//! taille : tout est refusé ([`InvitationError`]). Un lien est une entrée hostile —
//! il vient d'une caméra, donc de n'importe qui — et une lecture permissive ferait
//! entrer dans l'appairage des champs que personne n'a décidés.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use ed25519_dalek::VerifyingKey;

use crate::envelope::{from_hex, to_hex};

/// Le schéma du lien. Le préfixe `atelier` est celui que l'app déclare à iOS.
pub const SCHEMA: &str = "atelier://appairer";

/// Durée de vie d'un jeton, en secondes : cinq minutes.
pub const SECS_JETON: i64 = 300;

/// Longueur d'un jeton en hexadécimal : 32 caractères, soit 16 octets tirés au hasard.
pub const TAILLE_JETON: usize = 32;

/// Nombre d'invitations vivantes gardées à la fois (l'écran n'en affiche qu'une).
pub const CAPACITE_JETONS: usize = 8;

/// Ce qu'un QR code transporte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invitation {
    /// La clé publique de l'appareil qui invite : le scanneur l'épingle.
    pub cle: VerifyingKey,
    /// Où le joindre.
    pub adresse: SocketAddr,
    /// Le jeton à usage unique (hex, [`TAILLE_JETON`] caractères).
    pub jeton: String,
    /// Après quoi l'invitation ne vaut plus rien.
    pub expire: DateTime<Utc>,
}

/// Pourquoi un lien est refusé. Chaque variante dit **ce qui** est faux : un lien
/// photographié de travers et un lien hostile ne se diagnostiquent pas pareil.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvitationError {
    /// Ce n'est pas un lien d'appairage de l'Atelier.
    Schema,
    /// La clé n'est pas 32 octets en hexadécimal.
    Cle,
    /// L'adresse n'est pas `ip:port`.
    Adresse,
    /// Le jeton n'est pas [`TAILLE_JETON`] caractères hexadécimaux.
    Jeton,
    /// L'échéance est illisible.
    Echeance,
    /// Un paramètre inconnu, en double, ou manquant.
    Champ,
}

impl std::fmt::Display for InvitationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mot = match self {
            Self::Schema => "ce n'est pas un lien d'appairage",
            Self::Cle => "clé publique illisible (32 octets hexadécimaux attendus)",
            Self::Adresse => "adresse illisible (ip:port attendu)",
            Self::Jeton => "jeton illisible (32 caractères hexadécimaux attendus)",
            Self::Echeance => "échéance illisible",
            Self::Champ => "champ manquant, en double ou inconnu",
        };
        f.write_str(mot)
    }
}

impl std::error::Error for InvitationError {}

impl Invitation {
    /// Compose une invitation. Refuse une clé, une adresse ou un jeton mal formés :
    /// une invitation qui ne se relit pas ne doit pas être affichée.
    pub fn nouvelle(
        cle: VerifyingKey,
        adresse: SocketAddr,
        jeton: &str,
        maintenant: DateTime<Utc>,
    ) -> Result<Self, InvitationError> {
        if jeton.len() != TAILLE_JETON || from_hex(jeton).is_none() {
            return Err(InvitationError::Jeton);
        }
        Ok(Self {
            cle,
            adresse,
            jeton: jeton.to_owned(),
            expire: maintenant + chrono::Duration::seconds(SECS_JETON),
        })
    }

    /// Le lien à faire dessiner en QR code.
    pub fn lien(&self) -> String {
        format!(
            "{SCHEMA}?k={}&a={}&j={}&e={}",
            to_hex(self.cle.as_bytes()),
            self.adresse,
            self.jeton,
            self.expire.timestamp()
        )
    }

    /// Relit un lien. La validité de l'échéance n'est **pas** tranchée ici : voir
    /// [`Invitation::valide`]. Un lien périmé reste lisible, ce qui permet de dire
    /// « ce QR a expiré » plutôt que « ce n'est pas un QR ».
    pub fn analyser(lien: &str) -> Result<Self, InvitationError> {
        let reste = lien.strip_prefix(SCHEMA).ok_or(InvitationError::Schema)?;
        let requete = reste.strip_prefix('?').ok_or(InvitationError::Schema)?;
        if requete.is_empty() {
            return Err(InvitationError::Schema);
        }

        let mut cle = None;
        let mut adresse = None;
        let mut jeton = None;
        let mut echeance = None;
        for morceau in requete.split('&') {
            let (nom, valeur) = morceau.split_once('=').ok_or(InvitationError::Champ)?;
            // Chaque paramètre une seule fois : un doublon permettrait à un lien de
            // dire deux choses et au lecteur d'en choisir une.
            let place = match nom {
                "k" => &mut cle,
                "a" => &mut adresse,
                "j" => &mut jeton,
                "e" => &mut echeance,
                _ => return Err(InvitationError::Champ),
            };
            if place.replace(valeur).is_some() {
                return Err(InvitationError::Champ);
            }
        }

        let cle = cle.ok_or(InvitationError::Champ)?;
        let octets = from_hex(cle).ok_or(InvitationError::Cle)?;
        let cle = VerifyingKey::from_bytes(
            &<[u8; 32]>::try_from(octets.as_slice()).map_err(|_| InvitationError::Cle)?,
        )
        .map_err(|_| InvitationError::Cle)?;

        let adresse: SocketAddr = adresse
            .ok_or(InvitationError::Champ)?
            .parse()
            .map_err(|_| InvitationError::Adresse)?;

        let jeton = jeton.ok_or(InvitationError::Champ)?;
        if jeton.len() != TAILLE_JETON || from_hex(jeton).is_none() {
            return Err(InvitationError::Jeton);
        }

        let secondes: i64 = echeance
            .ok_or(InvitationError::Champ)?
            .parse()
            .map_err(|_| InvitationError::Echeance)?;
        let expire = Utc
            .timestamp_opt(secondes, 0)
            .single()
            .ok_or(InvitationError::Echeance)?;

        Ok(Self {
            cle,
            adresse,
            jeton: jeton.to_owned(),
            expire,
        })
    }

    /// L'invitation vaut-elle encore, à cet instant ?
    pub fn valide(&self, maintenant: DateTime<Utc>) -> bool {
        maintenant < self.expire
    }

    /// Le temps qu'il reste, borné à zéro (jamais négatif à l'écran).
    pub fn reste(&self, maintenant: DateTime<Utc>) -> Duration {
        (self.expire - maintenant).to_std().unwrap_or_default()
    }
}

/// Pourquoi un jeton est refusé.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusJeton {
    /// Jamais émis ici, déjà consommé, ou révoqué (l'écran s'est fermé).
    Inconnu,
    /// Émis, mais l'échéance est passée. Il est retiré en même temps.
    Expire,
    /// Le registre est plein, ou ce jeton y est déjà : l'aléa a produit un doublon.
    Registre,
}

impl std::fmt::Display for RefusJeton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mot = match self {
            Self::Inconnu => "jeton inconnu, déjà utilisé ou révoqué",
            Self::Expire => "jeton expiré",
            Self::Registre => "registre de jetons plein ou jeton déjà émis",
        };
        f.write_str(mot)
    }
}

impl std::error::Error for RefusJeton {}

/// Les jetons d'appairage vivants. Un jeton vaut **une fois** : [`Self::consommer`]
/// le retire, qu'il soit accepté ou périmé.
///
/// Le registre ne tire aucun aléa lui-même : le jeton est fourni par l'appelant, qui
/// a une source cryptographique. Un jeton deviné par la source d'en face ouvrirait
/// l'appairage à qui a photographié l'écran — l'aléa n'est pas un détail d'appel.
#[derive(Debug, Default)]
pub struct Jetons {
    internes: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl Jetons {
    pub fn nouveau() -> Self {
        Self {
            internes: Mutex::new(HashMap::new()),
        }
    }

    fn verrou(&self) -> std::sync::MutexGuard<'_, HashMap<String, DateTime<Utc>>> {
        // Un panique sous verrou laisserait le registre utilisable : on ne veut pas
        // qu'un fil mort rende l'appairage impossible.
        self.internes
            .lock()
            .unwrap_or_else(|empoisonne| empoisonne.into_inner())
    }

    /// Enregistre un jeton déjà tiré au hasard. Refuse un doublon (l'aléa aurait
    /// failli) et un registre plein plutôt que d'élaguer en silence.
    pub fn emettre(&self, jeton: &str, expire: DateTime<Utc>) -> Result<(), RefusJeton> {
        if jeton.len() != TAILLE_JETON || from_hex(jeton).is_none() {
            return Err(RefusJeton::Registre);
        }
        let mut internes = self.verrou();
        if internes.contains_key(jeton) || internes.len() >= CAPACITE_JETONS {
            return Err(RefusJeton::Registre);
        }
        internes.insert(jeton.to_owned(), expire);
        Ok(())
    }

    /// Consomme un jeton : `Ok` une seule fois par jeton, et seulement avant son
    /// échéance. Un jeton périmé rencontré ici est retiré au passage.
    pub fn consommer(&self, jeton: &str, maintenant: DateTime<Utc>) -> Result<(), RefusJeton> {
        let mut internes = self.verrou();
        let Some(expire) = internes.remove(jeton) else {
            return Err(RefusJeton::Inconnu);
        };
        if maintenant >= expire {
            return Err(RefusJeton::Expire);
        }
        Ok(())
    }

    /// Ferme l'écran : plus aucun jeton n'est valable. Rend le nombre révoqué.
    pub fn revoquer_tout(&self) -> usize {
        let mut internes = self.verrou();
        let n = internes.len();
        internes.clear();
        n
    }

    /// Combien de jetons sont encore valables (les périmés sont élagués).
    pub fn vivants(&self, maintenant: DateTime<Utc>) -> usize {
        let mut internes = self.verrou();
        internes.retain(|_, expire| maintenant < *expire);
        internes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn cle() -> VerifyingKey {
        SigningKey::from_bytes(&[7u8; 32]).verifying_key()
    }

    fn maintenant() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
    }

    fn invitation() -> Invitation {
        Invitation::nouvelle(
            cle(),
            "192.168.1.20:47821".parse().unwrap(),
            &"a".repeat(TAILLE_JETON),
            maintenant(),
        )
        .unwrap()
    }

    #[test]
    fn un_lien_se_relit_a_l_identique() {
        let depart = invitation();
        let lien = depart.lien();
        let relu = Invitation::analyser(&lien).unwrap();
        assert_eq!(relu, depart, "le lien ne revient pas sur ses pas : {lien}");
        assert_eq!(relu.lien(), lien, "le lien relu ne se réécrit pas pareil");
    }

    #[test]
    fn le_lien_ne_porte_ni_nom_ni_secret_durable() {
        let lien = invitation().lien();
        assert!(lien.starts_with("atelier://appairer?"), "{lien}");
        // Que des champs publics ou à usage unique, et rien d'autre.
        let champs: Vec<&str> = lien
            .split_once('?')
            .unwrap()
            .1
            .split('&')
            .map(|c| c.split_once('=').unwrap().0)
            .collect();
        assert_eq!(champs, ["k", "a", "j", "e"]);
    }

    #[test]
    fn une_echeance_est_a_cinq_minutes() {
        let inv = invitation();
        assert_eq!(
            inv.expire.timestamp() - maintenant().timestamp(),
            SECS_JETON
        );
        assert!(inv.valide(maintenant()));
        assert!(!inv.valide(maintenant() + chrono::Duration::seconds(SECS_JETON)));
        assert_eq!(
            inv.reste(maintenant() + chrono::Duration::seconds(SECS_JETON + 60)),
            Duration::ZERO,
            "le reste ne descend jamais sous zéro"
        );
    }

    #[test]
    fn un_schema_etranger_est_refuse() {
        let lien = invitation().lien().replace("atelier://", "autre://");
        assert_eq!(Invitation::analyser(&lien), Err(InvitationError::Schema));
    }

    #[test]
    fn une_cle_tronquee_est_refusee() {
        let lien = invitation().lien();
        let (avant, apres) = lien.split_once("k=").unwrap();
        let fin = &apres[2..]; // on retire deux caractères de la clé
        assert_eq!(
            Invitation::analyser(&format!("{avant}k={fin}")),
            Err(InvitationError::Cle)
        );
    }

    #[test]
    fn un_champ_inconnu_est_refuse() {
        let lien = invitation().lien();
        assert_eq!(
            Invitation::analyser(&format!("{lien}&x=1")),
            Err(InvitationError::Champ),
            "un champ que personne n'a décidé entre dans l'appairage"
        );
    }

    #[test]
    fn un_champ_en_double_est_refuse() {
        let lien = invitation().lien();
        assert_eq!(
            Invitation::analyser(&format!("{lien}&k={}", "b".repeat(64))),
            Err(InvitationError::Champ),
        );
    }

    #[test]
    fn une_adresse_sans_port_est_refusee() {
        let lien = invitation()
            .lien()
            .replace("192.168.1.20:47821", "192.168.1.20");
        assert_eq!(Invitation::analyser(&lien), Err(InvitationError::Adresse));
    }

    #[test]
    fn un_jeton_se_consomme_une_seule_fois() {
        let jetons = Jetons::nouveau();
        let j = "c".repeat(TAILLE_JETON);
        jetons
            .emettre(&j, maintenant() + chrono::Duration::seconds(60))
            .unwrap();
        assert_eq!(jetons.consommer(&j, maintenant()), Ok(()));
        assert_eq!(
            jetons.consommer(&j, maintenant()),
            Err(RefusJeton::Inconnu),
            "un jeton rejoué rouvrirait l'appairage à qui a photographié l'écran"
        );
    }

    #[test]
    fn un_jeton_expire_est_refuse_et_retire() {
        let jetons = Jetons::nouveau();
        let j = "d".repeat(TAILLE_JETON);
        jetons.emettre(&j, maintenant()).unwrap();
        assert_eq!(jetons.consommer(&j, maintenant()), Err(RefusJeton::Expire));
        // Retiré au passage : il ne peut pas redevenir valable.
        assert_eq!(jetons.vivants(maintenant()), 0);
        assert_eq!(jetons.consommer(&j, maintenant()), Err(RefusJeton::Inconnu));
    }

    #[test]
    fn un_jeton_inconnu_est_refuse() {
        let jetons = Jetons::nouveau();
        assert_eq!(
            jetons.consommer(&"e".repeat(TAILLE_JETON), maintenant()),
            Err(RefusJeton::Inconnu)
        );
    }

    #[test]
    fn fermer_l_ecran_revoque_tout() {
        let jetons = Jetons::nouveau();
        let j = "f".repeat(TAILLE_JETON);
        jetons
            .emettre(&j, maintenant() + chrono::Duration::seconds(600))
            .unwrap();
        assert_eq!(jetons.revoquer_tout(), 1);
        assert_eq!(jetons.consommer(&j, maintenant()), Err(RefusJeton::Inconnu));
        assert_eq!(jetons.revoquer_tout(), 0);
    }

    #[test]
    fn un_jeton_mal_forme_n_entre_pas_au_registre() {
        let jetons = Jetons::nouveau();
        assert_eq!(
            jetons.emettre("court", maintenant() + chrono::Duration::seconds(60)),
            Err(RefusJeton::Registre)
        );
        assert_eq!(
            jetons.emettre(
                &"z".repeat(TAILLE_JETON),
                maintenant() + chrono::Duration::seconds(60)
            ),
            Err(RefusJeton::Registre),
            "un jeton non hexadécimal ne doit pas passer"
        );
    }

    #[test]
    fn un_registre_plein_refuse_au_lieu_d_elaguer() {
        let jetons = Jetons::nouveau();
        let expire = maintenant() + chrono::Duration::seconds(600);
        for i in 0..CAPACITE_JETONS {
            jetons.emettre(&format!("{i:032x}"), expire).unwrap();
        }
        assert_eq!(
            jetons.emettre(&format!("{:032x}", CAPACITE_JETONS), expire),
            Err(RefusJeton::Registre)
        );
    }
}
