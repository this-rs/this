//! Autorité d'identité logique K — source DURABLE et INDÉPENDANTE de la
//! référence résolue, de la classe et du contenu.
//!
//! K est l'identité LOGIQUE de l'opération (taxonomie K/F/V/G/E/B), un **u128**
//! directement compatible avec `ObservationId` (`from_u128` / `as_u128`). Toute
//! représentation textuelle n'est qu'un ENCODAGE de ce u128, jamais l'identité.
//!
//! ## Ce que cette autorité fait
//!
//! - **allocation ATOMIQUE** : lire, incrémenter, persister et rendre se font
//!   sous UN verrou exclusif qui couvre TOUTE la décision (`emettre_observe`).
//!   Aucune fenêtre entre la lecture de l'état et son écriture : deux instances
//!   du même processus, ou deux processus, ne peuvent pas calculer le même K.
//!   La lecture de l'état ne se fait pas à l'ouverture puis « plus tard » : elle
//!   est refaite **sous verrou** à chaque émission (une vue d'ouverture est une
//!   vue périmée).
//! - **persistée avant publication** : la valeur SUIVANTE est écrite
//!   DURABLEMENT (`reconciliation::ecrire_durable`, la garantie de B : temporaire
//!   → octets → `fsync` fichier → `rename` → `fsync` répertoire) AVANT que le K
//!   ne soit rendu. Un K potentiellement publié ne peut donc pas être réattribué
//!   après une interruption ; au pire il est BRÛLÉ (saut de compteur), ce qui est
//!   admissible — la réutilisation, elle, ne l'est pas.
//! - **stable entre tentatives d'une même opération** : le consommateur RÉUTILISE
//!   le K émis (il ne ré-émet pas) ; la génération G distingue les tentatives.
//! - **distincte entre opérations indépendantes** : compteur u128 monotone
//!   DURABLE (jamais un PID + compteur local, jamais un `DefaultHasher`).
//! - **jamais réinitialisée silencieusement** : un état illisible, nul, disparu
//!   sous verrou ou RÉGRESSÉ est REFUSÉ nommément (quarantaine/erreur), il n'est
//!   jamais régénéré en aveugle. La saturation (`u128::MAX`) est refusée sans
//!   repli.
//!
//! ## Ce que cette autorité n'établit PAS — six notions à ne pas confondre
//!
//! Cette correction ne résout PAS « l'identité » en général. Elle rend
//! l'ALLOCATION atomique. Les six grandeurs restent distinctes :
//!
//! | notion | ce qu'elle est ici | borne mesurable |
//! |---|---|---|
//! | **atomicité d'allocation** | un verrou exclusif (`File::lock`, sémantique `flock`) sur le fichier `*.verrou`, tenu de la lecture à la publication | locale au système de fichiers qui porte ce fichier ; un verrou consultatif ne protège pas d'un écrivain qui l'ignore, ni de deux COPIES distinctes de l'état |
//! | **persistance** | `ecrire_durable` : `fsync` fichier avant `rename`, `fsync` répertoire après | survivre à la mort du processus est établi ; survivre à une coupure d'alimentation n'est pas prouvé (`fsync` en est la condition nécessaire, pas la démonstration) |
//! | **identité d'autorité** | K = u128 logique, distinct par opération, stable par tentative | n'atteste AUCUNE provenance : un K dit « telle opération de CETTE autorité », pas « de tel client » |
//! | **domaine canonique** | LE fichier d'état est le domaine : deux fichiers = deux autorités indépendantes, qui peuvent émettre les MÊMES K | aucune unicité globale : K n'est unique que dans son domaine |
//! | **continuité** | après redémarrage le compteur reprend ; un saut (valeur brûlée) n'est jamais réattribué ; une régression du compteur est refusée **dans un processus vivant** | une RESTAURATION historique du fichier (retour arrière d'un snapshot) n'est PAS détectable ici : le compteur repartirait de la valeur restaurée. Seul un témoin externe (journal d'émission, ancre hors du domaine) pourrait la constater |
//! | **domaine backend** | `AutoriteK` ne sait RIEN du backend : K n'en dérive pas | `ReferenceTentative` ne constitue pas sa propre preuve d'origine — deux bases partageant `writer.id` produisent des références identiques octet pour octet. Ce domaine doit venir d'une autorité indépendante, pas d'un verrou local |
//!
//! Le verrou porte sur un fichier DISTINCT de l'état (`<état>.verrou`) : le
//! `rename` atomique qui remplace `<état>` ne remplace donc jamais l'objet qui
//! porte le verrou. Verrouiller l'objet renommé reviendrait à changer de verrou
//! à chaque émission.
//!
//! Cette autorité vit HORS du `SubstrateStore` et ne sait rien de la référence
//! qui sera résolue : aucune circularité entre K et la référence. C'est l'UNIQUE
//! définition de K et l'UNIQUE autorité : aucune seconde n'existe.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

// La primitive de durabilité est extraite dans `crate::durabilite` : elle ne dépend
// pas d'Obrain, contrairement au reste de `reconciliation` (codec OBRR +
// réconciliation), qui exige un contrat natif absent du pin `7a795370`.
use crate::durabilite::ecrire_durable;

/// Taille de l'état durable : un u128 little-endian (16 octets).
const TAILLE_ETAT: usize = 16;

/// Suffixe du fichier de verrou. SÉPARÉ de l'état : `ecrire_durable` renomme
/// l'état, et un verrou posé sur l'objet renommé serait un autre verrou à chaque
/// émission (donc aucun verrou).
const SUFFIXE_VERROU: &str = ".verrou";

/// Les étapes OBSERVABLES d'une émission, dans l'ORDRE où elles doivent se
/// produire. L'observateur sert à MESURER cet ordre (témoin discriminant) ; il ne
/// le décide pas : `emettre` n'existe pas en deux implémentations.
///
/// L'observateur s'exécute PENDANT que le verrou est tenu : il ne doit pas
/// ré-entrer dans l'autorité (`prochain_durable`, `emettre`) sur le même
/// chemin, sous peine de s'attendre lui-même.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtapeEmission {
    /// Le verrou exclusif est tenu : il couvre TOUTE la décision qui suit.
    VerrouPris,
    /// L'état durable a été RELU sous verrou (valeur courante du compteur).
    EtatLu(u128),
    /// La valeur SUIVANTE est déjà persistée durablement (`fsync` faits), et le
    /// K à publier n'est pas encore rendu. C'est la garantie « persistance AVANT
    /// publication », mesurable : à cette étape l'état sur disque vaut `suivant`.
    SuivantPersiste(u128),
    /// Le K (`suivant - 1`) est rendu à l'appelant.
    Publie(u128),
}

/// Verrou exclusif libéré à la destruction — et par le noyau à la mort du
/// processus, ce qui rend un verrou orphelin impossible.
struct VerrouExclusif(File);

impl Drop for VerrouExclusif {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Autorité d'identité logique K. `prochain` est une VUE de l'état durable,
/// rafraîchie sous verrou ; la source de vérité reste le fichier.
#[derive(Debug)]
pub struct AutoriteK {
    chemin: PathBuf,
    verrou: PathBuf,
    prochain: u128,
    /// Dernier K émis PAR CETTE INSTANCE. Sert à refuser une régression du
    /// compteur (état restauré sous nos pieds) plutôt que de réattribuer un K.
    dernier_emis: Option<u128>,
}

fn erreur_invalide(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn chemin_verrou(chemin: &Path) -> PathBuf {
    let mut nom = chemin.as_os_str().to_os_string();
    nom.push(SUFFIXE_VERROU);
    PathBuf::from(nom)
}

/// Lit l'état durable. `Ok(None)` = le fichier n'existe pas encore (création
/// initiale possible, décidée SOUS verrou) ; `Err` = état présent mais
/// inexploitable, refusé nommément.
fn lire_etat(chemin: &Path) -> io::Result<Option<u128>> {
    match std::fs::read(chemin) {
        Ok(octets) => {
            if octets.len() != TAILLE_ETAT {
                return Err(erreur_invalide(format!(
                    "état K corrompu : {} octets (attendu {TAILLE_ETAT})",
                    octets.len()
                )));
            }
            let mut b = [0u8; TAILLE_ETAT];
            b.copy_from_slice(&octets);
            let valeur = u128::from_le_bytes(b);
            if valeur == 0 {
                return Err(erreur_invalide(
                    "état K corrompu : compteur nul (jamais émis)".to_string(),
                ));
            }
            Ok(Some(valeur))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

impl AutoriteK {
    /// Ouvre l'autorité : restaure le compteur durable, ou l'initialise à 1 si
    /// le fichier n'existe pas encore — décision prise SOUS VERROU, puis
    /// persistée, pour qu'un second ouvreur ne puisse pas écraser un compteur
    /// déjà avancé par une création concurrente.
    ///
    /// Un état existant mais illisible/corrompu est REFUSÉ nommément — jamais de
    /// réinitialisation silencieuse (un K émis doit rester distinct à jamais).
    pub fn ouvrir(chemin: PathBuf) -> io::Result<AutoriteK> {
        let ak = AutoriteK {
            verrou: chemin_verrou(&chemin),
            chemin,
            prochain: 0,
            dernier_emis: None,
        };
        let _v = ak.verrouiller()?;
        let prochain = match lire_etat(&ak.chemin)? {
            Some(valeur) => valeur,
            None => {
                // Création initiale durable : 1 (0 = « jamais émise »).
                ecrire_durable(&ak.chemin, &1u128.to_le_bytes())?;
                1
            }
        };
        Ok(AutoriteK { prochain, ..ak })
    }

    /// Émet un K pour une NOUVELLE opération : u128 monotone, persisté AVANT
    /// d'être rendu (durabilité avant publication). Pour un rejeu ou une
    /// re-tentative de la MÊME opération, le consommateur RÉUTILISE le K émis.
    pub fn emettre(&mut self) -> io::Result<u128> {
        self.emettre_observe(&mut |_| {})
    }

    /// Même chemin de code que [`AutoriteK::emettre`], avec l'ORDRE des étapes
    /// exposé. Il n'existe pas de seconde implémentation.
    ///
    /// Le verrou est tenu de `VerrouPris` à `Publie` : lecture de l'état,
    /// contrôle de régression, saturation, écriture durable, mise à jour de la
    /// vue, publication. C'est cette étendue — et non la seule écriture finale —
    /// qui rend l'allocation atomique.
    pub fn emettre_observe(&mut self, observe: &mut dyn FnMut(EtapeEmission)) -> io::Result<u128> {
        let _v = self.verrouiller()?;
        observe(EtapeEmission::VerrouPris);

        // La lecture est faite ICI, sous verrou — pas à l'ouverture.
        let lu = lire_etat(&self.chemin)?.ok_or_else(|| {
            erreur_invalide(
                "état K disparu sous verrou : aucune réinitialisation silencieuse".to_string(),
            )
        })?;
        observe(EtapeEmission::EtatLu(lu));

        // Un compteur qui RECULE sous nos pieds est un état restauré : refuser
        // plutôt que réattribuer un K déjà émis.
        if let Some(dernier) = self.dernier_emis
            && lu <= dernier
        {
            return Err(erreur_invalide(format!(
                "compteur K régressé : état durable {lu} ≤ dernier K émis {dernier} — \
                 réutilisation refusée (restauration du fichier ?)"
            )));
        }

        let suivant = lu.checked_add(1).ok_or_else(|| {
            io::Error::other(format!(
                "saturation du compteur K (u128) : {lu} est le dernier K admissible"
            ))
        })?;

        // PERSISTANCE AVANT PUBLICATION : si le processus meurt ici, `lu` est
        // brûlé — jamais réattribué.
        ecrire_durable(&self.chemin, &suivant.to_le_bytes())?;
        observe(EtapeEmission::SuivantPersiste(suivant));

        self.prochain = suivant;
        self.dernier_emis = Some(lu);
        observe(EtapeEmission::Publie(lu));
        Ok(lu)
    }

    /// Vue CACHÉE du prochain K, rafraîchie à la dernière opération faite SOUS
    /// verrou par cette instance. Ce n'est pas une lecture concurrente : pour
    /// l'état réel, voir [`AutoriteK::prochain_durable`].
    pub fn prochain(&self) -> u128 {
        self.prochain
    }

    /// L'état durable RÉEL, relu sous verrou (donc cohérent avec une autre
    /// instance ou un autre processus).
    pub fn prochain_durable(&self) -> io::Result<u128> {
        let _v = self.verrouiller()?;
        lire_etat(&self.chemin)?
            .ok_or_else(|| erreur_invalide("état K absent sous verrou".to_string()))
    }

    fn verrouiller(&self) -> io::Result<VerrouExclusif> {
        if let Some(parent) = self.verrou.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.verrou)?;
        // Exclusif et bloquant. Le noyau libère le verrou à la fermeture du
        // descripteur — y compris si le processus meurt sans destructeurs.
        f.lock()?;
        Ok(VerrouExclusif(f))
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

    #[test]
    fn deux_instances_meme_processus_n_emettent_jamais_le_meme_k() {
        let d = dir("instances");
        let chemin = d.join("autorite.k");
        // Interleaving DÉTERMINISTE : les deux instances lisent l'état AVANT
        // toute émission (c'était exactement le défaut : lecture à l'ouverture).
        let mut a = AutoriteK::ouvrir(chemin.clone()).expect("ouverture A");
        let mut b = AutoriteK::ouvrir(chemin).expect("ouverture B");
        let ka = a.emettre().expect("K de A");
        let kb = b.emettre().expect("K de B");
        assert_ne!(ka, kb, "deux instances ne peuvent pas émettre le même K");
        assert_eq!(
            (ka, kb),
            (1, 2),
            "la seconde instance voit l'émission de la première"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ordre_mesure_verrou_puis_lecture_puis_persistance_puis_publication() {
        let d = dir("ordre");
        let chemin = d.join("autorite.k");
        let mut ak = AutoriteK::ouvrir(chemin.clone()).expect("ouverture");
        let chemin_observation = chemin.clone();
        let mut etapes: Vec<EtapeEmission> = Vec::new();
        let mut sur_disque_a_la_persistance: Option<u128> = None;
        let k = ak
            .emettre_observe(&mut |e| {
                if let EtapeEmission::SuivantPersiste(suivant) = e {
                    let octets = std::fs::read(&chemin_observation).expect("état lisible");
                    let mut b = [0u8; TAILLE_ETAT];
                    b.copy_from_slice(&octets);
                    sur_disque_a_la_persistance = Some(u128::from_le_bytes(b));
                    let _ = suivant;
                }
                etapes.push(e);
            })
            .expect("émission");
        assert_eq!(k, 1);
        assert_eq!(
            etapes,
            vec![
                EtapeEmission::VerrouPris,
                EtapeEmission::EtatLu(1),
                EtapeEmission::SuivantPersiste(2),
                EtapeEmission::Publie(1),
            ],
            "l'ordre est mesuré, pas affirmé"
        );
        assert_eq!(
            sur_disque_a_la_persistance,
            Some(2),
            "l'état durable porte déjà k+1 AVANT que k ne soit publié"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn saturation_refusee_sans_repli() {
        let d = dir("saturation");
        std::fs::create_dir_all(&d).expect("répertoire");
        let chemin = d.join("autorite.k");
        std::fs::write(&chemin, u128::MAX.to_le_bytes()).expect("état saturé");
        let mut ak = AutoriteK::ouvrir(chemin.clone()).expect("ouverture");
        let err = ak.emettre().expect_err("saturation refusée");
        assert!(
            err.to_string().contains("saturation"),
            "refus nommé : {err}"
        );
        // Aucun repli : l'état n'a ni bouclé à 0, ni été réinitialisé à 1.
        let octets = std::fs::read(&chemin).expect("état");
        let mut b = [0u8; TAILLE_ETAT];
        b.copy_from_slice(&octets);
        assert_eq!(u128::from_le_bytes(b), u128::MAX, "état inchangé");
        let _ = std::fs::remove_dir_all(&d);
    }
}
