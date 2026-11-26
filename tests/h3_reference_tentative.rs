//! H3 native — résolution historique d'un effet par référence de tentative.
//!
//! Contract reproduit depuis obrain@8918879e (`contrat_public_resolution_apres_perte_reponse`),
//! consommé ici EXTERNE au crate substrate, via la dépendance DIRECTE `obrain-substrate`
//! (pin 8918879e4e7d5efb05befa52d4003cf4bad5b0e2 sur la branche feat/334-contrats).
//!
//! Bornes mesurées (ne pas contourner) :
//!  - `ResolutionEffet::Engage` atteste l'ENGAGEMENT HISTORIQUE (présence dans la
//!    chaîne canonique accessible), PAS un effet canonique courant : un effet masqué
//!    par une écriture ultérieure reste `Engage`.
//!  - La référence V1 est l'identité PHYSIQUE de tentative `(auteur=writer_id,
//!    horloge=HLC)` : elle n'atteste AUCUNE intégrité inter-base, et n'est PAS une
//!    clé d'idempotence durable (pas d'équivalent de l'identité logique K).
//!  - Le chemin DIFFÉRÉ (`preparer_transformation_differee`) attribue l'estampille à
//!    la matérialisation : la référence n'est pas disponible AVANT publication, donc
//!    une perte de réponse sur ce chemin reste NON résoluble.
//!  - La durabilité après arrêt brutal (kill -9 entre publication et flush) reste à
//!    qualifier : la publication n'est pas un acquittement de durabilité.

#![cfg(feature = "obrain")]


use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::{
    IssuePublication, ModePublication, Precondition, ReferenceTentative, ResolutionEffet,
    SubstrateStore,
};

/// Répertoire jetable unique (pas de dépendance `tempfile` dans ce crate).
fn dir(tag: &str) -> std::path::PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "h3_ref_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    d
}

fn i64_de(v: &Value) -> i64 {
    match v {
        Value::Int64(n) => *n,
        autre => panic!("attendu Int64, lu {autre:?}"),
    }
}

/// Contrat noyau : référence conservée AVANT publication → publication réelle
/// (réponse perdue) → réouverture → résolution Engage → masquage → toujours Engage.
#[test]
fn h3_reference_tentative_resout_apres_perte_reponse() {
    let d = dir("perte");
    let chemin = d.join("kb");
    let (id, reference) = {
        let store = SubstrateStore::create(&chemin).expect("création store");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre: Precondition = store
            .capturer_precondition(id, &["a"])
            .expect("capture précondition");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation transformation");
        // La référence est conservée AVANT la publication.
        let reference: ReferenceTentative = prep.reference_tentative();
        // Publication réelle, verdict « perdu » (non lu par le consommateur).
        let _verdict = store
            .publier_si_precondition_intacte(prep)
            .expect("publication");
        store.close().expect("fermeture");
        (id, reference)
    };

    // Réouverture : résoudre SANS republier.
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let res = store2
        .resoudre_effet_historique_par_reference(id, "a", reference)
        .expect("résolution");
    assert_eq!(
        res,
        ResolutionEffet::Engage,
        "l'engagement est attesté après réouverture, sans republier"
    );

    // Masquage ultérieur : l'événement historique reste attesté.
    store2.set_node_property(id, "a", Value::Int64(99));
    assert_eq!(
        store2
            .resoudre_effet_historique_par_reference(id, "a", reference)
            .expect("résolution après masquage"),
        ResolutionEffet::Engage,
        "masqué mais historiquement engagé"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// `Engage` ≠ effet canonique courant : après masquage, la valeur LUE est celle du
/// masque (99), tandis que la résolution historique atteste encore `Engage`.
/// La résolution répond à « cet effet a-t-il été engagé ? », pas « est-il visible ? ».
#[test]
fn h3_engage_n_est_pas_effet_confirme() {
    let d = dir("engage");
    let chemin = d.join("kb");
    let (id, reference) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .expect("préparation");
        let reference = prep.reference_tentative();
        assert_eq!(
            store.publier_si_precondition_intacte(prep).expect("publication"),
            IssuePublication::PublieIntegrale
        );
        store.close().expect("fermeture");
        (id, reference)
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    // Un écrivain ultérieur masque l'effet : la valeur courante devient 99.
    store2.set_node_property(id, "a", Value::Int64(99));
    let courante = store2
        .get_node_property(id, &PropertyKey::new("a"))
        .expect("lecture courante");
    assert_eq!(
        i64_de(&courante),
        99,
        "la valeur courante est le masque, pas l'effet originel"
    );
    // … mais l'engagement de la tentative est TOUJOURS attesté.
    assert_eq!(
        store2
            .resoudre_effet_historique_par_reference(id, "a", reference)
            .expect("résolution"),
        ResolutionEffet::Engage,
        "Engage atteste l'historique, pas la canonicité courante"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Une tentative PRÉPARÉE mais jamais PUBLIÉE n'est pas atteignable par la chaîne
/// canonique : la résolution rend `Indetermine`, pas `Engage`.
#[test]
fn h3_preparation_non_publiee_est_indeterminee() {
    let d = dir("nonpub");
    let chemin = d.join("kb");
    let (id, reference) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .expect("préparation");
        let reference = prep.reference_tentative();
        // prep est ABANDONNÉE (jamais publiée) — pages privées orphelines.
        drop(prep);
        store.close().expect("fermeture");
        (id, reference)
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    assert_eq!(
        store2
            .resoudre_effet_historique_par_reference(id, "a", reference)
            .expect("résolution"),
        ResolutionEffet::Indetermine,
        "une préparation non publiée n'entre jamais dans la chaîne canonique"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// La référence V1 est l'identité PHYSIQUE de tentative, propre à UNE base : elle
/// n'atteste aucune intégrité inter-base. Résolue contre une AUTRE base, elle rend
/// `Indetermine` (l'effet n'y a jamais été engagé), sans erreur ni faux positif.
#[test]
fn h3_reference_v1_ne_traverse_pas_les_bases() {
    let d = dir("interbase");
    let chemin_a = d.join("kb_a");
    let chemin_b = d.join("kb_b");

    // Base A : un effet engagé, référence capturée.
    let (id, reference) = {
        let store = SubstrateStore::create(&chemin_a).expect("création A");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture A");
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .expect("préparation A");
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).expect("publication A");
        store.close().expect("fermeture A");
        (id, reference)
    };

    // Base B : un nœud de même id, mais AUCUN effet engagé par cette référence.
    let store_b = SubstrateStore::create(&chemin_b).expect("création B");
    let id_b = store_b.create_node(&["ressource"]);
    assert_eq!(id, id_b, "même NodeId de part et d'autre (déterministe)");
    store_b.set_node_property(id_b, "a", Value::Int64(1));

    // La référence issue de A, résolue contre B, n'atteste RIEN : Indetermine.
    assert_eq!(
        store_b
            .resoudre_effet_historique_par_reference(id_b, "a", reference)
            .expect("résolution B"),
        ResolutionEffet::Indetermine,
        "la référence V1 est physique (auteur, horloge) : aucune intégrité inter-base"
    );
    store_b.close().expect("fermeture B");
    let _ = std::fs::remove_dir_all(&d);
}

/// Garde-fou de non-régression : la référence V1 n'est PAS une clé d'idempotence.
/// Deux tentatives DISTINCTES (même contenu) portent des références distinctes ;
/// résoudre la seconde AVANT sa publication ne peut pas « hériter » de la première.
#[test]
fn h3_deux_tentatives_portent_des_references_distinctes() {
    let d = dir("deuxref");
    let chemin = d.join("kb");
    let store = SubstrateStore::create(&chemin).expect("création");
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).expect("capture");
    let prep1 = store
        .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
        .expect("préparation 1");
    let r1 = prep1.reference_tentative();
    store.publier_si_precondition_intacte(prep1).expect("publication 1");

    // Seconde tentative : même contenu, mais référence DISTINCTE (HLC avance).
    let pre2 = store.capturer_precondition(id, &["a"]).expect("capture 2");
    let prep2 = store
        .preparer_transformation(id, &[("a", Value::Int64(20))], &pre2, ModePublication::Strict)
        .expect("préparation 2");
    let r2 = prep2.reference_tentative();
    assert_ne!(
        (r1.auteur, r1.horloge),
        (r2.auteur, r2.horloge),
        "deux tentatives = deux références physiques distinctes (pas d'idempotence par contenu)"
    );
    store.publier_si_precondition_intacte(prep2).expect("publication 2");
    store.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}
