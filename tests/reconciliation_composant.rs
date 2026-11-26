//! Le composant réutilisable `this::reconciliation` — importé hors tests,
//! exercé de bout en bout : encodage versionné, refus de version inconnue,
//! réconciliation contre l'enregistrement original, préservation des types
//! natifs (Int64 / Float64 / Bytes), refus de la conversion arbitraire, refus
//! d'une résolution substituée, et domaine de stockage HORS-BANDE (collision
//! HLC, obrain 62ecd940).

#![cfg(feature = "obrain")]

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    EffetResolu, EstampilleComplete, EtatPropriete, IntegriteTentative, ModePublication,
    Precondition, ResolutionEffet, ResolutionTentative, SubstrateStore,
};
use this::reconciliation::{
    DecodageEnregistrement, ENREGISTREMENT_VERSION, EncodageEnregistrement, EnregistrementOriginal,
    VerdictReconciliation, reconcilier,
};

fn dir(tag: &str) -> std::path::PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "reconc_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    d
}

#[test]
fn composant_roundtrip_et_refus_version_inconnue() {
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 42,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 3,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![
            ("a".to_string(), Value::Int64(10)),
            ("b".to_string(), Value::Int64(20)),
        ],
        reference_bytes: vec![0xAB, 0xCD, 0xEF],
    };
    let octets = enr.encoder().expect("encodage");
    let relu = EnregistrementOriginal::decoder(&octets).expect("décodage");
    assert_eq!(relu, enr, "round-trip fidèle du bloc cohérent unique");

    // Version inconnue → refus NOMÉ, pas d'ignorance silencieuse.
    let mut corrompu = octets.clone();
    // version est à l'offset 4 (après magic 4 octets), u32 LE.
    corrompu[4..8].copy_from_slice(&999u32.to_le_bytes());
    assert_eq!(
        EnregistrementOriginal::decoder(&corrompu),
        Err(DecodageEnregistrement::VersionInconnue(999)),
        "version inconnue refusée"
    );

    // Magic invalide → malformé.
    let mut mauvais_magic = octets.clone();
    mauvais_magic[0] = b'X';
    assert!(matches!(
        EnregistrementOriginal::decoder(&mauvais_magic),
        Err(DecodageEnregistrement::Malforme(_))
    ));
}

#[test]
fn composant_confirme_apres_reouverture() {
    let d = dir("nominal");
    let chemin = d.join("kb");
    let (enr_octets, _id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre: Precondition = store.capturer_precondition(id, &["a"]).expect("capture");
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
            k: 1,
            domaine: "base-test".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        let _verdict = store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (octets, id)
    };

    // Réouverture : décoder l'enregistrement, résoudre, réconcilier via le COMPOSANT.
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&enr.reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Confirme,
        "le composant réutilisable confirme après réouverture"
    );
    assert!(matches!(
        reconcilier(&enr, &reference, &res).vers_backend(),
        Some(this_harness::attribution::RésolutionBackend::EffetConfirme)
    ));
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Le format v2 (valeurs d'effet `i64` nues) est refusé nommément : le bump de
/// version 2→5 est assumé, un enregistrement v2 n'est jamais réinterprété.
#[test]
fn composant_version_v2_refusee() {
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 7,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 1,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: vec![0xAB],
    };
    let octets = enr.encoder().expect("encodage");
    let mut v2 = octets.clone();
    v2[4..8].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        EnregistrementOriginal::decoder(&v2),
        Err(DecodageEnregistrement::VersionInconnue(2)),
        "un enregistrement v2 est refusé, jamais réinterprété en v5"
    );
}

/// Int64, Float64 et Bytes survivent au round-trip, SANS conversion arbitraire.
#[test]
fn composant_valeurs_natives_roundtrip() {
    let payload: std::sync::Arc<[u8]> = (10..42u8).collect::<Vec<u8>>().into();
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 11,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 1,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![
            ("i".to_string(), Value::Int64(-42)),
            ("f".to_string(), Value::Float64(3.25)),
            ("b".to_string(), Value::Bytes(payload)),
        ],
        reference_bytes: vec![0xAA],
    };
    let octets = enr.encoder().expect("encodage");
    let relu = EnregistrementOriginal::decoder(&octets).expect("décodage");
    assert_eq!(relu, enr, "Int64 / Float64 / Bytes survivent au round-trip");
}

/// Un type non représentable est REFUSÉ à l'encodage, jamais converti en Int64.
#[test]
fn composant_refuse_conversion_arbitraire() {
    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k: 13,
        domaine: "base-test".to_string(),
        ecrivain: 7,
        generation: 1,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![("b".to_string(), Value::Bool(true))],
        reference_bytes: vec![],
    };
    assert!(
        matches!(
            enr.encoder(),
            Err(EncodageEnregistrement::ValeurNonRepresentable(nom)) if nom == "Bool"
        ),
        "Bool refusé nommément, jamais réduit à Int64"
    );
}

/// La valeur canonique courante porte Int64 / Float64 / Bytes TELLE QUELLE —
/// la réconciliation la consomme via `etat_courant` (EtatPropriete), sans
/// relecture indépendante ni conversion.
#[test]
fn composant_valeurs_natives_reconciliation() {
    let d = dir("types");
    let chemin = d.join("kb");
    let payload_attendu: std::sync::Arc<[u8]> = (0..32u8).collect::<Vec<u8>>().into();
    let score_attendu = Value::Float64(0.75);

    let (enr_octets, _id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "payload", Value::Bytes(vec![0u8; 32].into()));
        store.set_node_property(id, "score", Value::Float64(0.25));
        let pre = store
            .capturer_precondition(id, &["payload", "score"])
            .expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[
                    ("payload", Value::Bytes(payload_attendu.clone())),
                    ("score", score_attendu.clone()),
                ],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let reference = prep.reference_tentative();
        let enr = EnregistrementOriginal {
            version: ENREGISTREMENT_VERSION,
            k: 17,
            domaine: "base-test".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![
                ("payload".to_string(), Value::Bytes(payload_attendu.clone())),
                ("score".to_string(), score_attendu.clone()),
            ],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (octets, id)
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    assert_eq!(
        enr.effets_attendus[0].1,
        Value::Bytes(payload_attendu.clone())
    );
    assert_eq!(enr.effets_attendus[1].1, Value::Float64(0.75));
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&enr.reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    // L'observation canonique porte les valeurs telles quelles, sans conversion.
    assert!(matches!(
        &res.par_cle.iter().find(|e| e.cle == "payload").unwrap().etat_courant,
        EtatPropriete::Presente { valeur, .. } if valeur == &Value::Bytes(payload_attendu.clone())
    ));
    assert!(matches!(
        &res.par_cle.iter().find(|e| e.cle == "score").unwrap().etat_courant,
        EtatPropriete::Presente { valeur, .. } if valeur == &Value::Float64(0.75)
    ));
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Confirme,
        "Int64/Float64/Bytes préservés par l'observation canonique"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Une résolution SUBSTITUÉE (clés différentes de la référence) est refusée :
/// elle ne doit pas produire sa propre preuve.
#[test]
fn composant_resolution_substituee_refusee() {
    let d = dir("subst_res");
    let chemin = d.join("kb");
    let (enr, reference) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
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
            k: 19,
            domaine: "base-test".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        (enr, reference)
    };

    // Résolution FABRIQUÉE : cible/auteur corrects, mais clé "b" au lieu de "a".
    let res = ResolutionTentative {
        cible: reference.cible(),
        auteur_resolvant: reference.auteur(),
        par_cle: vec![EffetResolu {
            cle: "b".to_string(),
            etat: ResolutionEffet::Engage,
            etat_courant: EtatPropriete::Presente {
                estampille: EstampilleComplete {
                    auteur: reference.auteur(),
                    horloge: reference.horloge(),
                },
                valeur: Value::Int64(10),
            },
            estampille_courante: Some((reference.auteur(), reference.horloge())),
            valeur_courante: Some(Value::Int64(10)),
        }],
        integrite: IntegriteTentative::Integrale,
    };
    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Substitution,
        "une résolution substituée (clé différente) est refusée"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Collision HLC (obrain 62ecd940) : `writer.id` ne discrimine PAS les bases —
/// deux bases indépendantes partageant `writer.id` produisent des références
/// BYTE-IDENTIQUES. `reconcilier` ne vérifie donc PAS le domaine de stockage
/// (`domaine`), que la référence ne porte pas : deux enregistrements de même
/// écrivain/cible/référence mais de domaines DIFFÉRENTS réconcilient à
/// l'IDENTIQUE. La preuve du domaine est HORS-BANDE (responsabilité du
/// consommateur), jamais celle de la référence.
#[test]
fn composant_domaine_est_hors_bande() {
    let d = dir("domaine_hb");
    let chemin = d.join("kb");
    let (enr_octets, _id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
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
            k: 23,
            domaine: "base-a".to_string(),
            ecrivain: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), Value::Int64(10))],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder().expect("encodage");
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (octets, id)
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let enr = EnregistrementOriginal::decoder(&enr_octets).expect("décodage");
    let reference =
        obrain_substrate::ReferenceTentative::from_bytes(&enr.reference_bytes).expect("réf");
    let res = store2.resoudre_reference(&reference).expect("résolution");

    // Même écrivain/cible/référence/effets, domaine DIFFÉRENT.
    let mut enr_b = enr.clone();
    enr_b.domaine = "base-b".to_string();

    assert_eq!(
        reconcilier(&enr, &reference, &res),
        VerdictReconciliation::Confirme
    );
    assert_eq!(
        reconcilier(&enr_b, &reference, &res),
        reconcilier(&enr, &reference, &res),
        "domaine différent → verdict IDENTIQUE : la preuve du domaine est hors-bande"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}
