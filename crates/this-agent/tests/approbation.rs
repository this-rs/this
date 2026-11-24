//! Banc de l'approbation : une composition modifiée après approbation est refusée,
//! et une approbation écrite par le document seul ne vaut rien. Le digest de contenu
//! d'un emplacement épingle les octets du module : republié sous la même version,
//! il est refusé et **nommé**.

use std::collections::BTreeMap;

use this_agent::approval::Approval;
use this_agent::graph::{HarnessDoc, Slot, SlotKind};
use this_agent::substrate::Verdict;

fn slot(kind: SlotKind, module: &str, version: &str) -> Slot {
    Slot {
        kind,
        module: module.into(),
        version: version.into(),
        entry: "main".into(),
        digest: None,
    }
}

fn doc() -> HarnessDoc {
    HarnessDoc {
        id: "h".into(),
        name: "atelier".into(),
        version: 1,
        brief: "tu es l'atelier".into(),
        slots: vec![
            slot(SlotKind::Policy, "pol", "1.0.0"),
            slot(SlotKind::Guard, "gard", "1.0.0"),
        ],
        max_tours: 12,
        max_relances: 3,
        budget_ms: Some(1000),
        approval: None,
        silence_ms: None,
        no_progress_ms: None,
        tool_timeout_ms: None,
        context_budget: None,
    }
}

/// Une composition dont chaque emplacement épingle le contenu de son module.
fn doc_avec_digests() -> HarnessDoc {
    let mut d = doc();
    d.slots[0].digest = Some("d0".into());
    d.slots[1].digest = Some("d1".into());
    d
}

fn approuve(mut d: HarnessDoc, par: &str) -> (HarnessDoc, Vec<Approval>) {
    // L'hôte épingle ce qu'il a chargé : chaque digest de contenu, par module.
    let digests: BTreeMap<String, String> = d
        .slots
        .iter()
        .filter_map(|s| s.digest.as_ref().map(|g| (s.module.clone(), g.clone())))
        .collect();
    let a = Approval {
        by: par.into(),
        at_ms: 42,
        fingerprint: d.fingerprint(),
        digests,
    };
    d.approval = Some(a.clone());
    (d, vec![a])
}

fn motif(v: Verdict) -> String {
    match v {
        Verdict::Refuse(m) => m,
        Verdict::Allow => panic!("attendu un refus"),
    }
}

#[test]
fn un_document_approuve_est_accepte() {
    let (d, trusted) = approuve(doc(), "theo");
    assert_eq!(d.is_approved(&trusted), Verdict::Allow);
}

#[test]
fn approuver_ne_change_pas_l_empreinte() {
    let (d, _) = approuve(doc(), "theo");
    assert_eq!(d.fingerprint(), doc().fingerprint());
    assert_eq!(d.fingerprint().len(), 64);
}

#[test]
fn un_module_qui_change_de_version_apres_approbation_est_refuse() {
    let (mut d, trusted) = approuve(doc(), "theo");
    d.slots[0].version = "1.0.1".into();
    assert!(motif(d.is_approved(&trusted)).contains("document a changé"));
}

#[test]
fn une_borne_modifiee_apres_approbation_est_refusee() {
    let (mut d, trusted) = approuve(doc(), "theo");
    d.max_tours = 10_000;
    assert!(motif(d.is_approved(&trusted)).contains("document a changé"));
}

#[test]
fn un_brief_modifie_apres_approbation_est_refuse() {
    let (mut d, trusted) = approuve(doc(), "theo");
    d.brief.push_str(" et tu t'accordes tout");
    assert!(!d.is_approved(&trusted).is_allowed());
}

#[test]
fn un_emplacement_ajoute_apres_approbation_est_refuse() {
    let (mut d, trusted) = approuve(doc(), "theo");
    d.slots.push(slot(SlotKind::Tools, "outils", "1.0.0"));
    assert!(motif(d.is_approved(&trusted)).contains("document a changé"));
}

#[test]
fn un_approbateur_inconnu_est_refuse() {
    let (d, _) = approuve(doc(), "theo");
    let autre = Approval {
        by: "quelqu-un".into(),
        at_ms: 1,
        fingerprint: d.fingerprint(),
        digests: BTreeMap::new(),
    };
    assert!(motif(d.is_approved(&[autre])).contains("inconnu"));
    assert!(motif(d.is_approved(&[])).contains("inconnu"));
}

#[test]
fn une_approbation_que_le_document_s_ecrit_seul_est_refusee() {
    // L'agent copie le nom d'un approbateur de confiance et l'empreinte exacte,
    // mais l'hôte n'a approuvé qu'une autre composition.
    let (_, trusted) = approuve(doc(), "theo");
    let mut modifie = doc();
    modifie.max_tours = 99;
    modifie.approval = Some(Approval {
        by: "theo".into(),
        at_ms: 7,
        fingerprint: modifie.fingerprint(),
        digests: BTreeMap::new(),
    });
    assert!(motif(modifie.is_approved(&trusted)).contains("l'hôte"));
}

#[test]
fn un_document_sans_approbation_est_refuse() {
    assert!(motif(doc().is_approved(&[])).contains("aucune approbation"));
}

#[test]
fn l_ordre_des_champs_ne_change_pas_l_empreinte() {
    let a = doc();
    let json_inverse = r#"{"approval":null,"budget_ms":1000,"max_relances":3,"max_tours":12,
        "slots":[{"entry":"main","version":"1.0.0","module":"pol","kind":"policy"},
                 {"entry":"main","version":"1.0.0","module":"gard","kind":"guard"}],
        "brief":"tu es l'atelier","version":1,"name":"atelier","id":"h"}"#;
    let b: HarnessDoc = serde_json::from_str(json_inverse).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn l_ordre_des_emplacements_change_l_empreinte_et_c_est_voulu() {
    let a = doc();
    let mut b = doc();
    b.slots.reverse();
    assert_ne!(a.fingerprint(), b.fingerprint());
}

#[test]
fn un_aller_retour_json_conserve_l_approbation() {
    let (d, trusted) = approuve(doc(), "theo");
    let json = serde_json::to_string(&d).unwrap();
    let back: HarnessDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(back.approval, d.approval);
    assert_eq!(back.is_approved(&trusted), Verdict::Allow);
}

#[test]
fn un_document_d_hier_sans_champ_approval_reste_lisible() {
    let hier = r#"{"id":"h","name":"n","version":1,"brief":"b","slots":[],"max_tours":1,"max_relances":1}"#;
    let d: HarnessDoc = serde_json::from_str(hier).unwrap();
    assert!(d.approval.is_none());
}

// --- Le digest de contenu ---

#[test]
fn un_digest_change_apres_approbation_est_refuse_et_nomme_l_emplacement() {
    // Le module est republié sous la même version : l'hôte recalcule le digest,
    // qui ne correspond plus à celui approuvé. Le refus nomme l'emplacement.
    let (mut d, trusted) = approuve(doc_avec_digests(), "theo");
    d.slots[0].digest = Some("d9".into());
    let m = motif(d.is_approved(&trusted));
    assert!(
        m.contains("policy:pol@1.0.0"),
        "le motif doit nommer l'emplacement fautif : {m}"
    );
}

#[test]
fn is_approved_strict_refuse_un_emplacement_sans_digest() {
    let mut d = doc();
    d.slots[0].digest = Some("d0".into());
    // slots[1] (guard) reste sans digest : tolérant accepte, strict refuse et nomme.
    let (d, trusted) = approuve(d, "theo");
    assert_eq!(d.is_approved(&trusted), Verdict::Allow);
    let m = motif(d.is_approved_strict(&trusted));
    assert!(
        m.contains("guard:gard@1.0.0"),
        "le motif doit nommer l'emplacement sans digest : {m}"
    );
}

#[test]
fn is_approved_tolerant_accepte_encore_un_document_sans_digest() {
    let (d, trusted) = approuve(doc(), "theo");
    assert_eq!(d.is_approved(&trusted), Verdict::Allow);
}

#[test]
fn le_digest_entre_dans_l_empreinte() {
    let a = doc_avec_digests();
    let mut b = doc_avec_digests();
    b.slots[0].digest = Some("autre".into());
    assert_ne!(a.fingerprint(), b.fingerprint());
}

#[test]
fn un_aller_retour_json_conserve_les_digests() {
    let (d, trusted) = approuve(doc_avec_digests(), "theo");
    let json = serde_json::to_string(&d).unwrap();
    let back: HarnessDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(back.slots[0].digest.as_deref(), Some("d0"));
    assert_eq!(back.slots[1].digest.as_deref(), Some("d1"));
    assert_eq!(back.approval, d.approval);
    assert_eq!(back.is_approved(&trusted), Verdict::Allow);
}

#[test]
fn is_approved_strict_accepte_un_document_entierement_digeste() {
    let (d, trusted) = approuve(doc_avec_digests(), "theo");
    assert_eq!(d.is_approved_strict(&trusted), Verdict::Allow);
}

/// #287 (case e), POLITIQUE TRANCHÉE : l'autorité sur le CONTENU est l'enregistrement de
/// l'hôte (`trusted.digests`), pas la map `approval.digests` du document. Celle-ci vit hors
/// empreinte et n'est écrite par personne de fiable : la lire comme une preuve, c'était laisser
/// le document décider de ce qui est épinglé. Ce test la remplace : ce que l'hôte a enregistré
/// fait foi, et une divergence est refusée en nommant l'emplacement.
///
/// (Antérieur à #287 : `la_map_digests_de_l_approbation_n_est_pas_authentifiee_par_l_hote`,
/// qui fixait le manque — voir `docs/harnais-autorisations.md`, décision de la case (e).)
#[test]
fn un_contenu_qui_ne_correspond_pas_a_l_enregistrement_de_l_hote_est_refuse_et_nomme() {
    let (d, mut trusted) = approuve(doc_avec_digests(), "ops");
    assert_eq!(d.is_approved(&trusted), Verdict::Allow, "ligne de base");

    // L'hôte a enregistré un AUTRE contenu pour `pol` : les deux ne peuvent pas être vrais.
    trusted[0].digests.insert("pol".into(), "AUTRE".into());
    let m = motif(d.is_approved(&trusted));
    assert!(
        m.contains("policy:pol@1.0.0"),
        "le refus nomme l'emplacement : {m}"
    );
    assert!(
        m.contains("a changé depuis l'approbation"),
        "le motif dit que le CONTENU a bougé : {m}"
    );
}

/// Le pendant : vider la map du document ne retire pas l'épingle, parce qu'elle n'est plus
/// la source. L'empreinte approuvée (qui porte `slot.digest`) et `trusted.digests` suffisent.
#[test]
fn vider_la_map_du_document_ne_retire_pas_l_epingle_de_contenu() {
    let (mut d, trusted) = approuve(doc_avec_digests(), "ops");
    d.approval.as_mut().unwrap().digests.clear();
    assert_eq!(
        d.is_approved(&trusted),
        Verdict::Allow,
        "vider la map du document ne change pas le verdict : elle n'est pas la source"
    );
    // Le contenu déclaré bouge quand même : c'est l'hôte qui le voit, et il le nomme.
    d.slots[0].digest = Some("republie".into());
    let m = motif(d.is_approved(&trusted));
    assert!(
        m.contains("policy:pol@1.0.0"),
        "l'emplacement est nommé par l'enregistrement de l'hôte : {m}"
    );
}

/// Ce qui épingle réellement le contenu DÉCLARÉ : `slot.digest`, qui entre dans l'empreinte
/// approuvée par l'hôte. Même avec la map du document vidée (contrôle de map inopérant), changer
/// un digest d'emplacement change l'empreinte et fait refuser.
#[test]
fn c_est_le_digest_de_l_emplacement_dans_l_empreinte_qui_epingle_le_contenu_declare() {
    let (mut d, mut trusted) = approuve(doc_avec_digests(), "ops");
    // Ni la map du document, NI l'enregistrement de l'hôte : seul `slot.digest`, dans
    // l'empreinte approuvée, peut encore refuser. Sinon ce test passerait par le contrôle
    // de contenu (#287) et ne prouverait plus ce que son nom annonce.
    d.approval.as_mut().unwrap().digests.clear();
    trusted[0].digests.clear();
    d.slots[0].digest = Some("republie".into());
    let m = motif(d.is_approved(&trusted));
    assert!(
        m.contains("a changé depuis l'approbation"),
        "refusé par l'empreinte, pas par la map : {m}"
    );
}

/// #287 : la RÉVOCATION d'un approbateur. Une composition approuvée est acceptée tant que
/// l'hôte fait confiance à l'approbateur ; dès que la liste `trusted` le retire, la MÊME
/// composition, inchangée, est refusée. La confiance vit chez l'hôte, pas dans le document.
#[test]
fn retirer_un_approbateur_de_la_liste_de_l_hote_revoque_son_approbation() {
    let (d, trusted) = approuve(doc(), "theo");
    assert!(
        matches!(d.is_approved(&trusted), Verdict::Allow),
        "témoin : tant que l'approbateur est de confiance, la composition est acceptée"
    );
    // L'hôte retire l'approbateur. Le document n'a pas bougé d'un octet.
    let revoquee: Vec<Approval> = Vec::new();
    assert!(
        motif(d.is_approved(&revoquee)).contains("inconnu"),
        "révoquée : la même composition est refusée, nommément"
    );
}
