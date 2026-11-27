//! Durabilité d'un fichier d'état : écriture atomique ET durable, extraction
//! VERBATIM du bloc « Durabilité du record » de `src/reconciliation.rs`
//! (branche `integ/abdk`, `b78aa8ea`), dont les dépendances sont purement
//! `std::fs` — aucune dépendance `obrain`.
//!
//! Ce module est extrait pour une raison précise : `AutoriteK` (allocation de K)
//! n'a besoin QUE de cette primitive, alors que `reconciliation.rs` la portait
//! dans le même fichier que le codec OBRR et la réconciliation, eux dépendants du
//! contrat natif `obrain-substrate` (`ReferenceTentative`, `ResolutionTentative`,
//! `IntegriteTentative`) absent du pin `7a795370`. Extraire la primitive rend
//! l'allocation de K autonome SANS tirer le format persistant OBRR, qui n'a pas
//! de récupération fonctionnelle sur ce pin.
//!
//! # Ce que ce bloc ÉTABLIT, et ce qu'il n'établit PAS
//!
//! - ATOMICITÉ (par construction, mesurable) : les octets vont dans un temporaire
//!   du MÊME répertoire, puis `rename`. Un lecteur voit l'ancien contenu ou le
//!   nouveau, jamais un entre-deux. Un écrivain tué laisse au pire un
//!   `<fichier>.tmp.*` orphelin — jamais un fichier tronqué.
//! - DURABILITÉ POSIX (par l'ORDRE des opérations) : les octets sont `fsync`és
//!   AVANT le rename, et l'entrée de répertoire APRÈS. Sans le premier, le rename
//!   peut précéder les données ; sans le second, le rename lui-même peut être
//!   perdu. L'ordre est OBSERVABLE (`ecrire_durable_observe`), donc vérifié, pas
//!   seulement affirmé.
//! - NON ÉTABLIE : la survie à une coupure d'alimentation. `fsync` en est la
//!   condition nécessaire ; la preuve exigerait une panne matérielle réelle.
//!
//! # Le CONTRAT de durabilité (résultat fonctionnel, pas un commentaire)
//!
//! `ecrire_durable` ne retourne pas `io::Result<()>`. Une erreur APRÈS le
//! `rename` n'est PAS une absence de publication : la traiter comme telle
//! mentirait au consommateur. Elle retourne [`ResultatEcriture`], qui encode le
//! niveau de garantie RÉELLEMENT obtenu :
//!
//! | état | publication | durabilité |
//! |---|---|---|
//! | [`ResultatEcriture::EchecAvantPublication`] | NON | — (rien à durabiliser) |
//! | [`ResultatEcriture::PublicationDurable`] | OUI | QUALIFIÉE (`fsync` fichier + répertoire, POSIX) |
//! | [`ResultatEcriture::PublicationDurabiliteIndeterminee`] | OUI | INDÉTERMINÉE (`fsync` répertoire a échoué) |
//! | [`ResultatEcriture::GarantieIndisponible`] | OUI | INDISPONIBLE (primitive absente de la plateforme) |
//!
//! ## La durabilité d'entrée de répertoire sous Windows — garanties DOCUMENTÉES
//!
//! La durabilité POSIX de l'entrée de répertoire s'obtient par `fsync` d'un
//! descripteur de RÉPERTOIRE. Sous Windows, la chaîne équivalente n'existe pas
//! en `std::fs`, et les primitives natives ne fournissent pas la même garantie
//! pour un `rename` même volume :
//!
//! 1. **`File::open(répertoire)`** échoue : obtenir un handle de répertoire exige
//!    `FILE_FLAG_BACKUP_SEMANTICS` (`CreateFileW` : « You must set this flag to
//!    obtain a handle to a directory »), que `std::fs` n'expose pas. Sans ce
//!    drapeau, `CreateFileW` répond `ERROR_ACCESS_DENIED` (code 5).
//! 2. **`FlushFileBuffers`** (ce qu'appelle `File::sync_all` sous Windows) exige
//!    un handle ayant le droit `GENERIC_WRITE` (« The file handle must have the
//!    GENERIC_WRITE access right »). Or un répertoire s'ouvre en `GENERIC_READ`
//!    (« Specify the GENERIC_READ access right instead »). Même avec
//!    `FILE_FLAG_BACKUP_SEMANTICS`, `FlushFileBuffers` ne peut donc pas
//!    synchroniser l'entrée de répertoire.
//! 3. **`MoveFileExW(MOVEFILE_WRITE_THROUGH)`** ne garantit le flush que pour un
//!    déplacement « copy and delete » (volume différent) : « Setting this value
//!    guarantees that a move performed as a copy and delete operation is flushed
//!    to disk ». Pour un `rename` même volume (ce que fait ce module), ce
//!    drapeau ne documente PAS la durabilité de l'entrée de répertoire.
//!
//! Aucune de ces trois primitives ne fournit l'équivalent DOCUMENTÉ du `fsync`
//! de répertoire POSIX pour un `rename` atomique même volume. Ce module ne le
//! PRÉSUME pas : sous Windows il retourne [`ResultatEcriture::GarantieIndisponible`],
//! et c'est au consommateur ([`crate::autorite_k`]) de REFUSER de confirmer une
//! allocation durable sur cette garantie insuffisante — jamais de réattribution
//! silencieuse.

use std::path::{Path, PathBuf};

/// Les étapes observables d'une écriture durable, dans l'ORDRE où elles doivent
/// se produire. L'observateur sert à MESURER l'ordre (test) ; il ne le décide pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtapeEcriture {
    TemporaireCree,
    OctetsEcrits,
    FichierSynchronise,
    Renomme,
    /// POSIX : l'entrée de répertoire a été synchronisée (durabilité QUALIFIÉE).
    RepertoireSynchronise,
    /// Le `rename` a eu lieu, mais la synchronisation de l'entrée de répertoire
    /// a ÉCHOUÉ (durabilité INDÉTERMINÉE).
    RepertoireNonSynchronise,
    /// Le `rename` a eu lieu, mais la primitive de synchronisation d'entrée de
    /// répertoire est INDISPONIBLE sur la plateforme (Windows).
    RepertoireIndisponible,
}

fn horloge_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Nom du temporaire : MÊME répertoire que la cible (sinon `rename` traverserait
/// un système de fichiers et perdrait son atomicité), unique par processus + temps.
pub fn nom_temporaire(chemin: &Path) -> PathBuf {
    let mut nom = chemin.as_os_str().to_os_string();
    nom.push(format!(".tmp.{}.{}", std::process::id(), horloge_nanos()));
    PathBuf::from(nom)
}

/// Le résultat d'une écriture durable, qui ENCODE le niveau de garantie réel.
///
/// Ce n'est pas un `io::Result<()>` : une erreur après le `rename` (publication
/// EFFECTUÉE) n'est pas une absence de publication, et ne doit jamais être
/// confondue avec elle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultatEcriture {
    /// Échec AVANT publication : aucun contenu n'a été publié, le temporaire est
    /// retiré. Rejouer est sans danger (l'état sur disque est inchangé).
    EchecAvantPublication {
        /// L'erreur qui a empêché la publication (avant le `rename`).
        cause: String,
    },
    /// Publication EFFECTUÉE, durabilité INDÉTERMINÉE : le `rename` a réussi (le
    /// contenu est publié et lisible), mais la synchronisation de l'entrée de
    /// répertoire a ÉCHOUÉ sur une plateforme qui en est capable. Une coupure
    /// d'alimentation peut perdre le `rename`.
    PublicationDurabiliteIndeterminee {
        /// Pourquoi la synchronisation de l'entrée de répertoire a échoué.
        cause: String,
    },
    /// Publication DURABLE qualifiée : `fsync` du fichier AVANT le `rename`, puis
    /// `fsync` de l'entrée de répertoire APRÈS. C'est la garantie POSIX complète,
    /// mesurable par l'ordre des étapes ([`EtapeEcriture`]).
    PublicationDurable,
    /// Publication EFFECTUÉE, garantie INDISPONIBLE sur la plateforme : la
    /// primitive qui synchronise l'entrée de répertoire n'existe pas avec
    /// `std::fs` sur cette plateforme (Windows). Le contenu est publié, la
    /// durabilité de l'entrée de répertoire ne peut pas être établie par ce code.
    GarantieIndisponible {
        /// Décrit la plateforme et la primitive manquante.
        plateforme: String,
    },
}

impl ResultatEcriture {
    /// La publication a-t-elle eu lieu ? (le contenu est visible et lisible).
    /// Seul [`ResultatEcriture::EchecAvantPublication`] répond `false`.
    pub fn publication_effectuee(&self) -> bool {
        !matches!(self, ResultatEcriture::EchecAvantPublication { .. })
    }

    /// La durabilité est-elle QUALIFIÉE (`fsync` fichier + répertoire) ?
    pub fn durable_qualifiee(&self) -> bool {
        matches!(self, ResultatEcriture::PublicationDurable)
    }
}

/// Écriture atomique ; le résultat ENCODE le niveau de durabilité réellement
/// obtenu ([`ResultatEcriture`]). Voir [`ecrire_durable_observe`] pour l'ordre.
pub fn ecrire_durable(chemin: &Path, octets: &[u8]) -> ResultatEcriture {
    ecrire_durable_observe(chemin, octets, &mut |_| {})
}

/// Même chemin de code que [`ecrire_durable`], avec l'ORDRE des étapes exposé.
/// Il n'existe pas de seconde implémentation : l'observation ne change rien à ce
/// qui est exécuté.
pub fn ecrire_durable_observe(
    chemin: &Path,
    octets: &[u8],
    observe: &mut dyn FnMut(EtapeEcriture),
) -> ResultatEcriture {
    use std::io::Write;
    let Some(dossier) = chemin.parent() else {
        return ResultatEcriture::EchecAvantPublication {
            cause: "chemin sans répertoire parent".to_string(),
        };
    };
    let tmp = nom_temporaire(chemin);

    // Phase AVANT publication : tout échec ici laisse le disque inchangé, et le
    // temporaire est retiré — aucun résidu d'une écriture qui n'a pas publié.
    let avant = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        observe(EtapeEcriture::TemporaireCree);
        f.write_all(octets)?;
        observe(EtapeEcriture::OctetsEcrits);
        // Les OCTETS avant le rename : c'est l'ordre qui rend l'échange sûr.
        f.sync_all()?;
        observe(EtapeEcriture::FichierSynchronise);
        drop(f);
        std::fs::rename(&tmp, chemin)?;
        observe(EtapeEcriture::Renomme);
        Ok(())
    })();
    if let Err(e) = avant {
        let _ = std::fs::remove_file(&tmp);
        return ResultatEcriture::EchecAvantPublication {
            cause: e.to_string(),
        };
    }

    // Phase APRÈS publication : le contenu EST publié (le `rename` a réussi).
    // Reste la durabilité de l'entrée de répertoire. Un échec ici ne retire PAS
    // la publication : il réduit seulement la garantie de durabilité.
    match synchroniser_repertoire(dossier) {
        Ok(()) => {
            observe(EtapeEcriture::RepertoireSynchronise);
            ResultatEcriture::PublicationDurable
        }
        Err(ErreurRepertoire::Indisponible(plateforme)) => {
            observe(EtapeEcriture::RepertoireIndisponible);
            ResultatEcriture::GarantieIndisponible { plateforme }
        }
        Err(ErreurRepertoire::Echec(e)) => {
            observe(EtapeEcriture::RepertoireNonSynchronise);
            ResultatEcriture::PublicationDurabiliteIndeterminee {
                cause: e.to_string(),
            }
        }
    }
}

/// Pourquoi la synchronisation de l'entrée de répertoire n'a pas eu lieu.
enum ErreurRepertoire {
    /// La primitive est INDISPONIBLE sur la plateforme (Windows) : ce n'est pas
    /// un échec transitoire, retenter n'y changera rien.
    #[cfg_attr(not(windows), allow(dead_code))] // construit seulement par la branche Windows
    Indisponible(String),
    /// La primitive existe mais a échoué (erreur réelle).
    #[cfg_attr(windows, allow(dead_code))] // construit seulement par la branche non-Windows
    Echec(std::io::Error),
}

/// POSIX (et assimilés) : `fsync` du descripteur de répertoire. `File::open`
/// ouvre un répertoire en lecture seule et `sync_all` le synchronise.
#[cfg(not(windows))]
fn synchroniser_repertoire(dossier: &Path) -> Result<(), ErreurRepertoire> {
    match std::fs::File::open(dossier) {
        Ok(dir) => dir.sync_all().map_err(ErreurRepertoire::Echec),
        Err(e) => Err(ErreurRepertoire::Echec(e)),
    }
}

/// Windows : la primitive est INDISPONIBLE via `std::fs`. Voir la section
/// « garanties DOCUMENTÉES » du module. On ne PRÉSUME PAS l'équivalence avec
/// `fsync` : on représente l'indisponibilité dans le résultat.
#[cfg(windows)]
fn synchroniser_repertoire(_dossier: &Path) -> Result<(), ErreurRepertoire> {
    Err(ErreurRepertoire::Indisponible(
        "Windows : la durabilité de l'entrée de répertoire n'est pas disponible via std::fs. \
         Ouvrir un handle de répertoire exige FILE_FLAG_BACKUP_SEMANTICS (non exposé) ; \
         FlushFileBuffers exige GENERIC_WRITE (refusé sur un répertoire) ; \
         MoveFileExW(MOVEFILE_WRITE_THROUGH) ne garantit le flush que pour un déplacement \
         copy+delete, pas pour un rename même volume."
            .to_string(),
    ))
}

/// Les temporaires ORPHELINS d'un écrivain mort : `<record>.tmp.*`. Le record
/// publié n'est jamais touché. Un temporaire n'est pas un record : il n'est ni
/// lu, ni interprété, ni promu — seulement constaté.
pub fn temporaires_orphelins(chemin: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dossier = match chemin.parent() {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };
    let Some(nom) = chemin.file_name().and_then(|n| n.to_str()) else {
        return Ok(Vec::new());
    };
    let prefixe = format!("{nom}.tmp.");
    let mut orphelins = Vec::new();
    for entree in std::fs::read_dir(dossier)? {
        let entree = entree?;
        if entree.file_name().to_string_lossy().starts_with(&prefixe) {
            orphelins.push(entree.path());
        }
    }
    orphelins.sort();
    Ok(orphelins)
}
