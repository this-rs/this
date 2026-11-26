//! Primitive d'écriture durable — l'ORDRE est MESURÉ, pas affirmé.
//!
//! Témoin ADAPTÉ de `tests/durabilite_obrr.rs::ecriture_durable_ordre_observe`
//! (branche `integ/abdk`, `b78aa8ea`). La source fabriquait ses octets en encodant
//! un enregistrement OBRR ; le format OBRR n'est PAS porté ici (la récupération
//! fonctionnelle qu'il exige n'existe pas sur le pin `7a795370`). La primitive, elle,
//! ne connaît que des OCTETS : le témoin lui en donne d'opaques, et la propriété
//! mesurée est identique.
//!
//! Ce que ces témoins DISCRIMINENT :
//!   - si l'ordre des étapes change (rename avant `fsync` du fichier, ou `fsync`
//!     du répertoire avant le rename), `ecriture_durable_ordre_observe` échoue ;
//!   - si le rename n'était pas un remplacement, `la_seconde_ecriture_remplace`
//!     échouerait ;
//!   - si un temporaire était promu en fichier publié, ou promu par erreur,
//!     `temporaire_orphelin_constate_jamais_promu` échouerait.

use std::path::PathBuf;

use this::durabilite::{
    EtapeEcriture, ecrire_durable, ecrire_durable_observe, temporaires_orphelins,
};

fn dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "durap_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("création du répertoire de travail");
    d
}

/// Les cinq étapes, dans l'ordre EXIGÉ par la durabilité POSIX.
#[test]
fn ecriture_durable_ordre_observe() {
    let d = dir("ordre");
    let chemin = d.join("etat.k");
    let octets: Vec<u8> = (0u8..32).collect();

    let mut etapes: Vec<EtapeEcriture> = Vec::new();
    ecrire_durable_observe(&chemin, &octets, &mut |e| etapes.push(e)).expect("écriture durable");

    assert_eq!(
        etapes,
        vec![
            EtapeEcriture::TemporaireCree,
            EtapeEcriture::OctetsEcrits,
            // `fsync` du FICHIER avant le rename : sans lui, le nom peut être
            // publié avant les octets qu'il désigne.
            EtapeEcriture::FichierSynchronise,
            EtapeEcriture::Renomme,
            // `fsync` du RÉPERTOIRE après le rename : sans lui, le rename
            // lui-même peut être perdu.
            EtapeEcriture::RepertoireSynchronise,
        ],
        "les cinq étapes, dans cet ordre — mesuré, pas affirmé"
    );

    let relu = std::fs::read(&chemin).expect("lecture");
    assert_eq!(
        relu, octets,
        "le contenu publié est exactement celui demandé"
    );
    assert!(
        temporaires_orphelins(&chemin).expect("balayage").is_empty(),
        "aucun temporaire ne subsiste après une écriture réussie"
    );

    let _ = std::fs::remove_dir_all(&d);
}

/// Une seconde écriture REMPLACE, elle n'ajoute pas (rename, pas append).
#[test]
fn la_seconde_ecriture_remplace() {
    let d = dir("remplace");
    let chemin = d.join("etat.k");
    ecrire_durable(&chemin, b"premier").expect("écriture 1");
    assert_eq!(std::fs::read(&chemin).expect("lecture 1"), b"premier");
    ecrire_durable(&chemin, b"second-plus-long").expect("écriture 2");
    assert_eq!(
        std::fs::read(&chemin).expect("lecture 2"),
        b"second-plus-long",
        "le rename remplace : aucun résidu du premier contenu"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Un temporaire est CONSTATÉ, jamais promu : le fichier publié n'existe pas
/// parce qu'une écriture a été interrompue avant son rename.
#[test]
fn temporaire_orphelin_constate_jamais_promu() {
    let d = dir("orphelin");
    let chemin = d.join("etat.k");
    let temporaire = d.join("etat.k.tmp.999.123456");
    std::fs::write(&temporaire, b"ecriture-interrompue").expect("temporaire simulé");

    let orphelins = temporaires_orphelins(&chemin).expect("balayage");
    assert_eq!(
        orphelins,
        vec![temporaire.clone()],
        "le temporaire est constaté"
    );
    assert!(
        !chemin.exists(),
        "aucun fichier publié : un temporaire n'est jamais promu par le balayage"
    );

    let _ = std::fs::remove_dir_all(&d);
}
