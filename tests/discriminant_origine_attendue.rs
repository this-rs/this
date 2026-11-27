#![cfg(feature = "obrain")]
//! Discriminant du **DOMAINE ATTENDU** — décision `1166641f` (option b) soumise à
//! l'épreuve : *domaine A ATTENDU, enregistrement B, référence B, store B*.
//!
//! # La question, et pourquoi elle n'est pas déjà traitée
//!
//! La décision `1166641f` a choisi de NE PAS mettre le domaine natif dans
//! `EnregistrementOriginal` (pas de bump OBRR v5→v6) et de s'appuyer sur la garde
//! NATIVE du substrate : `resoudre_reference` rend `Indeterminee` si
//! `reference.domaine()` diffère du `domaine_id` du store.
//!
//! Cette garde répond à « cette référence appartient-elle à CE store ? ». Elle ne
//! peut pas répondre à « CE store est-il celui que je visais ? » : quand la
//! référence, l'enregistrement et le store sont tous B, la cohérence est totale,
//! la garde native accepte, et `reconcilier` confirme — alors que l'intention
//! était A. Ce fichier mesure ce cas, puis mesure la garde qui le ferme.
//!
//! # Ce que ce témoin DÉMONTRE
//!
//! 1. Deux bases INDÉPENDANTES ont deux `domaine_id` distincts.
//! 2. La garde native ACCEPTE référence B dans store B (`Integrale`, pas
//!    `Indeterminee`) — elle n'est pas fautive : elle n'a jamais reçu l'intention.
//! 3. `reconcilier(enr_B, réf_B, res_B) == Confirme`, y compris si l'on RENOMME
//!    le label `enr.domaine` en « base-a » : un nom n'est pas une identité.
//! 4. `verifier_domaine_attendu(store_B, domaine_A) == Etranger` — et le contrôle
//!    positif `…, domaine_B) == Concorde`, donc le refus n'est pas un refus en
//!    bloc : il discrimine.
//! 5. `resoudre_operation_sous_domaine(…, domaine_A, …) == DomaineEtranger`, et le
//!    cycle `Attribution` est INTACT après ce refus (la même tentative résout
//!    encore) : aucune transition n'a eu lieu avant la garde.
//! 6. Contrôle positif du chemin COMPLET : le même appel avec `domaine_A` sur le
//!    store A rend `Operation(Resolu(true))`. Forme identique, seule la valeur
//!    ATTENDUE change — c'est le discriminant.
//! 7. Une copie INTÉGRALE du répertoire porte le même `domaine_id` et la même
//!    résolution : l'égalité d'octets n'est pas une preuve d'origine, et cette
//!    limite est NOMMÉE, pas contournée.
//!
//! # Ce que ce témoin ne démontre PAS, et pourquoi
//!
//! - La distinction d'une base et de sa COPIE : elle exige un fork DÉCLARÉ côté
//!   substrate (`domaine_id::forker`), hors du périmètre du consommateur.
//! - `IssueDomaineAttendu::SansIdentite` (`domaine_id == 0`) : aucun chemin
//!   public de ce pin ne produit un store à domaine nul (`create` en matérialise
//!   toujours un). Branche écrite, NON EXERCÉE ici.
//! - L'authenticité du domaine attendu : il vient de l'appelant, et un appelant
//!   qui se trompe d'intention ne peut pas être corrigé par ce module. C'est
//!   exactement pourquoi la source doit être indépendante des octets.

use std::path::{Path, PathBuf};

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    IntegriteTentative, IssuePublication, ModePublication, Precondition, ReferenceTentative,
    SubstrateStore,
};
use this::reconciliation::{
    DomaineAttendu, ENREGISTREMENT_VERSION, EnregistrementOriginal, IssueDomaineAttendu,
    IssueResolutionOperation, IssueResolutionSousDomaine, VerdictReconciliation, ecrire_durable,
    reconcilier, resoudre_operation_sous_domaine, verifier_domaine_attendu,
};
use this_harness::attribution::{
    Attribution, Definition, Observation, Resultat, RésolutionBackend, Tentative,
};

// ───────────────────────────────────────────────────────────────────────────────
// Outillage — aucune valeur fabriquée : domaine, écrivain, génération, autorité
// et décision sortent du store et du cycle réels.
// ───────────────────────────────────────────────────────────────────────────────

fn dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!(
        "discriminant_domaine_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("création du répertoire de travail");
    d
}

fn def(identite: &str) -> Definition {
    Definition {
        identite: Some(identite.to_string()),
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

/// Une base indépendante ET son cycle consommateur réel : record durable écrit
/// AVANT publication, réservation réelle, publication intégrale.
struct Base {
    chemin: PathBuf,
    domaine: u128,
    enr: EnregistrementOriginal,
    reference: ReferenceTentative,
    attr: Attribution,
    tentative: Tentative,
}

fn construire(d: &Path, label: &str, k: u128, decision_id: u128) -> Base {
    let chemin = d.join("kb");
    let record = d.join("record.obrr");

    let store = SubstrateStore::create(&chemin).expect("création de la base");
    let domaine = store.domaine_id();
    assert_ne!(
        domaine, 0,
        "une base créée porte une identité native non nulle"
    );

    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre: Precondition = store
        .capturer_precondition(id, &["a"])
        .expect("capture de précondition");
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let reference = prep.reference_tentative();
    assert_eq!(
        reference.domaine(),
        domaine,
        "OBRT v2 : la référence porte le domaine de SA base"
    );

    let mut attr = Attribution::new();
    attr.record(decision_id, def("k1"));
    attr.recevoir(decision_id, obs());
    let tentative = attr.reserver(decision_id).expect("réservation réelle");

    let enr = EnregistrementOriginal {
        version: ENREGISTREMENT_VERSION,
        k,
        domaine: label.to_string(),
        ecrivain: reference.auteur(),
        generation: tentative.generation(),
        decision_id: tentative.decision_id(),
        autorite_consommateur: tentative.autorite(),
        cible: reference.cible(),
        effets_attendus: vec![("a".to_string(), Value::Int64(10))],
        reference_bytes: reference.to_bytes(),
    };
    assert!(
        enr.verifier_liaison(&tentative),
        "la liaison au descripteur RÉEL est vérifiée avant d'écrire"
    );
    ecrire_durable(&record, &enr.encoder().expect("encodage")).expect("écriture durable du record");
    assert_eq!(
        store
            .publier_si_precondition_intacte(prep)
            .expect("publication"),
        IssuePublication::PublieIntegrale,
        "publication intégrale"
    );
    store.close().expect("fermeture");

    Base {
        chemin,
        domaine,
        enr,
        reference,
        attr,
        tentative,
    }
}

fn copier_recursif(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let cible = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copier_recursif(&e.path(), &cible)?;
        } else {
            std::fs::copy(e.path(), &cible)?;
        }
    }
    Ok(())
}

// ───────────────────────────────────────────────────────────────────────────────
// 1. Le discriminant : domaine A attendu, enregistrement B, référence B, store B
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn discriminant_domaine_attendu_a_contre_b() {
    let d = dir("a_contre_b");
    let (a, mut b) = (
        construire(&d.join("a"), "base-a", 7, 11),
        construire(&d.join("b"), "base-b", 7, 11),
    );

    assert_ne!(
        a.domaine, b.domaine,
        "MESURÉ : deux bases INDÉPENDANTES → deux `domaine_id` distincts"
    );
    assert_eq!(a.enr.k, b.enr.k, "même K logique des deux côtés");
    assert_eq!(
        a.enr.effets_attendus, b.enr.effets_attendus,
        "même contenu attendu des deux côtés"
    );

    // ── 1. La garde NATIVE accepte B dans B ────────────────────────────────────
    let store_b = SubstrateStore::open(&b.chemin).expect("réouverture B");
    let res_b = store_b
        .resoudre_reference(&b.reference)
        .expect("résolution de la référence B dans le store B");
    assert_ne!(
        res_b.integrite,
        IntegriteTentative::Indeterminee,
        "la garde native ACCEPTE : la référence nomme le domaine du store. Elle est juste — \
         elle ne pouvait pas savoir que l'intention était A."
    );
    assert_eq!(
        res_b.integrite,
        IntegriteTentative::Integrale,
        "MESURÉ : intégrité intégrale, donc aucun soupçon côté substrate"
    );

    // ── 2. `reconcilier` CONFIRME ──────────────────────────────────────────────
    assert_eq!(
        reconcilier(&b.enr, &b.reference, &res_b),
        VerdictReconciliation::Confirme,
        "référence B, enregistrement B, store B : TROIS FOIS B, donc confirmation. \
         C'est précisément la confirmation CONTRAIRE À L'INTENTION A que le consommateur doit empêcher."
    );

    // ── 3. Le LABEL de l'enregistrement n'est pas une autorité ────────────────
    let mut b_renomme_a = b.enr.clone();
    b_renomme_a.domaine = "base-a".to_string();
    assert_eq!(
        reconcilier(&b_renomme_a, &b.reference, &res_b),
        VerdictReconciliation::Confirme,
        "renommer le label ne change RIEN : `enr.domaine` est un nom, pas une identité"
    );

    // ── 4. La garde du CONSOMMATEUR, seule, refuse — et dit pourquoi ──────────
    assert_eq!(
        verifier_domaine_attendu(&store_b, a.domaine),
        IssueDomaineAttendu::Etranger {
            attendu: a.domaine,
            courant: b.domaine,
        },
        "DÉMONTRÉ : l'attendu A contre le natif B → refus, sans consulter aucun contenu"
    );
    assert_eq!(
        verifier_domaine_attendu(&store_b, b.domaine),
        IssueDomaineAttendu::Concorde,
        "CONTRÔLE POSITIF : la même garde ACCEPTE quand l'intention est B — elle discrimine, \
         elle ne refuse pas en bloc"
    );
    assert_eq!(
        verifier_domaine_attendu(&store_b, 0),
        IssueDomaineAttendu::Etranger {
            attendu: 0,
            courant: b.domaine,
        },
        "une intention INCONNUE (0) n'autorise rien"
    );

    // ── 5. Le chemin opérationnel refuse AVANT toute transition ──────────────
    let issue = resoudre_operation_sous_domaine(
        &DomaineAttendu::nouveau(&store_b, a.domaine),
        &mut b.attr,
        b.enr.decision_id,
        b.tentative,
        &b.enr,
        &b.reference,
        &res_b,
    );
    assert_eq!(
        issue,
        IssueResolutionSousDomaine::DomaineEtranger {
            attendu: a.domaine,
            courant: b.domaine,
        },
        "le chemin opérationnel refuse AVANT liaison, AVANT réconciliation, AVANT résolution"
    );
    assert!(
        b.attr.resoudre(
            b.enr.decision_id,
            b.tentative,
            RésolutionBackend::EffetConfirme
        ),
        "DÉMONTRÉ : le cycle est INTACT après le refus (la même tentative résout encore) \
         — aucune transition n'a eu lieu"
    );

    // ── 6. Contrôle positif du chemin COMPLET : intention A dans le store A ──
    let mut a = a;
    let store_a = SubstrateStore::open(&a.chemin).expect("réouverture A");
    let res_a = store_a
        .resoudre_reference(&a.reference)
        .expect("résolution de la référence A dans le store A");
    assert_eq!(res_a.integrite, IntegriteTentative::Integrale);
    let issue = resoudre_operation_sous_domaine(
        &DomaineAttendu::nouveau(&store_a, a.domaine),
        &mut a.attr,
        a.enr.decision_id,
        a.tentative,
        &a.enr,
        &a.reference,
        &res_a,
    );
    assert_eq!(
        issue,
        IssueResolutionSousDomaine::Operation(IssueResolutionOperation::Resolu(true)),
        "CONTRÔLE POSITIF : même forme d'appel, seule la valeur ATTENDUE change → résolution"
    );

    store_a.close().expect("fermeture A");
    store_b.close().expect("fermeture B");
    let _ = std::fs::remove_dir_all(&d);
}

// ───────────────────────────────────────────────────────────────────────────────
// 2. L'égalité d'octets n'est pas une preuve d'origine — la limite, NOMMÉE
// ───────────────────────────────────────────────────────────────────────────────

#[test]
fn copie_integrale_partage_le_domaine_donc_les_octets_ne_prouvent_rien() {
    let d = dir("copie");
    let b = construire(&d.join("b"), "base-b", 7, 11);

    // Copie INTÉGRALE du répertoire, non déclarée comme fork.
    copier_recursif(&b.chemin, &d.join("c/kb")).expect("copie intégrale de la base");
    let store_c = SubstrateStore::open(d.join("c/kb")).expect("ouverture de la copie");

    assert_eq!(
        store_c.domaine_id(),
        b.domaine,
        "MESURÉ : le domaine suit les FICHIERS, pas le chemin — une copie porte le MÊME domaine"
    );
    let res_c = store_c
        .resoudre_reference(&b.reference)
        .expect("résolution dans la copie");
    assert_eq!(
        res_c.integrite,
        IntegriteTentative::Integrale,
        "MESURÉ : la référence de B se résout à l'IDENTIQUE dans la copie — les octets OBRT \
         et OBRR ne distinguent pas une base de sa copie"
    );
    assert_eq!(
        verifier_domaine_attendu(&store_c, b.domaine),
        IssueDomaineAttendu::Concorde,
        "LIMITE ASSUMÉE, nommée et non contournée : la garde sépare deux bases INDÉPENDANTES ; \
         séparer une base de sa copie exige un fork DÉCLARÉ côté substrate (`domaine_id::forker`)"
    );

    store_c.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&d);
}

// ───────────────────────────────────────────────────────────────────────────────
// 3. CONTRE-ÉPREUVE : sans la garde, le chemin existant CONFIRME
// ───────────────────────────────────────────────────────────────────────────────

/// La contre-épreuve qui rend le témoin 1 discriminant.
///
/// Le chemin de la décision `1166641f` (option b) s'arrête à `resoudre_operation`
/// — liaison → réconciliation → résolution. Exécuté sur la MÊME situation
/// (intention A, enregistrement B, référence B, store B), il RÉSOUT. Le défaut
/// n'est donc pas une hypothèse : il est reproduit. Sans ce témoin, un refus
/// observé avec la garde pourrait venir de n'importe quoi d'autre.
#[test]
fn sans_la_garde_le_chemin_existant_confirme_contre_l_intention() {
    use this::reconciliation::resoudre_operation;

    let d = dir("contre_epreuve");
    let a = construire(&d.join("a"), "base-a", 7, 11);
    let mut b = construire(&d.join("b"), "base-b", 7, 11);
    assert_ne!(a.domaine, b.domaine);

    let store_b = SubstrateStore::open(&b.chemin).expect("réouverture B");
    let res_b = store_b
        .resoudre_reference(&b.reference)
        .expect("résolution de la référence B dans le store B");

    // Le chemin SANS garde de domaine attendu : il résout.
    let issue = resoudre_operation(
        &mut b.attr,
        b.enr.decision_id,
        b.tentative,
        &b.enr,
        &b.reference,
        &res_b,
    );
    assert_eq!(
        issue,
        IssueResolutionOperation::Resolu(true),
        "CONTRE-ÉPREUVE MESURÉE : sans garde de domaine attendu, l'intention A est CONFIRMÉE          par la base B — c'est le défaut que la décision 1166641f (option b) laissait ouvert"
    );

    // Et avec la garde, la même intention n'atteint jamais la résolution.
    let d2 = dir("contre_epreuve_2");
    let b2 = construire(&d2.join("b"), "base-b", 7, 11);
    let store_b2 = SubstrateStore::open(&b2.chemin).expect("réouverture B2");
    let res_b2 = store_b2
        .resoudre_reference(&b2.reference)
        .expect("résolution B2");
    let mut b2 = b2;
    assert_eq!(
        resoudre_operation_sous_domaine(
            &DomaineAttendu::nouveau(&store_b2, a.domaine),
            &mut b2.attr,
            b2.enr.decision_id,
            b2.tentative,
            &b2.enr,
            &b2.reference,
            &res_b2,
        ),
        IssueResolutionSousDomaine::DomaineEtranger {
            attendu: a.domaine,
            courant: b2.domaine,
        },
        "même situation, avec la garde : refus"
    );

    store_b.close().expect("fermeture");
    store_b2.close().expect("fermeture 2");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}
