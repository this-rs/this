//! Le composant réutilisable `this::reconciliation` — importé hors tests,
//! exercé de bout en bout : encodage versionné, refus de version inconnue,
//! réconciliation contre l'enregistrement original.

#![cfg(feature = "obrain")]

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{IntegriteTentative, ModePublication, Precondition, SubstrateStore};
use this::reconciliation::{
    DecodageEnregistrement, ENREGISTREMENT_VERSION, EnregistrementOriginal, VerdictReconciliation,
    reconcilier,
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
        backend: 7,
        generation: 3,
        decision_id: 0,
        autorite_consommateur: 0,
        cible: NodeId(99),
        effets_attendus: vec![("a".to_string(), 10), ("b".to_string(), 20)],
        reference_bytes: vec![0xAB, 0xCD, 0xEF],
    };
    let octets = enr.encoder();
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
            backend: reference.auteur(),
            generation: 1,
            decision_id: 0,
            autorite_consommateur: 0,
            cible: id,
            effets_attendus: vec![("a".to_string(), 10)],
            reference_bytes: reference.to_bytes(),
        };
        let octets = enr.encoder();
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
        reconcilier(&enr, &reference, &res, &store2),
        VerdictReconciliation::Confirme,
        "le composant réutilisable confirme après réouverture"
    );
    assert!(matches!(
        reconcilier(&enr, &reference, &res, &store2).vers_backend(),
        Some(this_harness::attribution::RésolutionBackend::EffetConfirme)
    ));
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}
