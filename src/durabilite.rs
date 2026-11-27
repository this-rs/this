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
//! Ce que ce bloc ÉTABLIT, et ce qu'il n'établit PAS :
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

use std::path::{Path, PathBuf};

/// Les étapes observables d'une écriture durable, dans l'ORDRE où elles doivent
/// se produire. L'observateur sert à MESURER l'ordre (test) ; il ne le décide pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtapeEcriture {
    TemporaireCree,
    OctetsEcrits,
    FichierSynchronise,
    Renomme,
    RepertoireSynchronise,
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

/// Écriture atomique ET durable : temporaire → octets → `fsync` fichier →
/// `rename` → `fsync` répertoire. En cas d'échec avant le rename, le temporaire
/// est retiré : aucun résidu n'est laissé par une écriture qui a échoué.
pub fn ecrire_durable(chemin: &Path, octets: &[u8]) -> std::io::Result<()> {
    ecrire_durable_observe(chemin, octets, &mut |_| {})
}

/// Même chemin de code que [`ecrire_durable`], avec l'ORDRE des étapes exposé.
/// Il n'existe pas de seconde implémentation : l'observation ne change rien à ce
/// qui est exécuté.
pub fn ecrire_durable_observe(
    chemin: &Path,
    octets: &[u8],
    observe: &mut dyn FnMut(EtapeEcriture),
) -> std::io::Result<()> {
    use std::io::Write;
    let dossier = chemin.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "chemin sans répertoire parent",
        )
    })?;
    let tmp = nom_temporaire(chemin);
    let resultat = (|| -> std::io::Result<()> {
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
        // Le NOM du fichier après le rename : sans cela l'entrée de répertoire
        // peut être perdue, et le record avec elle.
        // POSIX uniquement : sous Windows, `File::open` sur un RÉPERTOIRE renvoie
        // ERROR_ACCESS_DENIED (il faudrait FILE_FLAG_BACKUP_SEMANTICS). Le journal
        // NTFS garantit la durabilité du nom autrement ; on n'émet donc pas de
        // sync_all explicite sur Windows, mais l'étape reste observable.
        #[cfg(unix)]
        {
            std::fs::File::open(dossier)?.sync_all()?;
        }
        observe(EtapeEcriture::RepertoireSynchronise);
        Ok(())
    })();
    if resultat.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    resultat
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
