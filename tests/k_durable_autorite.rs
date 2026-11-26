//! Lot A — K durable + autorité opérationnelle.
//!
//! Qualifie l'identité logique K et le chemin opérationnel de résolution :
//!   - K est DURABLE (persistée avant publication, stable après redémarrage),
//!     DISTINCTE entre opérations indépendantes, INDÉPENDANTE de la classe et du
//!     contenu, issue d'une source INDÉPENDANTE de la référence résolue
//!     (`this::autorite_k::AutoriteK`) ;
//!   - `verifier_liaison` n'est plus un appel de banc : le chemin opérationnel
//!     `this::reconciliation::resoudre_operation` la vérifie AVANT toute
//!     transition `Attribution::resoudre` ;
//!   - une tentative étrangère (même decision_id + génération) est refusée ;
//!   - aucune nouvelle opération artificielle après résultat indéterminé ;
//!   - aucun double apprentissage.
//!
//! Réutilise `this::reconciliation` et `this_harness::attribution::Attribution`
//! sans les dupliquer. Les scénarios de durabilité traversent une frontière de
//! PROCESSUS réelle (`std::process::Command`), pas un simple close/open.

#![cfg(feature = "obrain")]

use std::path::{Path, PathBuf};

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    IntegriteTentative, IssuePublication, ModePublication, Precondition, ReferenceTentative,
    SubstrateStore,
};
use this::autorite_k::AutoriteK;
use this::reconciliation::{
    ENREGISTREMENT_VERSION, EnregistrementOriginal, IssueResolutionOperation,
    VerdictReconciliation, reconcilier, resoudre_operation,
};
use this_harness::attribution::{
    Attribution, Definition, Etat, Observation, Record, Resultat, Tentative,
};

fn dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "kdurable_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("création du répertoire de travail");
    d
}

fn ecrire_atomique(chemin: &Path, octets: &[u8]) {
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
    std::fs::write(&tmp, octets).expect("écriture temporaire");
    std::fs::rename(&tmp, chemin).expect("rename atomique");
}

fn relire_record(chemin: &Path) -> EnregistrementOriginal {
    let octets = std::fs::read(chemin).expect("lecture du record durable");
    EnregistrementOriginal::decoder(&octets).expect("décodage du record durable")
}

fn lire_k(chemin: &Path) -> u128 {
    std::fs::read_to_string(chemin)
        .expect("lecture de K")
        .trim()
        .parse::<u128>()
        .expect("K u128")
}

fn definition() -> Definition {
    Definition {
        identite: Some("k1".to_string()),
        prevue: Some(("a".to_string(), "m1".to_string())),
        servie: Some(("a".to_string(), "m1".to_string())),
    }
}

fn observation() -> Observation {
    Observation {
        fournisseur: "a".to_string(),
        modele: "m1".to_string(),
        resultat: Resultat::Succes,
    }
}

fn est_worker() -> bool {
    std::env::var("K_MODE").is_ok()
}

fn mode() -> String {
    std::env::var("K_MODE").unwrap_or_default()
}

/// L'opération COMPLÈTE côté écrivain : émet un K depuis `AutoriteK`, constitue
/// un cycle `Attribution` réel (record → recevoir → reserver), prépare la
/// transformation, conserve la référence AVANT publication, publie (ou pas), et
/// ferme. Retourne l'enregistrement et le K émis.
fn effectuer_operation(
    d: &Path,
    decision_id: u128,
    publier: bool,
) -> (EnregistrementOriginal, u128) {
    let mut ak = AutoriteK::ouvrir(d.join("autorite.k")).expect("autorité K");
    let k = ak.emettre().expect("émission K");

    let mut attr = Attribution::new();
    assert_eq!(attr.record(decision_id, definition()), Record::Enregistre);
    attr.recevoir(decision_id, observation());
    let tentative = attr.reserver(decision_id).expect("réservation");

    let store = SubstrateStore::create(d.join("kb")).expect("création");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre: Precondition = store
        .capturer_precondition(id, &["a"])
        .expect("précondition");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let reference = prep.reference_tentative();
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: k,
        domaine: "k-durable-test".to_string(),
        ecrivain: reference.auteur(),
        generation: tentative.generation(),
        decision_id,
        autorite_consommateur: tentative.autorite(),
        cible: reference.cible(),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: reference.to_bytes(),
    };
    if publier {
        let verdict = store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        assert_eq!(
            verdict,
            IssuePublication::PublieIntegrale,
            "publication intégrale"
        );
    } else {
        drop(prep); // jamais publiée → résolution Indeterminee
    }
    store.close().expect("fermeture");
    (enr, k)
}

/// Reconstitution CONTRÔLÉE après redémarrage : K et référence sont PRÉSERVÉS,
/// génération et autorité consommateur sont RÉÉMISES depuis une réservation
/// NEUVE (le fichier ne restaure jamais une réservation).
fn reconstituer(
    attr: &mut Attribution,
    decision_id: u128,
    enr: &EnregistrementOriginal,
) -> (Tentative, EnregistrementOriginal) {
    assert_eq!(attr.record(decision_id, definition()), Record::Enregistre);
    attr.recevoir(decision_id, observation());
    let tentative = attr
        .reserver(decision_id)
        .expect("réservation après redémarrage");
    let reconstitue = EnregistrementOriginal {
        version: enr.version,
        k: enr.k, // SAME K — l'identité logique de l'opération est stable
        domaine: enr.domaine.clone(),
        ecrivain: enr.ecrivain,
        generation: tentative.generation(), // ré-émise
        decision_id,
        autorite_consommateur: tentative.autorite(), // ré-émise
        cible: enr.cible,
        effets_attendus: enr.effets_attendus.clone(),
        reference_bytes: enr.reference_bytes.clone(), // SAME référence
    };
    (tentative, reconstitue)
}

/// Spawne le processus WORKER (le test courant, rejoué avec `--exact` + env).
fn spawner(exe: &Path, test: &str, d: &Path, mode_worker: &str, envs: &[(&str, &str)]) {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--exact", test])
        .env("K_MODE", mode_worker)
        .env("K_DIR", d);
    for (cle, val) in envs {
        cmd.env(cle, val);
    }
    let statut = cmd.status().expect("spawn du worker");
    assert!(
        statut.success(),
        "worker {mode_worker} du test {test} a échoué"
    );
}

/// Dispatcheur du rôle worker — renvoie `true` si ce processus ÉTAIT le worker.
fn executer_worker() -> bool {
    if !est_worker() {
        return false;
    }
    let d = PathBuf::from(std::env::var("K_DIR").expect("K_DIR"));
    match mode().as_str() {
        "operation_publiee" | "operation_indeterminee" => {
            let decision_id = std::env::var("K_DECISION")
                .expect("K_DECISION")
                .parse::<u128>()
                .expect("K_DECISION u128");
            let publier = mode() == "operation_publiee";
            let (enr, k) = effectuer_operation(&d, decision_id, publier);
            ecrire_atomique(&d.join("record.obrr"), &enr.encoder().unwrap());
            std::fs::write(d.join("k.txt"), k.to_string()).expect("écriture k.txt");
        }
        "emettre" => {
            let sortie = std::env::var("K_SORTIE").expect("K_SORTIE");
            let mut ak = AutoriteK::ouvrir(d.join("autorite.k")).expect("autorité K");
            let k = ak.emettre().expect("émission K");
            std::fs::write(d.join(sortie), k.to_string()).expect("écriture du K émis");
        }
        autre => panic!("mode worker inconnu : {autre}"),
    }
    true
}

// ── 1. Même opération après redémarrage : K STABLE, génération/autorité réémises ──
#[test]
fn scenario_1_meme_operation_apres_redemarrage() {
    if executer_worker() {
        return;
    }
    let d = dir("s1_redemarrage");
    let decision_id = 100u128;
    let exe = std::env::current_exe().expect("exe de test");
    spawner(
        &exe,
        "scenario_1_meme_operation_apres_redemarrage",
        &d,
        "operation_publiee",
        &[("K_DECISION", "100")],
    );

    // Processus B (≠ écrivain) : K a survécu à la frontière de processus.
    let enr = relire_record(&d.join("record.obrr"));
    let k_original = lire_k(&d.join("k.txt"));
    assert_eq!(enr.k, k_original, "K stable entre les processus");
    assert_ne!(enr.k, 0, "K non nul");

    // Reconstitution contrôlée : autorité + génération réémises, K + référence préservés.
    let mut attr = Attribution::new();
    let (tentative, reconstitue) = reconstituer(&mut attr, decision_id, &enr);

    let store = SubstrateStore::open(d.join("kb")).expect("réouverture");
    let reference =
        ReferenceTentative::from_bytes(&reconstitue.reference_bytes).expect("référence");
    let res = store.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(
        resoudre_operation(
            &mut attr,
            decision_id,
            tentative,
            &reconstitue,
            &reference,
            &res
        ),
        IssueResolutionOperation::Resolu(true),
        "chemin opérationnel : liaison + réconciliation + résolution"
    );
    assert_eq!(attr.etat(decision_id).expect("cycle").etat, Etat::Confirme);
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 2. Nouvelle tentative après refus certain : même K, génération incrémentée ──
#[test]
fn scenario_2_nouvelle_tentative_apres_refus_certain() {
    let d = dir("s2_refus_certain");
    let decision_id = 200u128;

    let mut ak = AutoriteK::ouvrir(d.join("autorite.k")).expect("autorité K");
    let k = ak.emettre().expect("K de l'opération");

    let mut attr = Attribution::new();
    attr.record(decision_id, definition());
    attr.recevoir(decision_id, observation());

    // Tentative 1 : refus CERTAIN (aucun effet engagé).
    let t1 = attr.reserver(decision_id).expect("réservation 1");
    assert_eq!(t1.generation(), 1);
    assert!(
        attr.echec_certain(decision_id, t1),
        "refus certain → relâche"
    );

    // Tentative 2 : la MÊME opération (K réutilisé, jamais ré-émis), génération 2.
    let t2 = attr.reserver(decision_id).expect("réservation 2");
    assert_eq!(t2.generation(), 2, "génération incrémentée");
    assert_eq!(
        ak.prochain(),
        2,
        "aucun nouveau K émis : la même opération réutilise K (prochain inchangé)"
    );

    // Le K est le même d'une tentative à l'autre, la génération distingue les tentatives.
    let enr1 = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: k,
        domaine: "k-durable-test".to_string(),
        ecrivain: 7,
        generation: t1.generation(),
        decision_id,
        autorite_consommateur: t1.autorite(),
        cible: obrain_common::types::NodeId(1),
        effets_attendus: vec![],
        reference_bytes: vec![],
    };
    let enr2 = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: k,
        domaine: "k-durable-test".to_string(),
        ecrivain: 7,
        generation: t2.generation(),
        decision_id,
        autorite_consommateur: t2.autorite(),
        cible: obrain_common::types::NodeId(1),
        effets_attendus: vec![],
        reference_bytes: vec![],
    };
    assert_eq!(
        enr1.k, enr2.k,
        "même K entre deux tentatives de la même opération"
    );
    assert_ne!(enr1.generation, enr2.generation, "générations distinctes");

    // L'ANCIENNE tentative (génération 1) ne satisfait plus la liaison de la nouvelle.
    assert!(
        enr2.verifier_liaison(&t2),
        "liaison valide pour la génération 2"
    );
    assert!(
        !enr2.verifier_liaison(&t1),
        "l'ancienne génération ne lie plus"
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 3. Ancienne tentative indéterminée : pas de nouvelle opération artificielle ──
#[test]
fn scenario_3_ancienne_tentative_indeterminee() {
    if executer_worker() {
        return;
    }
    let d = dir("s3_indeterminee");
    let decision_id = 300u128;
    let exe = std::env::current_exe().expect("exe de test");
    spawner(
        &exe,
        "scenario_3_ancienne_tentative_indeterminee",
        &d,
        "operation_indeterminee",
        &[("K_DECISION", "300")],
    );

    let enr = relire_record(&d.join("record.obrr"));
    let k_original = lire_k(&d.join("k.txt"));
    assert_eq!(
        enr.k, k_original,
        "K inchangé après la tentative indéterminée"
    );

    // Réémission contrôlée (autorité + génération neuves), K préservé.
    let mut attr = Attribution::new();
    let (tentative, reconstitue) = reconstituer(&mut attr, decision_id, &enr);

    let store = SubstrateStore::open(d.join("kb")).expect("réouverture");
    let reference =
        ReferenceTentative::from_bytes(&reconstitue.reference_bytes).expect("référence");
    let res = store.resoudre_reference(&reference).expect("résolution");
    assert_eq!(
        res.integrite,
        IntegriteTentative::Indeterminee,
        "jamais publiée"
    );

    // La réconciliation NE transforme PAS l'indétermination en absence définitive.
    assert_eq!(
        reconcilier(&reconstitue, &reference, &res),
        VerdictReconciliation::Indetermine,
        "Indeterminee n'est jamais AbsenceDefinitive"
    );
    // Le chemin opérationnel résout en Indetermine : PAS de confirmation, PAS de
    // nouvelle réservation (donc aucune opération artificielle, aucun re-publication).
    assert_eq!(
        resoudre_operation(
            &mut attr,
            decision_id,
            tentative,
            &reconstitue,
            &reference,
            &res
        ),
        IssueResolutionOperation::Resolu(true),
        "résolution Indetermine acceptée sans transition terminale"
    );
    assert_ne!(
        attr.etat(decision_id).expect("cycle").etat,
        Etat::Confirme,
        "un résultat indéterminé ne confirme rien"
    );
    assert!(
        attr.reserver(decision_id).is_none(),
        "pas de ré-réservation après un résultat indéterminé"
    );

    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 4. Deux opérations indépendantes de contenu identique : K distincts ────────
#[test]
fn scenario_4_deux_operations_independantes_contenu_identique() {
    if executer_worker() {
        return;
    }
    let d = dir("s4_contenu_identique");
    let exe = std::env::current_exe().expect("exe de test");
    // Deux PROCESSUS indépendants, sur la MÊME autorité K : chacun émet un K.
    // Le contenu est identique par construction : `emettre` n'accepte AUCUN
    // contenu — K est indépendant de la classe et du contenu.
    spawner(
        &exe,
        "scenario_4_deux_operations_independantes_contenu_identique",
        &d,
        "emettre",
        &[("K_SORTIE", "k1.txt")],
    );
    spawner(
        &exe,
        "scenario_4_deux_operations_independantes_contenu_identique",
        &d,
        "emettre",
        &[("K_SORTIE", "k2.txt")],
    );
    let k1 = lire_k(&d.join("k1.txt"));
    let k2 = lire_k(&d.join("k2.txt"));
    assert_ne!(k1, k2, "deux opérations indépendantes → K distincts");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 5. Deux opérations d'une même classe : K distincts ─────────────────────────
#[test]
fn scenario_5_deux_operations_meme_classe() {
    let d = dir("s5_meme_classe");
    let mut ak = AutoriteK::ouvrir(d.join("autorite.k")).expect("autorité K");
    // Même classe : la `Definition` est identique pour les deux opérations.
    let k1 = ak.emettre().expect("K 1");
    let k2 = ak.emettre().expect("K 2");
    assert_ne!(k1, k2, "même classe, deux opérations → K distincts");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 6. Collision de decision_id : K distingue, la liaison lie l'autorité ──────
#[test]
fn scenario_6_collision_decision_id() {
    let d = dir("s6_collision");
    let decision_id = 600u128; // le MÊME decision_id pour deux opérations distinctes

    let mut ak = AutoriteK::ouvrir(d.join("autorite.k")).expect("autorité K");
    let k1 = ak.emettre().expect("K op 1");
    let k2 = ak.emettre().expect("K op 2");
    assert_ne!(k1, k2, "collision de decision_id, K distincts");

    // Deux instances d'Attribution (deux autorités) sur le même decision_id.
    let mut a = Attribution::new();
    let mut b = Attribution::new();
    a.record(decision_id, definition());
    b.record(decision_id, definition());
    a.recevoir(decision_id, observation());
    b.recevoir(decision_id, observation());
    let ta = a.reserver(decision_id).expect("réservation A");
    let tb = b.reserver(decision_id).expect("réservation B");
    assert_eq!(ta.decision_id(), tb.decision_id(), "même decision_id");
    assert_ne!(ta.autorite(), tb.autorite(), "autorités distinctes");

    let enr_a = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: k1,
        domaine: "k-durable-test".to_string(),
        ecrivain: 7,
        generation: ta.generation(),
        decision_id,
        autorite_consommateur: ta.autorite(),
        cible: obrain_common::types::NodeId(1),
        effets_attendus: vec![],
        reference_bytes: vec![],
    };
    let enr_b = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: k2,
        domaine: "k-durable-test".to_string(),
        ecrivain: 7,
        generation: tb.generation(),
        decision_id,
        autorite_consommateur: tb.autorite(),
        cible: obrain_common::types::NodeId(1),
        effets_attendus: vec![],
        reference_bytes: vec![],
    };
    // Chaque enregistrement ne lie QUE sa propre autorité (collision résolue par K + autorité).
    assert!(enr_a.verifier_liaison(&ta));
    assert!(enr_b.verifier_liaison(&tb));
    assert!(
        !enr_a.verifier_liaison(&tb),
        "collision : l'autorité B ne lie pas A"
    );
    assert!(
        !enr_b.verifier_liaison(&ta),
        "collision : l'autorité A ne lie pas B"
    );

    let _ = std::fs::remove_dir_all(&d);
}

// ── 7. Tentative étrangère (même decision_id + génération) : refusée ───────────
#[test]
fn scenario_7_tentative_etrangere_refusee() {
    let d = dir("s7_etrangere");
    let decision_id = 700u128;

    let mut a = Attribution::new();
    a.record(decision_id, definition());
    a.recevoir(decision_id, observation());
    let ta = a.reserver(decision_id).expect("réservation légitime");

    let mut ak = AutoriteK::ouvrir(d.join("autorite.k")).expect("autorité K");
    let k_legitime = ak.emettre().expect("K légitime");

    let store = SubstrateStore::create(d.join("kb")).expect("création");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre: Precondition = store.capturer_precondition(id, &["a"]).expect("pre");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("prep");
    let reference = prep.reference_tentative();
    store
        .publier_si_precondition_intacte(prep)
        .expect("publication");
    let res = store.resoudre_reference(&reference).expect("résolution");

    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: k_legitime,
        domaine: "k-durable-test".to_string(),
        ecrivain: reference.auteur(),
        generation: ta.generation(),
        decision_id,
        autorite_consommateur: ta.autorite(),
        cible: reference.cible(),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: reference.to_bytes(),
    };

    // Tentative ÉTRANGÈRE : autorité différente, mais decision_id + génération coïncident.
    let mut b = Attribution::new();
    b.record(decision_id, definition());
    b.recevoir(decision_id, observation());
    let tb = b.reserver(decision_id).expect("réservation étrangère");
    assert_eq!(tb.decision_id(), ta.decision_id(), "decision_id coïncide");
    assert_eq!(tb.generation(), ta.generation(), "génération coïncide");
    assert_ne!(tb.autorite(), ta.autorite(), "mais autorité DIFFÉRENTE");

    // Le chemin opérationnel refuse la tentative étrangère AVANT toute résolution.
    assert_eq!(
        resoudre_operation(&mut a, decision_id, tb, &enr, &reference, &res),
        IssueResolutionOperation::LiaisonRefusee,
        "tentative étrangère refusée malgré decision_id + génération coïncidents"
    );
    assert_eq!(
        a.etat(decision_id).expect("cycle").etat,
        Etat::EnCours,
        "le cycle légitime n'est pas résolu par la tentative étrangère"
    );

    // Contrôle POSITIF : la tentative LÉGITIME résout normalement.
    assert_eq!(
        resoudre_operation(&mut a, decision_id, ta, &enr, &reference, &res),
        IssueResolutionOperation::Resolu(true)
    );
    assert_eq!(a.etat(decision_id).expect("cycle").etat, Etat::Confirme);

    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 8. Corruption valide de K / génération : détectée par la liaison ───────────
#[test]
fn scenario_8_corruption_valide_k_generation() {
    let d = dir("s8_corruption");
    let decision_id = 800u128;

    let mut attr = Attribution::new();
    attr.record(decision_id, definition());
    attr.recevoir(decision_id, observation());
    let tentative = attr.reserver(decision_id).expect("réservation");

    let mut ak = AutoriteK::ouvrir(d.join("autorite.k")).expect("autorité K");
    let k = ak.emettre().expect("K");

    let store = SubstrateStore::create(d.join("kb")).expect("création");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre: Precondition = store.capturer_precondition(id, &["a"]).expect("pre");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("prep");
    let reference = prep.reference_tentative();
    store
        .publier_si_precondition_intacte(prep)
        .expect("publication");
    let res = store.resoudre_reference(&reference).expect("résolution");

    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k,
        domaine: "k-durable-test".to_string(),
        ecrivain: reference.auteur(),
        generation: tentative.generation(),
        decision_id,
        autorite_consommateur: tentative.autorite(),
        cible: reference.cible(),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: reference.to_bytes(),
    };

    // 8a — K NUL (corruption syntaxiquement valide de K : jamais émis).
    let mut enr_k_vide = enr.clone();
    enr_k_vide.k = 0;
    assert!(!enr_k_vide.verifier_liaison(&tentative), "K nul détecté");
    assert_eq!(
        resoudre_operation(
            &mut attr,
            decision_id,
            tentative,
            &enr_k_vide,
            &reference,
            &res
        ),
        IssueResolutionOperation::LiaisonRefusee,
        "corruption valide de K → liaison refusée"
    );

    // 8b — GÉNÉRATION corrompue (u64 valide, mais ≠ réservation).
    let mut enr_gen = enr.clone();
    enr_gen.generation = 99;
    assert!(
        !enr_gen.verifier_liaison(&tentative),
        "génération corrompue détectée"
    );
    assert_eq!(
        resoudre_operation(
            &mut attr,
            decision_id,
            tentative,
            &enr_gen,
            &reference,
            &res
        ),
        IssueResolutionOperation::LiaisonRefusee,
        "corruption valide de génération → liaison refusée"
    );

    // Le cycle n'a pas bougé (aucune transition sur liaison refusée).
    assert_eq!(attr.etat(decision_id).expect("cycle").etat, Etat::EnCours);

    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}
