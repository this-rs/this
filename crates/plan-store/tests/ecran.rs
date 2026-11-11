//! La démo de bout en bout, lue par l'écran : créer → voir dans la liste → modifier →
//! supprimer, chaque étape passant par le même descripteur (`ecran`) et le même
//! `Store` (`populate` → `handle` → `apply`). Rien n'est simulé : la liste est celle que
//! l'écran dérive du manifeste, pas une valeur écrite en dur.

use plan_store::Store;
use serde_json::{Value, json};
use this_descriptor::ModuleDescriptor;
use this_module_plan::{PlanModule, ecran};
use this_module_sdk::{Module, Request, Response};

fn req(v: Value) -> Request {
    serde_json::from_value(v).expect("requête valide")
}

fn descripteur() -> ModuleDescriptor {
    let json = serde_json::to_string(&PlanModule::manifest()).unwrap();
    ModuleDescriptor::from_json(&json).expect("manifeste relu par le vrai descripteur")
}

fn items(resp: Response) -> Vec<Value> {
    match resp {
        Response::Ok { output, .. } => output["items"].as_array().unwrap().clone(),
        Response::Err(e) => panic!("liste attendue : {e:?}"),
    }
}

#[test]
fn la_demo_alterne_store_et_ecran_sans_valeur_ecrite_en_dur() {
    let desc = descripteur();
    let mut store = Store::new();

    // 1. Créer une tâche : le module émet un `put`, le store l'applique.
    let cree = store.run::<PlanModule>(req(json!({
        "op": "create", "entity": "task",
        "input": {
            "plan": "plan-1", "title": "Écrire", "description": "rédiger la démo",
            "priority": 3, "due_date": "2026-11-15"
        }
    })));
    assert!(matches!(cree, Response::Ok { .. }), "{cree:?}");
    assert_eq!(store.len("task"), 1);

    // 2. La voir dans la liste, à travers l'écran dérivé du manifeste.
    let ecran = ecran::liste(&desc, "taches").expect("vue `taches`");
    assert_eq!(ecran.identite, "title");
    assert_eq!(
        ecran
            .colonnes
            .iter()
            .map(|c| c.champ.as_str())
            .collect::<Vec<_>>(),
        ["priority", "due_date"]
    );

    let lignes = ecran.lignes(&items(store.run::<PlanModule>(req(json!({
        "op": "list", "entity": "task"
    })))));
    assert_eq!(lignes.len(), 1);
    assert!(!lignes[0].id.is_empty(), "l'adaptateur a généré l'id");
    // [Texte(title), Montant(priority), Date(due_date)] — aligné sur `en_tete()`.
    assert_eq!(
        lignes[0].cellules[0],
        ecran::Cellule::Texte("Écrire".into())
    );
    assert_eq!(lignes[0].cellules[1], ecran::Cellule::Montant("3".into()));
    assert_eq!(
        lignes[0].cellules[2],
        ecran::Cellule::Date("2026-11-15".into())
    );

    // 3. Modifier : la ligne existante est fournie par l'adaptateur, le module fusionne.
    let id = lignes[0].id.clone();
    let modifie = store.run::<PlanModule>(req(json!({
        "op": "update", "entity": "task", "input": {"id": id, "priority": 7}
    })));
    assert!(matches!(modifie, Response::Ok { .. }), "{modifie:?}");
    assert_eq!(store.row("task", &id).unwrap()["priority"], 7);
    assert_eq!(store.row("task", &id).unwrap()["title"], "Écrire");

    // La liste relue montre la priorité modifiée, le reste intact.
    let lignes = ecran.lignes(&items(store.run::<PlanModule>(req(json!({
        "op": "list", "entity": "task"
    })))));
    assert_eq!(lignes[0].cellules[1], ecran::Cellule::Montant("7".into()));
    assert_eq!(
        lignes[0].cellules[0],
        ecran::Cellule::Texte("Écrire".into())
    );

    // 4. Supprimer : un effet `delete`, la ligne disparaît de la liste.
    let supprime = store.run::<PlanModule>(req(json!({
        "op": "delete", "entity": "task", "input": {"id": id}
    })));
    assert!(matches!(supprime, Response::Ok { .. }), "{supprime:?}");
    assert_eq!(store.len("task"), 0);
    assert!(
        ecran
            .lignes(&items(store.run::<PlanModule>(req(json!({
                "op": "list", "entity": "task"
            })))))
            .is_empty()
    );
}
