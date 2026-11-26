//! H3 V2 — le cycle harness RÉEL sur obrain 89d169d7 (feat/334-contrats-v2).
//!
//! Consomme l'API publique consolidée :
//!   - `TransformationPrete::reference_tentative()` ;
//!   - `ReferenceTentative::to_bytes/from_bytes` (codec versionné, magic « OBRT ») ;
//!   - `SubstrateStore::resoudre_reference(&ReferenceTentative)`.
//!
//! Cycle : décision → préparation → conservation de référence → publication →
//! perte de réponse → réouverture → résolution → réconciliation.
//!
//! Bornes NON contournées (mesurées, à préserver) :
//!   - `IntegriteTentative::Integrale` (tous les effets engagés) n'est PAS
//!     `EffetConfirme` : un effet engagé puis masqué reste `Integrale`. La
//!     réconciliation compare l'estampille courante à la référence.
//!   - `ResolutionEffet::Indetermine` (et `IntegriteTentative::Indeterminee`) ne
//!     signifie JAMAIS absence définitive : « jamais engagé » et « compacté »
//!     sont indistinguables dans l'histoire accessible.
//!   - `ReferenceTentative` est l'identité PHYSIQUE de tentative (auteur =
//!     writer_id, horloge = HLC, cible, clés) : aucune intégrité inter-base ;
//!     une référence forgée ou étrangère ne peut produire qu'`Indeterminee`.

#![cfg(feature = "obrain")]

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    IntegriteTentative, IssuePublication, ModePublication, Precondition, ReferenceTentative,
    ResolutionEffet, ResolutionTentative, SubstrateStore,
};
use this_harness::attribution::{Attribution, Definition, Etat, Observation, Reception, Resultat, RésolutionBackend};

fn dir(tag: &str) -> std::path::PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "h3v2_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    d
}

/// Réconciliation SOIGNEUSE d'une résolution vers le verdict du backend.
///
/// Le point central de H3 V2 : `Integrale` n'implique PAS `EffetConfirme`.
/// On ne confirme que si TOUS les effets sont engagés ET canoniques (estampille
/// courante égale à celle de la référence). « Engagé puis masqué » est
/// `Incompatible`, pas un faux `EffetConfirme`. `Indeterminee` est `Indetermine`,
/// jamais `AbsenceDefinitive`.
fn reconcilier(res: &ResolutionTentative, reference: &ReferenceTentative) -> RésolutionBackend {
    match res.integrite {
        IntegriteTentative::Indeterminee => RésolutionBackend::Indetermine,
        IntegriteTentative::Partielle => RésolutionBackend::Incompatible,
        IntegriteTentative::Integrale => {
            let tous_canoniques = res.par_cle.iter().all(|e| {
                e.estampille_courante == Some((reference.auteur(), reference.horloge()))
            });
            if tous_canoniques {
                RésolutionBackend::EffetConfirme
            } else {
                // Engagé (historique) mais masqué (estampille courante ≠ référence).
                RésolutionBackend::Incompatible
            }
        }
    }
}

/// Cycle nominal : la réconciliation confirme UNIQUEMENT parce que l'effet est
/// à la fois engagé ET canonique — pas parce que `Integrale` est lu.
#[test]
fn h3_v2_cycle_harness_reel_nominal() {
    let d = dir("nominal");
    let chemin = d.join("kb");

    // 1. DÉCISION : un cycle d'attribution à identité constituée.
    let mut attr = Attribution::new();
    let decision_id = 42u128;
    assert_eq!(
        attr.record(
            decision_id,
            Definition {
                identite: Some("k1".to_string()),
                prevue: Some(("a".to_string(), "m1".to_string())),
                servie: Some(("a".to_string(), "m1".to_string())),
            }
        ),
        this_harness::attribution::Record::Enregistre
    );
    assert_eq!(
        attr.recevoir(
            decision_id,
            Observation {
                fournisseur: "a".to_string(),
                modele: "m1".to_string(),
                resultat: Resultat::Succes,
            }
        ),
        Reception::Recu
    );

    // 2. PRÉPARATION + 3. CONSERVATION DE RÉFÉRENCE (octets persistés AVANT publication).
    let (reference_bytes, id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre: Precondition = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .expect("préparation");
        let reference = prep.reference_tentative();
        let reference_bytes = reference.to_bytes();

        // 4. PUBLICATION réelle ; 5. PERTE DE RÉPONSE : le verdict n'est pas lu.
        let _verdict = store.publier_si_precondition_intacte(prep).expect("publication");
        store.close().expect("fermeture");
        (reference_bytes, id)
    };

    // 6. RÉOUVERTURE + RÉSOLUTION (sans republier, depuis les octets conservés).
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference = ReferenceTentative::from_bytes(&reference_bytes).expect("décodage");
    assert_eq!(reference.cible(), id, "la référence vise la bonne cible");
    let res = store2.resoudre_reference(&reference).expect("résolution");

    // 7. RÉCONCILIATION : Integrale + canonique → EffetConfirme.
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    let verdict = reconcilier(&res, &reference);
    assert!(
        matches!(verdict, RésolutionBackend::EffetConfirme),
        "l'effet est engagé ET canonique : confirmé"
    );
    // La réconciliation alimente le cycle harness (RAM, recréé).
    let tentative = attr.reserver(decision_id).expect("réservation");
    assert!(attr.resoudre(decision_id, tentative, verdict));
    assert_eq!(attr.etat(decision_id).unwrap().etat, Etat::Confirme);
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// `Integrale` n'est PAS `EffetConfirme` : un effet engagé puis masqué reste
/// `Integrale`, mais l'estampille courante diffère de la référence → Incompatible.
#[test]
fn h3_v2_engage_puis_masque_n_est_pas_effet_confirme() {
    let d = dir("masque");
    let chemin = d.join("kb");
    let (reference_bytes, id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .expect("préparation");
        let reference = prep.reference_tentative();
        let bytes = reference.to_bytes();
        assert_eq!(
            store.publier_si_precondition_intacte(prep).expect("publication"),
            IssuePublication::PublieIntegrale
        );
        store.close().expect("fermeture");
        (bytes, id)
    };

    // Un écrivain ultérieur MASQUE l'effet : la valeur courante devient 99.
    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    store2.set_node_property(id, "a", Value::Int64(99));
    let reference = ReferenceTentative::from_bytes(&reference_bytes).expect("décodage");
    let res = store2.resoudre_reference(&reference).expect("résolution");

    // L'engagement historique est INTÉGRAL, mais l'effet n'est plus canonique.
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(res.par_cle[0].etat, ResolutionEffet::Engage);
    assert_ne!(
        res.par_cle[0].estampille_courante,
        Some((reference.auteur(), reference.horloge())),
        "l'estampille courante est celle du masque, pas celle de la tentative"
    );
    assert!(
        matches!(reconcilier(&res, &reference), RésolutionBackend::Incompatible),
        "engagé puis masqué : PAS un EffetConfirme"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// `Indetermine` n'est JAMAIS une absence définitive : une tentative préparée
/// mais jamais publiée rend `Indeterminee`, qui se réconcilie en `Indetermine`
/// (pas `AbsenceDefinitive`).
#[test]
fn h3_v2_indetermine_n_est_pas_absence_definitive() {
    let d = dir("indet");
    let chemin = d.join("kb");
    let reference_bytes = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .expect("préparation");
        let bytes = prep.reference_tentative().to_bytes();
        drop(prep); // jamais publiée
        store.close().expect("fermeture");
        bytes
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference = ReferenceTentative::from_bytes(&reference_bytes).expect("décodage");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    assert_eq!(res.integrite, IntegriteTentative::Indeterminee);
    assert!(
        matches!(reconcilier(&res, &reference), RésolutionBackend::Indetermine),
        "indéterminé, PAS une absence définitive"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}

/// Le codec `to_bytes/from_bytes` survit à la réouverture, et une référence
/// forgée ou étrangère ne peut produire qu'`Indeterminee` (jamais un faux Engage).
#[test]
fn h3_v2_reference_forgee_ou_etrangere_indeterminee() {
    let d = dir("forgee");
    let chemin = d.join("kb");
    let (reference_bytes, id) = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).expect("capture");
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .expect("préparation");
        let bytes = prep.reference_tentative().to_bytes();
        store.publier_si_precondition_intacte(prep).expect("publication");
        store.close().expect("fermeture");
        (bytes, id)
    };

    // 1. Magic invalide → refus NOMMÉ (pas de panique, pas de décodage silencieux).
    let mut corrompu = reference_bytes.clone();
    corrompu[0] = b'X';
    assert!(ReferenceTentative::from_bytes(&corrompu).is_err(), "magic invalide refusé");

    // 2. Référence BIEN FORMÉE mais étrangère (autre base) → Indeterminee.
    let autre_d = dir("autre_base");
    let chemin_b = autre_d.join("kb_b");
    let store_b = SubstrateStore::create(&chemin_b).expect("base B");
    let id_b = store_b.create_node(&["ressource"]);
    store_b.set_node_property(id_b, "a", Value::Int64(1));
    let reference = ReferenceTentative::from_bytes(&reference_bytes).expect("décodage");
    assert_eq!(id, id_b, "même NodeId déterministe de part et d'autre");
    let res = store_b.resoudre_reference(&reference).expect("résolution B");
    assert_eq!(
        res.integrite,
        IntegriteTentative::Indeterminee,
        "une référence étrangère ne peut produire qu'Indeterminee, jamais un Engage faux"
    );
    store_b.close().expect("fermeture B");
    let _ = std::fs::remove_dir_all(&autre_d);
    let _ = std::fs::remove_dir_all(&d);
}

/// Résolution multi-effets : la granularité est PAR CLÉ, jamais un agrégat seul.
/// Une tentative à deux clés, l'une masquée l'autre canonique, reste `Integrale`
/// (les deux sont engagées) — le masquage n'est pas une absence d'engagement.
/// (`Partielle` — une clé engagée, une non retrouvée — n'est pas déclenchée ici :
/// la publication est une bascule atomique, et la compaction n'est pas
/// déterministe ; la réconciliation la mappe néanmoins sur `Incompatible`.)
#[test]
fn h3_v2_multi_effet_granularite_par_cle() {
    let d = dir("partiel");
    let chemin = d.join("kb");
    let reference_bytes = {
        let store = SubstrateStore::create(&chemin).expect("création");
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        store.set_node_property(id, "b", Value::Int64(2));
        let pre = store.capturer_precondition(id, &["a", "b"]).expect("capture");
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10)), ("b", Value::Int64(20))],
                &pre,
                ModePublication::Strict,
            )
            .expect("préparation");
        let bytes = prep.reference_tentative().to_bytes();
        // On publie puis on MASQUE UNE SEULE clé : `b` reste canonique, `a` est masquée.
        store.publier_si_precondition_intacte(prep).expect("publication");
        store.set_node_property(id, "a", Value::Int64(99));
        store.close().expect("fermeture");
        bytes
    };

    let store2 = SubstrateStore::open(&chemin).expect("réouverture");
    let reference = ReferenceTentative::from_bytes(&reference_bytes).expect("décodage");
    let res = store2.resoudre_reference(&reference).expect("résolution");
    // Les deux effets sont engagés (l'un masqué) → Integrale, pas Partielle :
    // le masquage n'est PAS une absence d'engagement.
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(res.par_cle.len(), 2, "une entrée par clé, jamais un agrégat seul");
    // `a` masqué (estampille ≠ référence), `b` canonique.
    let a = res.par_cle.iter().find(|e| e.cle == "a").expect("clé a");
    let b = res.par_cle.iter().find(|e| e.cle == "b").expect("clé b");
    assert_ne!(a.estampille_courante, Some((reference.auteur(), reference.horloge())));
    assert_eq!(b.estampille_courante, Some((reference.auteur(), reference.horloge())));
    assert!(
        matches!(reconcilier(&res, &reference), RésolutionBackend::Incompatible),
        "un effet masqué parmi deux → pas un EffetConfirme intégral"
    );
    store2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
}
