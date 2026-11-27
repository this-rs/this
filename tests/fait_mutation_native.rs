//! La mutation RÉELLE du fait (clés `texte` et `projet`) par le contrat natif du
//! substrat — pas par un ledger cognitif séparé.
//!
//! Le discriminant : la `ReferenceTentative` obtenue de `preparer_transformation`
//! porte les clés EFFECTIVEMENT écrites (`texte`, `projet`). Une publication
//! `obs:*` réussie ailleurs ne couvre PAS cette écriture ; ici, c'est la
//! référence de CETTE mutation qui est résolue.
#![cfg(feature = "obrain")]

use std::path::PathBuf;

use obrain_common::types::NodeId;
use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_engine::ObrainDB;
use obrain_substrate::{EtatPropriete, IssuePublication, ModePublication, SubstrateStore};

fn repertoire() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "this-fait-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    base
}

fn texte_de(store: &SubstrateStore, cible: NodeId) -> String {
    match store
        .lire_etat_propriete(cible, "texte")
        .expect("lire texte")
    {
        EtatPropriete::Presente { valeur, .. } => valeur.as_str().unwrap_or("").to_string(),
        _ => panic!("texte attendu présent"),
    }
}

fn projet_de(store: &SubstrateStore, cible: NodeId) -> String {
    match store
        .lire_etat_propriete(cible, "projet")
        .expect("lire projet")
    {
        EtatPropriete::Presente { valeur, .. } => valeur.as_str().unwrap_or("").to_string(),
        _ => panic!("projet attendu présent"),
    }
}

#[test]
fn le_fait_est_ecrit_puis_corrige_avec_la_reference_de_sa_mutation() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire");
    let db = ObrainDB::open(&chemin).expect("open");
    let store = db.substrate_handle().expect("substrate handle");

    // Le label doit rester aligné sur `this::memoire::LABEL_FAIT`.
    let cible = store.create_node(&[":Fait"]);

    // ── création du fait : texte + projet, précondition déclare les deux ──
    let pre = store
        .capturer_precondition(cible, &["texte", "projet"])
        .expect("capture création");
    let nouvelles = [
        ("texte", Value::from("déploiement prévu mardi")),
        ("projet", Value::from("alpha")),
    ];
    let prep = store
        .preparer_transformation(cible, &nouvelles, &pre, ModePublication::Strict)
        .expect("préparation création");
    let reference_creation = prep.reference_tentative();
    assert!(matches!(
        store.publier_si_precondition_intacte(prep),
        Ok(IssuePublication::PublieIntegrale)
    ));

    assert_eq!(texte_de(&store, cible), "déploiement prévu mardi");
    assert_eq!(projet_de(&store, cible), "alpha");

    // La référence couvre les DEUX clés écrites (la mutation évaluée), pas un
    // `obs:*` d'un autre registre.
    let resolution = store
        .resoudre_reference(&reference_creation)
        .expect("résoudre création");
    assert_eq!(resolution.nb_effets(), 2, "texte et projet couverts");
    assert_eq!(
        resolution.integrite(),
        obrain_substrate::IntegriteTentative::Integrale,
        "les deux effets attendus sont engagés"
    );

    // ── correction : texte seulement, projet ÉPINGLÉ dans la précondition ──
    let pre2 = store
        .capturer_precondition(cible, &["texte", "projet"])
        .expect("capture correction");
    let nouvelles2 = [("texte", Value::from("déploiement prévu jeudi"))];
    let prep2 = store
        .preparer_transformation(cible, &nouvelles2, &pre2, ModePublication::Strict)
        .expect("préparation correction");
    let reference_correction = prep2.reference_tentative();
    assert!(matches!(
        store.publier_si_precondition_intacte(prep2),
        Ok(IssuePublication::PublieIntegrale)
    ));

    assert_eq!(texte_de(&store, cible), "déploiement prévu jeudi");
    assert_eq!(projet_de(&store, cible), "alpha"); // projet conservé

    // La correction est résolue ELLE AUSSI : une seule clé écrite, intégrale.
    let resolution_correction = store
        .resoudre_reference(&reference_correction)
        .expect("résoudre correction");
    assert_eq!(
        resolution_correction.nb_effets(),
        1,
        "seul `texte` est écrit"
    );
    assert_eq!(
        resolution_correction.integrite(),
        obrain_substrate::IntegriteTentative::Integrale,
        "l'effet de correction est engagé"
    );

    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&chemin);
}
