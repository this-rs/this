//! Un fait présent mais NON CONFORME est une erreur, pas « absent » ni un fait vide.
//! Exercé sur l'ADAPTATEUR RÉEL (pas le faux store).
#![cfg(feature = "obrain-cognitive")]

use std::path::PathBuf;
use std::sync::Arc;

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_engine::ObrainDB;
use obrain_substrate::ModePublication;
use this::memoire::{
    AdaptateurNatif, ErreurStockage, LABEL_FAIT, PROP_PROJET, PROP_TEXTE, StockageFait,
};

fn repertoire() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "this-nc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    base
}

#[test]
fn un_projet_non_textuel_est_un_fait_non_conforme() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire");
    let db = ObrainDB::open(&chemin).expect("open");
    let store = db.substrate_handle().expect("substrate");

    let cible = store.create_node(&[LABEL_FAIT]);
    let pre = store
        .capturer_precondition(cible, &[PROP_TEXTE, PROP_PROJET])
        .expect("capture");
    let prep = store
        .preparer_transformation(
            cible,
            &[
                (PROP_TEXTE, Value::from("un fait")),
                (PROP_PROJET, Value::Int64(7)),
            ],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let _ = store
        .publier_si_precondition_intacte(prep)
        .expect("publication");

    let adaptateur: Arc<dyn StockageFait> = Arc::new(AdaptateurNatif::nouveau(store));
    match adaptateur.lire(cible.0) {
        Err(ErreurStockage::AConfirmer { raison, .. }) => {
            assert!(raison.contains("non textuel"), "{raison}")
        }
        autre => panic!("attendu non conforme, mesuré {autre:?}"),
    }
    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&chemin);
}

#[test]
fn un_texte_sans_projet_est_un_fait_non_conforme() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire");
    let db = ObrainDB::open(&chemin).expect("open");
    let store = db.substrate_handle().expect("substrate");

    let cible = store.create_node(&[LABEL_FAIT]);
    // Écrit `texte` SEUL, sans `projet`.
    let pre = store
        .capturer_precondition(cible, &[PROP_TEXTE])
        .expect("capture");
    let prep = store
        .preparer_transformation(
            cible,
            &[(PROP_TEXTE, Value::from("un fait"))],
            &pre,
            ModePublication::Strict,
        )
        .expect("préparation");
    let _ = store
        .publier_si_precondition_intacte(prep)
        .expect("publication");

    let adaptateur: Arc<dyn StockageFait> = Arc::new(AdaptateurNatif::nouveau(store));
    match adaptateur.lire(cible.0) {
        Err(ErreurStockage::AConfirmer { raison, .. }) => {
            assert!(raison.contains("pas de `projet`"), "{raison}")
        }
        autre => panic!("attendu non conforme, mesuré {autre:?}"),
    }
    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&chemin);
}
