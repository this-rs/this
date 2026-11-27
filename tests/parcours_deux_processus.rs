//! Le VRAI parcours à deux processus : le parent lance deux fois le même binaire de test,
//! avec une phase explicite, et la référence traverse la frontière de processus par son
//! codec natif (`ReferenceTentative::to_bytes`/`from_bytes`) et un fichier — pas par un
//! objet mémoire partagé.
//!
//! Ce scénario qualifie un redémarrage PROPRE (fermeture + commit explicite + réouverture).
//! Il ne qualifie PAS une coupure de courant ni la reprise après réponse perdue.
#![cfg(feature = "obrain-cognitive")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use obrain_engine::ObrainDB;
use obrain_substrate::{IntegriteTentative, ReferenceTentative};
use this::memoire::{AdaptateurNatif, Memoire, Resultat, StockageFait};

fn repertoire() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "this-deux-proc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    base
}

fn memoire_sur(chemin: &Path) -> (ObrainDB, Memoire) {
    let db = ObrainDB::open(chemin).expect("ouverture de la base");
    let store = db.substrate_handle().expect("substrate handle");
    let adaptateur: Arc<dyn StockageFait> = Arc::new(AdaptateurNatif::nouveau(store));
    let memoire = Memoire::new(adaptateur);
    (db, memoire)
}

/// Processus A : écrit, corrige, persiste la référence et l'id, committe, ferme.
fn phase_a(chemin: &Path) {
    let (db, memoire) = memoire_sur(chemin);
    let r1 = memoire.memoriser("alpha", "le déploiement passe par Caddy");
    assert!(r1.visible(), "mémorisation 1 : {r1:?}");
    let Resultat::Ecrit { id, .. } = memoire.memoriser("alpha", "le port est 8080") else {
        panic!("mémorisation 2");
    };
    let Resultat::Ecrit { reference, .. } = memoire.reviser("alpha", id, "le port est 9090") else {
        panic!("correction");
    };
    let reference = reference.expect("la correction porte sa référence");

    // Persistance des références nécessaires à la reprise, via le codec natif.
    std::fs::write(chemin.join("reference.bin"), reference.to_bytes()).expect("écrire référence");
    std::fs::write(chemin.join("id.txt"), id.to_string()).expect("écrire id");

    // Synchronisation explicite AVEC propagation d'erreur (pas close().expect()).
    db.commit_wal().expect("commit_wal : durabilité du flush");
    db.close().expect("fermeture propre");
}

/// Processus B : recharge la référence depuis le fichier, résout, rappelle, isole.
fn phase_b(chemin: &Path) {
    let (db, memoire) = memoire_sur(chemin);

    // Rechargement depuis le fichier — aucun objet mémoire ne vient de A.
    let octets = std::fs::read(chemin.join("reference.bin")).expect("lire référence");
    let reference = ReferenceTentative::from_bytes(&octets).expect("décoder la référence");
    let resolution = memoire.resoudre(&reference).expect("résolution");
    assert_eq!(
        resolution.integrite(),
        IntegriteTentative::Integrale,
        "la correction est engagée"
    );

    let rappels = memoire
        .rappeler("alpha", "quel est le port ?", 5)
        .expect("rappel");
    assert_eq!(rappels.len(), 1, "{rappels:?}");
    assert_eq!(
        rappels[0].texte, "le port est 9090",
        "le rappel sert la correction"
    );

    // Isolation du projet : beta ne rappelle ni ne corrige.
    assert!(
        memoire
            .rappeler("beta", "quel est le port ?", 5)
            .expect("rappel beta")
            .is_empty()
    );
    let id: u64 = std::fs::read_to_string(chemin.join("id.txt"))
        .expect("lire id")
        .trim()
        .parse()
        .expect("id numérique");
    let verdict = memoire.reviser("beta", id, "le port est 1");
    assert!(matches!(verdict, Resultat::Refuse { .. }), "{verdict:?}");

    db.close().expect("fermeture");
}

#[test]
fn parcours_deux_processus_reels() {
    match std::env::var("MEMOIRE_PHASE").as_deref() {
        Ok("A") => phase_a(Path::new(
            &std::env::var("MEMOIRE_DIR").expect("MEMOIRE_DIR"),
        )),
        Ok("B") => phase_b(Path::new(
            &std::env::var("MEMOIRE_DIR").expect("MEMOIRE_DIR"),
        )),
        _ => {
            // Le parent orchestre deux processus distincts.
            let exe = std::env::current_exe().expect("current_exe");
            let chemin = repertoire();
            std::fs::create_dir_all(&chemin).expect("répertoire");

            let a = std::process::Command::new(&exe)
                .arg("parcours_deux_processus_reels")
                .arg("--nocapture")
                .env("MEMOIRE_PHASE", "A")
                .env("MEMOIRE_DIR", &chemin)
                .output()
                .expect("lancer A");
            assert!(
                a.status.success(),
                "processus A a échoué :\n{}",
                String::from_utf8_lossy(&a.stdout)
            );

            let b = std::process::Command::new(&exe)
                .arg("parcours_deux_processus_reels")
                .arg("--nocapture")
                .env("MEMOIRE_PHASE", "B")
                .env("MEMOIRE_DIR", &chemin)
                .output()
                .expect("lancer B");
            assert!(
                b.status.success(),
                "processus B a échoué :\n{}",
                String::from_utf8_lossy(&b.stdout)
            );

            let _ = std::fs::remove_dir_all(&chemin);
        }
    }
}
