//! Durabilité du record OBRR — reconstruction interprocessus et restauration
//! contrôlée de l'autorité `Attribution`.
//!
//! Ce test qualifie SIX propriétés, sur la branche `chore/h3-native-v2` :
//!   1. conservation EFFECTIVE du record (médium durable + écriture atomique) ;
//!   2. écriture interrompue → refus NOMMÉ (pas de reconstruction silencieuse) ;
//!   3. reconstruction dans un PROCESSUS INDÉPENDANT (spawn réel, pas close/open) ;
//!   4. restauration CONTRÔLÉE (un fichier contenant une référence ne restaure
//!      PAS une réservation : génération et autorité sont réémises) ;
//!   5. tentative PÉRIMÉE après redémarrage refusée (autorité + génération) ;
//!   6. absence de double apprentissage (jamais de ré-publication aveugle).
//!
//! La réconciliation est CELLE du composant `this::reconciliation` (importée,
//! jamais réimplémentée) : `EnregistrementOriginal`, `reconcilier`,
//! `encoder`/`decoder`, `VerdictReconciliation`. Aucun second moteur.

#![cfg(feature = "obrain")]

use std::path::{Path, PathBuf};

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    IntegriteTentative, IssuePublication, ModePublication, Precondition, ReferenceTentative,
    SubstrateStore,
};
use this::reconciliation::{
    reconcilier, DecodageEnregistrement, EnregistrementOriginal, VerdictReconciliation,
    ENREGISTREMENT_VERSION,
};
use this_harness::attribution::{
    Attribution, Definition, Etat, Observation, Reception, Record, Resultat, RésolutionBackend,
};

fn dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "durab_obrr_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("création du répertoire de travail");
    d
}

/// Écriture ATOMIQUE : les octets sont écrits dans un fichier temporaire du
/// MÊME répertoire, puis `rename` (atomique sur POSIX). Un lecteur ne voit
/// jamais un fichier à moitié écrit — soit l'ancien, soit le nouveau, jamais un
/// entre-deux. Après l'opération, aucun fichier temporaire ne subsiste.
fn ecrire_atomique(chemin: &Path, octets: &[u8]) -> std::io::Result<()> {
    let mut nom_tmp = chemin.as_os_str().to_os_string();
    nom_tmp.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let tmp = PathBuf::from(nom_tmp);
    std::fs::write(&tmp, octets)?;
    std::fs::rename(&tmp, chemin)?;
    Ok(())
}

/// Écrit le record OBRR sur le médium durable (fichier, écriture atomique) et
/// vérifie qu'aucun fichier temporaire ne traîne (atomicité du rename).
fn ecrire_record(chemin: &Path, enr: &EnregistrementOriginal) {
    ecrire_atomique(chemin, &enr.encoder()).expect("écriture atomique du record");
    let dossier = chemin.parent().expect("dossier parent");
    let restes: Vec<_> = std::fs::read_dir(dossier)
        .expect("lecture du dossier")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(
        restes.is_empty(),
        "aucun fichier temporaire résiduel après l'écriture atomique"
    );
}

/// Relit le record depuis le médium durable et le décode — le point d'entrée de
/// la reconstruction après redémarrage.
fn relire_record(chemin: &Path) -> EnregistrementOriginal {
    let octets = std::fs::read(chemin).expect("lecture du record durable");
    EnregistrementOriginal::decoder(&octets).expect("décodage du record durable")
}

/// Le geste de l'ÉCRIVAIN : crée le store, écrit l'effet, CONSERVE le record
/// (écriture atomique, AVANT publication), publie puis ferme. Retourne le record.
fn publier_et_conserver(d: &Path, chemin_record: &Path, k: &str) -> EnregistrementOriginal {
    let store = SubstrateStore::create(&d.join("kb")).expect("création du store");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre: Precondition = store.capturer_precondition(id, &["a"]).expect("capture précondition");
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
        .expect("préparation");
    let reference = prep.reference_tentative();
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: k.to_string(),
        backend: reference.auteur(),
        generation: 1,
        cible: reference.cible(),
        effets_attendus: vec![("a".to_string(), 10)],
        reference_bytes: reference.to_bytes(),
    };
    // CONSERVATION AVANT publication (la chaîne d'autorité est constituée ici).
    ecrire_record(chemin_record, &enr);
    let verdict = store.publier_si_precondition_intacte(prep).expect("publication");
    assert_eq!(verdict, IssuePublication::PublieIntegrale, "publication intégrale");
    store.close().expect("fermeture");
    enr
}

fn def() -> Definition {
    Definition {
        identite: Some("k1".to_string()),
        prevue: Some(("a".to_string(), "m1".to_string())),
        servie: Some(("a".to_string(), "m1".to_string())),
    }
}

fn obs() -> Observation {
    Observation {
        fournisseur: "a".to_string(),
        modele: "m1".to_string(),
        resultat: Resultat::Succes,
    }
}

/// Détecte le processus ÉCRIVAIN spawné par le test interprocessus (variable
/// d'environnement, jamais un chemin fixe).
fn est_processus_ecrivain() -> bool {
    std::env::var("OBRR_MODE").is_ok_and(|v| v == "writer")
}

// ── 1. Conservation EFFECTIVE sur médium durable (écriture atomique) ─────────
#[test]
fn record_obrr_conserve_sur_fichier_atomique() {
    let d = dir("atomique");
    let chemin = d.join("record.obrr");
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: "op-42".to_string(),
        backend: 7,
        generation: 3,
        cible: NodeId(99),
        effets_attendus: vec![("a".to_string(), 10), ("b".to_string(), 20)],
        reference_bytes: vec![0xAB, 0xCD, 0xEF],
    };
    ecrire_record(&chemin, &enr);

    // Le médium est un FICHIER (durable), pas une variable RAM d'octets : on le
    // relit depuis le système de fichiers et on décode fidèlement.
    let relu = relire_record(&chemin);
    assert_eq!(relu, enr, "round-trip fidèle du record durable");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 2. Écriture interrompue → refus NOMMÉ ───────────────────────────────────
#[test]
fn ecriture_interrompue_refus_nomme() {
    let d = dir("interrompue");
    let chemin = d.join("record.obrr");
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: "op-interrompu".to_string(),
        backend: 7,
        generation: 1,
        cible: NodeId(5),
        effets_attendus: vec![("a".to_string(), 10)],
        reference_bytes: vec![1, 2, 3, 4],
    };
    let octets = enr.encoder();

    // Simulation d'une interruption à MI-ÉCRITURE : le fichier ne contient
    // qu'une fraction des octets (crash avant la fin du write / du rename).
    let moitie = octets.len() / 2;
    std::fs::write(&chemin, &octets[..moitie]).expect("écriture tronquée");
    let relu = EnregistrementOriginal::decoder(&std::fs::read(&chemin).expect("lecture"));
    assert!(
        matches!(relu, Err(DecodageEnregistrement::Malforme(_))),
        "écriture interrompue → refus NOMMÉ Malforme, pas de reconstruction silencieuse"
    );

    // Magic corrompu (premiers octets altérés) → refus nommé aussi.
    let mut corrompu = octets.clone();
    corrompu[0] = b'X';
    assert!(matches!(
        EnregistrementOriginal::decoder(&corrompu),
        Err(DecodageEnregistrement::Malforme(_))
    ));

    // Version inconnue → refus NOMMÉ (pas d'ignorance silencieuse).
    let mut version_inconnue = octets.clone();
    version_inconnue[4..8].copy_from_slice(&999u32.to_le_bytes());
    assert_eq!(
        EnregistrementOriginal::decoder(&version_inconnue),
        Err(DecodageEnregistrement::VersionInconnue(999))
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 3. Reconstruction dans un PROCESSUS INDÉPENDANT ─────────────────────────
#[test]
fn reconstruction_interprocessus_independant() {
    if est_processus_ecrivain() {
        // Processus ÉCRIVAIN (spawné par le lecteur ci-dessous) : écrit le record
        // sur le médium durable puis sort. C'est un AUTRE processus OS.
        let d = PathBuf::from(std::env::var("OBRR_DIR").expect("OBRR_DIR"));
        let chemin = d.join("record.obrr");
        publier_et_conserver(&d, &chemin, "op-interprocessus");
        return;
    }

    // Processus LECTEUR (le test courant) : prépare le répertoire, SPAWNE un
    // processus indépendant qui écrit, puis relit dans CE processus.
    let d = dir("interprocessus");
    let chemin = d.join("record.obrr");
    let exe = std::env::current_exe().expect("chemin de l'exécutable de test");
    let statut = std::process::Command::new(&exe)
        .args(["--exact", "reconstruction_interprocessus_independant"])
        .env("OBRR_MODE", "writer")
        .env("OBRR_DIR", &d)
        .status()
        .expect("spawn du processus écrivain");
    assert!(statut.success(), "le processus écrivain a réussi");

    // Le record a survécu au médium durable ET à la frontière de processus :
    // c'est CE processus (≠ écrivain) qui décode, résout et réconcilie.
    let enr = relire_record(&chemin);
    let store = SubstrateStore::open(&d.join("kb")).expect("réouverture");
    let reference = ReferenceTentative::from_bytes(&enr.reference_bytes).expect("référence");
    let res = store.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(
        reconcilier(&enr, &reference, &res, &store),
        VerdictReconciliation::Confirme,
        "le composant réutilisable confirme après reconstruction interprocessus"
    );
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 4. Restauration CONTRÔLÉE : un fichier ne restaure PAS une réservation ──
#[test]
fn restauration_controlee_pas_de_reservation_implicite() {
    let d = dir("restauration");
    let chemin = d.join("record.obrr");
    let decision_id = 7u128;

    // L'écrivain publie l'effet et conserve le record (génération 1, backend B).
    let enr = publier_et_conserver(&d, &chemin, "k1");

    // « REDÉMARRAGE » : une instance Attribution NEUVE, aucun cycle en RAM.
    let mut attr = Attribution::new();
    let autorite_apres = attr.autorite();

    // Le record ne restaure AUCUNE réservation : relire le fichier ne crée ni
    // cycle ni réservation, et `reserver` est refusé faute de cycle.
    assert!(attr.etat(decision_id).is_none(), "aucun cycle restauré depuis le fichier");
    assert!(attr.reserver(decision_id).is_none(), "pas de réservation implicite");

    // Génération et autorité doivent être RÉÉMISES explicitement.
    assert_eq!(attr.record(decision_id, def()), Record::Enregistre);
    assert_eq!(attr.recevoir(decision_id, obs()), Reception::Recu);
    let tentative = attr.reserver(decision_id).expect("réservation explicite");

    // La nouvelle réservation est une génération NEUVE portée par une autorité
    // NEUVE — pas un « rejeu » de l'enregistrement historique (qui ne porte que
    // des MÉTADONNÉES : K, backend B, génération G, cible, effets, référence).
    assert_eq!(tentative.generation(), 1, "cycle neuf : génération 1");
    assert_eq!(tentative.autorite(), autorite_apres, "autorité de l'instance courante");

    // L'autorité est réémise par instance : deux instances distinctes portent
    // des autorités distinctes (jamais restaurées depuis le disque).
    let autre = Attribution::new();
    assert_ne!(attr.autorite(), autre.autorite(), "autorité réémise, jamais restaurée");

    // Le record historique reste consultable (métadonnées), mais il ne devient
    // jamais une réservation vivante par lui-même.
    assert_eq!(enr.generation, 1, "métadonnée G conservée telle quelle");

    let _ = std::fs::remove_dir_all(&d);
}

// ── 5. Tentative PÉRIMÉE après redémarrage → refus ──────────────────────────
#[test]
fn tentative_perimee_apres_redemarrage_refusee() {
    let decision_id = 42u128;

    // « AVANT redémarrage » : une instance réserve une tentative.
    let mut avant = Attribution::new();
    avant.record(decision_id, def());
    avant.recevoir(decision_id, obs());
    let t_avant = avant.reserver(decision_id).expect("réservation avant");

    // « APRÈS redémarrage » : une instance NEUVE réserve à son tour.
    let mut apres = Attribution::new();
    apres.record(decision_id, def());
    apres.recevoir(decision_id, obs());
    let t_apres = apres.reserver(decision_id).expect("réservation après");

    // Les accesseurs lecture seule exposent autorité + génération du descripteur.
    assert_ne!(t_avant.autorite(), t_apres.autorite(), "autorités distinctes après redémarrage");
    assert_eq!(t_avant.generation(), 1);
    assert_eq!(t_apres.generation(), 1);

    // La tentative de l'ANCIENNE autorité est refusée par la nouvelle instance.
    assert!(!apres.confirmer(decision_id, t_avant), "tentative périmée refusée");
    assert!(!apres.resoudre(decision_id, t_avant, RésolutionBackend::EffetConfirme));

    // Au sein d'une MÊME instance, une génération antérieure est refusée aussi.
    let mut meme = Attribution::new();
    meme.record(decision_id, def());
    meme.recevoir(decision_id, obs());
    let g1 = meme.reserver(decision_id).expect("génération 1");
    meme.echec_certain(decision_id, g1); // relâche → Recu
    let g2 = meme.reserver(decision_id).expect("génération 2");
    assert_eq!(g1.generation(), 1);
    assert_eq!(g2.generation(), 2);
    assert!(!meme.confirmer(decision_id, g1), "génération antérieure refusée");
    assert!(meme.confirmer(decision_id, g2));
    assert_eq!(meme.etat(decision_id).expect("cycle").etat, Etat::Confirme);
}

// ── 6. Absence de double apprentissage ──────────────────────────────────────
#[test]
fn absence_de_double_apprentissage() {
    let d = dir("double");
    let chemin = d.join("record.obrr");
    let decision_id = 99u128;

    // Écrivain : publie l'effet et conserve le record.
    let enr = publier_et_conserver(&d, &chemin, "k-double");

    // « Redémarrage » : instance Attribution neuve + réouverture du store.
    let mut attr = Attribution::new();
    attr.record(decision_id, def());
    attr.recevoir(decision_id, obs());
    let tentative = attr.reserver(decision_id).expect("réservation");

    let store = SubstrateStore::open(&d.join("kb")).expect("réouverture");
    let reference = ReferenceTentative::from_bytes(&enr.reference_bytes).expect("référence");
    let res = store.resoudre_reference(&reference).expect("résolution");

    // La réconciliation est une LECTURE idempotente : même verdict, deux fois.
    let verdict = reconcilier(&enr, &reference, &res, &store);
    assert_eq!(verdict, VerdictReconciliation::Confirme);
    assert_eq!(
        verdict,
        reconcilier(&enr, &reference, &res, &store),
        "réconciliation idempotente, aucun effet de bord"
    );

    // L'effet est confirmé UNE fois via le cycle — jamais ré-publié.
    let backend = verdict.vers_backend().expect("verdict non ambigu");
    assert!(matches!(backend, RésolutionBackend::EffetConfirme));
    assert!(attr.resoudre(decision_id, tentative, backend));
    assert_eq!(attr.etat(decision_id).expect("cycle").etat, Etat::Confirme);

    // Cycle terminal : pas de ré-réservation, donc pas de ré-publication aveugle.
    assert!(attr.reserver(decision_id).is_none(), "cycle terminal, pas de ré-réservation");

    // `Indetermine` n'est JAMAIS `AbsenceDefinitive` : le verdict nuancé ne peut
    // pas déclencher une ré-publication aveugle (jamais de faux « tout est perdu »).
    assert!(matches!(
        VerdictReconciliation::Indetermine.vers_backend(),
        Some(RésolutionBackend::Indetermine)
    ));
    // Les verdicts nuancés exigent une DÉCISION EXPLICITE du consommateur
    // (`RésolutionBackend` n'a ni `PartialEq` ni `Debug` : on n'affirme que
    // l'ABSENCE de projection, jamais l'écrasement).
    assert!(
        VerdictReconciliation::Partielle.vers_backend().is_none(),
        "verdict nuancé jamais écrasé en confirmation/absence"
    );

    store.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}
