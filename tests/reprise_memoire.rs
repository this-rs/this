//! La conservation du contexte à travers une issue incertaine APRÈS mutation native.
//!
//! La mutation réussit réellement (référence native produite), puis la relecture échoue de
//! façon contrôlée (erreur de lecture OU refus) : le service doit rendre `AConfirmer` EN
//! CONSERVANT l'identifiant et la référence, que le consommateur peut résoudre sans republier.
#![cfg(feature = "obrain-cognitive")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use obrain_engine::ObrainDB;
use obrain_substrate::{IntegriteTentative, ReferenceTentative, ResolutionTentative};
use this::memoire::{AdaptateurNatif, ErreurStockage, FaitPublie, Memoire, Resultat, StockageFait};

/// Un décorateur qui fait échouer la relecture après que la mutation réelle a réussi.
struct RelectureEnPanne {
    inner: Arc<dyn StockageFait>,
    en_panne: AtomicBool,
    refus: AtomicBool,
}

impl StockageFait for RelectureEnPanne {
    fn creer(&self, projet: &str, texte: &str) -> Result<FaitPublie, ErreurStockage> {
        self.inner.creer(projet, texte)
    }
    fn corriger(
        &self,
        id: u64,
        projet_attendu: &str,
        texte: &str,
    ) -> Result<FaitPublie, ErreurStockage> {
        self.inner.corriger(id, projet_attendu, texte)
    }
    fn lire(&self, id: u64) -> Result<Option<(String, String)>, ErreurStockage> {
        if self.refus.load(Ordering::Relaxed) {
            return Err(ErreurStockage::Refus("refus en relecture".to_string()));
        }
        if self.en_panne.load(Ordering::Relaxed) {
            return Err(ErreurStockage::AConfirmer {
                raison: "lecture impossible".to_string(),
                id: Some(id),
                reference: None,
            });
        }
        self.inner.lire(id)
    }
    fn lister(&self) -> Result<Vec<(u64, String, String)>, ErreurStockage> {
        self.inner.lister()
    }
    fn resoudre(
        &self,
        reference: &ReferenceTentative,
    ) -> Result<ResolutionTentative, ErreurStockage> {
        self.inner.resoudre(reference)
    }
}

fn repertoire() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "this-reprise-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    base
}

fn nouvel_adaptateur(chemin: &std::path::Path) -> (ObrainDB, Arc<RelectureEnPanne>, Memoire) {
    let db = ObrainDB::open(chemin).expect("open");
    let store = db.substrate_handle().expect("substrate");
    let adaptateur: Arc<dyn StockageFait> = Arc::new(AdaptateurNatif::nouveau(store));
    let en_panne = Arc::new(RelectureEnPanne {
        inner: adaptateur,
        en_panne: AtomicBool::new(false),
        refus: AtomicBool::new(false),
    });
    let memoire = Memoire::new(en_panne.clone());
    (db, en_panne, memoire)
}

#[test]
fn la_reference_survit_a_une_relecture_en_echec_apres_mutation() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire");
    let (db, en_panne, memoire) = nouvel_adaptateur(&chemin);

    let Resultat::Ecrit { id, .. } = memoire.memoriser("alpha", "le port est 8080") else {
        panic!("mémorisation");
    };

    en_panne.en_panne.store(true, Ordering::Relaxed);
    let verdict = memoire.reviser("alpha", id, "le port est 9090");
    match verdict {
        Resultat::AConfirmer {
            id: fid, reference, ..
        } => {
            assert_eq!(fid, Some(id), "identifiant conservé");
            let reference = reference.expect("référence conservée");
            let resolution = memoire.resoudre(&reference).expect("résolution");
            assert_eq!(resolution.integrite(), IntegriteTentative::Integrale);
        }
        autre => panic!("attendu AConfirmer, mesuré {autre:?}"),
    }

    en_panne.en_panne.store(false, Ordering::Relaxed);
    assert_eq!(
        memoire
            .rappeler("alpha", "quel est le port ?", 5)
            .expect("rappel")[0]
            .texte,
        "le port est 9090"
    );

    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&chemin);
}

#[test]
fn un_refus_pendant_la_relecture_apres_mutation_conserve_le_contexte() {
    let chemin = repertoire();
    std::fs::create_dir_all(&chemin).expect("répertoire");
    let (db, en_panne, memoire) = nouvel_adaptateur(&chemin);

    let Resultat::Ecrit { id, .. } = memoire.memoriser("alpha", "le port est 8080") else {
        panic!("mémorisation");
    };

    // Mutation réussie, relecture rend un Refus : ce n'est PAS un Refuse (une mutation a été tentée).
    en_panne.refus.store(true, Ordering::Relaxed);
    let verdict = memoire.reviser("alpha", id, "le port est 9090");
    match verdict {
        Resultat::AConfirmer {
            id: fid, reference, ..
        } => {
            assert_eq!(fid, Some(id), "identifiant conservé");
            let reference = reference.expect("référence conservée");
            let resolution = memoire.resoudre(&reference).expect("résolution");
            assert_eq!(resolution.integrite(), IntegriteTentative::Integrale);
        }
        autre => panic!("attendu AConfirmer (pas Refuse), mesuré {autre:?}"),
    }

    en_panne.refus.store(false, Ordering::Relaxed);
    assert_eq!(
        memoire
            .rappeler("alpha", "quel est le port ?", 5)
            .expect("rappel")[0]
            .texte,
        "le port est 9090"
    );

    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&chemin);
}
