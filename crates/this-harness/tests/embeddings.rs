//! Le port Embed : sans implémentation rien ne tourne, la dimension est
//! comparée et nommée, l'erreur est propagée, le lexique reste.
use std::cell::Cell;
use std::rc::Rc;

use futures::executor::block_on;
use this_harness::{
    Embed, EmbedError, EmbeddingModel, SansEmbed, vecteur_dense, vectoriser, verifier,
};

/// Un double : estampille configurable, compteur d'appels, résultat scripté.
struct Double {
    modele: Option<EmbeddingModel>,
    appels: Rc<Cell<usize>>,
    resultat: Result<Vec<Vec<f32>>, EmbedError>,
}

#[async_trait::async_trait(?Send)]
impl Embed for Double {
    fn modele(&self) -> Option<&EmbeddingModel> {
        self.modele.as_ref()
    }

    async fn embed(&self, _textes: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.appels.set(self.appels.get() + 1);
        self.resultat.clone()
    }
}

fn taille(n: usize) -> Vec<f32> {
    vec![0.0; n]
}

#[test]
fn sans_implementation_embed_n_est_jamais_appele() {
    let appels = Rc::new(Cell::new(0));
    let double = Double {
        modele: None,
        appels: Rc::clone(&appels),
        resultat: Ok(vec![taille(3)]),
    };
    let r = block_on(vectoriser(&double, &["a".into()]));
    assert_eq!(r, Err(EmbedError::Unsupported));
    assert_eq!(
        appels.get(),
        0,
        "sans estampille, embed ne doit pas être appelé"
    );
}

#[test]
fn une_mauvaise_dimension_est_refusee_nomment() {
    let double = Double {
        modele: Some(EmbeddingModel {
            nom: "bge-m3".into(),
            dimension: 384,
        }),
        appels: Rc::new(Cell::new(0)),
        resultat: Ok(vec![taille(256)]),
    };
    let r = block_on(vectoriser(&double, &["a".into()]));
    match r {
        Err(EmbedError::Dimension { attendue, recue }) => {
            assert_eq!((attendue, recue), (384, 256));
        }
        other => panic!("attendu un refus de dimension, reçu {other:?}"),
    }
}

#[test]
fn le_refus_de_dimension_dit_les_deux_dimensions() {
    let dit = EmbedError::Dimension {
        attendue: 384,
        recue: 256,
    }
    .to_string();
    assert!(
        dit.contains("384") && dit.contains("256"),
        "le refus doit nommer les deux dimensions : {dit}"
    );
}

#[test]
fn une_erreur_est_propagee_pas_avalee() {
    let double = Double {
        modele: Some(EmbeddingModel {
            nom: "bge-m3".into(),
            dimension: 384,
        }),
        appels: Rc::new(Cell::new(0)),
        resultat: Err(EmbedError::Failed("le moteur ne répond pas".into())),
    };
    let r = block_on(vectoriser(&double, &["a".into()]));
    match r {
        Err(EmbedError::Failed(raison)) => assert!(raison.contains("ne répond pas"), "{raison}"),
        other => panic!("attendu la propagation de l'erreur, reçu {other:?}"),
    }
}

#[test]
fn le_defaut_ne_fait_rien() {
    assert!(SansEmbed.modele().is_none());
    assert_eq!(
        block_on(SansEmbed.embed(&["a".into()])),
        Err(EmbedError::Unsupported)
    );
}

#[test]
fn le_lexique_reste_quand_embed_n_est_pas_branche() {
    // Sans branchement, vecteur_dense rend (None, None) : le consommateur garde
    // le lexique seul, sans appel réseau et sans rien écrire au journal.
    let (vecteur, journal) = block_on(vecteur_dense(&SansEmbed, "requête"));
    assert!(vecteur.is_none());
    assert!(
        journal.is_none(),
        "pas branché : pas de raison à journaliser"
    );
}

#[test]
fn un_vecteur_de_bonne_dimension_passe() {
    let double = Double {
        modele: Some(EmbeddingModel {
            nom: "bge-m3".into(),
            dimension: 3,
        }),
        appels: Rc::new(Cell::new(0)),
        resultat: Ok(vec![taille(3), taille(3)]),
    };
    let r = block_on(vectoriser(&double, &["a".into(), "b".into()]));
    assert_eq!(r.unwrap().len(), 2, "un vecteur par texte, dans l'ordre");
}

// ---- la règle de dimension, seule : une fonction pure, donc appelable par un
//      hôte dont le futur doit être `Send` (coquille native), sans le port.

fn estampille(dimension: usize) -> EmbeddingModel {
    EmbeddingModel {
        nom: "bge-m3".into(),
        dimension,
    }
}

#[test]
fn verifier_accepte_un_lot_conforme() {
    let modele = estampille(384);
    let lot = vec![taille(384), taille(384), taille(384)];
    assert_eq!(verifier(&modele, &lot), Ok(()), "un lot conforme passe");
    assert_eq!(
        verifier(&modele, &[]),
        Ok(()),
        "un lot vide ne contredit rien : il passe"
    );
}

#[test]
fn verifier_refuse_un_seul_vecteur_fautif_meme_pas_le_premier() {
    // Le fautif est au TROISIÈME rang : un contrôle qui ne regarderait que le
    // premier (ou le dernier) laisserait passer ce lot, et le cosinus entre
    // deux espaces serait calculé en silence.
    let modele = estampille(384);
    let lot = vec![taille(384), taille(384), taille(256), taille(384)];
    match verifier(&modele, &lot) {
        Err(EmbedError::Dimension { attendue, recue }) => {
            assert_eq!((attendue, recue), (384, 256), "les deux dimensions");
        }
        other => panic!("attendu un refus de dimension, reçu {other:?}"),
    }
}

#[test]
fn le_refus_de_verifier_nomme_les_deux_dimensions() {
    let dit = match verifier(&estampille(384), &[taille(384), taille(256)]) {
        Err(raison) => raison.to_string(),
        Ok(()) => panic!("un vecteur de 256 dans un espace de 384 doit être refusé"),
    };
    assert!(
        dit.contains("384") && dit.contains("256"),
        "le refus doit nommer les deux dimensions : {dit}"
    );
}

#[test]
fn vectoriser_applique_la_regle_du_lot_entier() {
    // `vectoriser` n'a pas sa propre copie de la règle : un lot dont le second
    // vecteur est fautif est refusé par `verifier`, pas accepté par omission.
    let double = Double {
        modele: Some(estampille(384)),
        appels: Rc::new(Cell::new(0)),
        resultat: Ok(vec![taille(384), taille(256)]),
    };
    let r = block_on(vectoriser(&double, &["a".into(), "b".into()]));
    assert_eq!(
        r,
        Err(EmbedError::Dimension {
            attendue: 384,
            recue: 256
        })
    );
}
