//! Le module d'exemple CALCULE son arbre : deux entrées, deux arbres ; chacun passe le contrat.

use serde_json::{Value, json};
use this_descriptor::views::ViewDesc;
use this_descriptor::{CheckProfile, CustomRegistry, ModuleDescriptor};
use this_module_compteur::{Compteur, rendre};
use this_module_sdk::Module;
use this_module_sdk::rt;

fn rendu(req: Value) -> Value {
    let octets = rt::render_bytes(req.to_string().as_bytes(), rendre);
    serde_json::from_slice(&octets).unwrap()
}

/// Le manifeste du module + les vues rendues, passés au `check(Stored)` du framework : l'entité,
/// les champs, les actions que l'arbre nomme existent — c'est la validation du contrat.
fn valide(rendu: &Value) -> Vec<String> {
    let mut manifeste = serde_json::to_value(Compteur::manifest()).unwrap();
    manifeste["views"] = rendu.clone();
    let d = ModuleDescriptor::from_json(&manifeste.to_string()).expect("relu par le contrat");
    let _: Vec<ViewDesc> = serde_json::from_value(rendu.clone()).unwrap();
    match d.check(
        &CustomRegistry::default(),
        CheckProfile::Stored {
            allow_public: false,
        },
    ) {
        Ok(_) => vec![],
        Err(e) => vec![format!("{e:?}")],
    }
}

#[test]
fn deux_entrees_donnent_deux_arbres_differents() {
    let zero = rendu(json!({"ouvertes": 0}));
    let trois = rendu(json!({"ouvertes": 3, "filtre": "a_faire"}));
    let faites = rendu(json!({"ouvertes": 3, "filtre": "faites"}));
    let beaucoup = rendu(json!({"ouvertes": 42}));
    assert_ne!(zero, trois);
    assert_ne!(trois, faites);
    assert_ne!(trois, beaucoup);

    // La branche qui dépend d'une valeur : le texte porte le compte…
    assert!(trois.to_string().contains("3 à faire"));
    assert!(beaucoup.to_string().contains("42 tâches ouvertes"));
    // …le ton et la brique changent avec l'état…
    assert!(zero.to_string().contains(r#""tone":"success""#));
    assert!(zero.to_string().contains(r#""type":"form""#));
    assert!(beaucoup.to_string().contains(r#""tone":"error""#));
    assert!(trois.to_string().contains(r#""type":"list""#));
    // …et le bouton « terminer » n'existe que pour les tâches ouvertes.
    assert!(trois.to_string().contains(r#""type":"button""#));
    assert!(!faites.to_string().contains(r#""type":"button""#));
}

#[test]
fn chaque_arbre_passe_la_validation_du_contrat() {
    for req in [
        json!({"ouvertes": 0}),
        json!({"ouvertes": 3}),
        json!({"ouvertes": 3, "filtre": "faites"}),
        json!({"ouvertes": 10}),
        json!({}),
        Value::Null,
    ] {
        let r = rendu(req.clone());
        assert_eq!(r[0]["name"], "accueil", "{req}");
        assert_eq!(valide(&r), Vec::<String>::new(), "{req}");
    }
}

#[test]
fn la_validation_du_contrat_refuse_bien_un_arbre_fautif() {
    // Garde-fou du test précédent : il ne passe pas par défaut. Une action fantôme est refusée.
    let mut r = rendu(json!({"ouvertes": 3}));
    r[0]["root"]["children"][1]["tabs"][0]["children"][0]["children"][1]["brick"]["action"] =
        json!("fantome");
    assert!(!valide(&r).is_empty());
}
