//! Le parcours mémoire par le SERVICE et l'ADAPTATEUR de production, sur une vraie base,
//! en deux « processus » : fermeture propre puis réouverture.
//!
//! Ce témoin prouve la survie à une fin de processus PROPRE (fermeture + réouverture). Il ne
//! simule PAS une coupure de courant, et ne qualifie PAS la reprise après réponse perdue : la
//! référence de reprise est conservée dans une variable par l'appelant, ce qui démontre la
//! résolution après réouverture mais pas la persistance de la référence elle-même.
#![cfg(feature = "obrain-cognitive")]

use std::path::PathBuf;
use std::sync::Arc;

use obrain_engine::ObrainDB;
use obrain_substrate::IntegriteTentative;
use this::memoire::{AdaptateurNatif, Memoire, Resultat, StockageFait};

fn repertoire() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "this-parcours-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    base
}

fn memoire_sur(chemin: &std::path::Path) -> (ObrainDB, Memoire) {
    let db = ObrainDB::open(chemin).expect("ouverture de la base");
    let store = db.substrate_handle().expect("substrate handle");
    let adaptateur: Arc<dyn StockageFait> = Arc::new(AdaptateurNatif::nouveau(store));
    let memoire = Memoire::new(adaptateur);
    (db, memoire)
}

#[test]
fn memoriser_corriger_fermer_rouvrir_rappeler_la_correction() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire de test");

    // ── Processus A ──
    let (id_corrige, reference_correction) = {
        let (db, memoire) = memoire_sur(&chemin);
        memoire.memoriser("alpha", "le déploiement passe par Caddy");
        let Resultat::Ecrit { id, .. } = memoire.memoriser("alpha", "le port est 8080") else {
            panic!("la seconde mémorisation doit réussir");
        };
        let Resultat::Ecrit { reference, .. } = memoire.reviser("alpha", id, "le port est 9090")
        else {
            panic!("la correction doit réussir");
        };
        db.close().expect("fermeture propre");
        (id, reference)
    }; // fin du processus A : adaptateur, store et db relâchés

    // ── Processus B ──
    let (db, memoire) = memoire_sur(&chemin);

    // Résolution de la référence de correction conservée : l'effet `texte` est engagé.
    let reference = reference_correction.expect("la correction porte sa référence");
    let resolution = memoire.resoudre(&reference).expect("résolution");
    assert_eq!(
        resolution.integrite(),
        IntegriteTentative::Integrale,
        "l'effet de correction est engagé"
    );

    // Rappel : la version CORRIGÉE est servie, dans le bon projet.
    let rappels = memoire.rappeler("alpha", "quel est le port ?", 5);
    assert_eq!(rappels.len(), 1, "{rappels:?}");
    assert_eq!(
        rappels[0].texte, "le port est 9090",
        "le rappel sert la correction"
    );

    // Le projet B ne peut ni rappeler ni corriger le fait.
    assert!(
        memoire.rappeler("beta", "quel est le port ?", 5).is_empty(),
        "aucun fait n'appartient à beta"
    );
    let verdict = memoire.reviser("beta", id_corrige, "le port est 1");
    assert!(matches!(verdict, Resultat::Refuse { .. }), "{verdict:?}");
    assert_eq!(
        memoire.rappeler("alpha", "quel est le port ?", 5)[0].texte,
        "le port est 9090",
        "la correction refusée n'a rien changé"
    );

    db.close().expect("fermeture");
    let _ = std::fs::remove_dir_all(&chemin);
}
