//! Preuve de bout en bout : `handle(Request)` → effets → adaptateur → relecture →
//! `handle(Request)` rend la même chose. Le module ne voit jamais le stockage.

use plan_store::Store;
use serde_json::{Value, json};
use this_module_plan::PlanModule;
use this_module_sdk::{Effect, Module, Request, Response};

fn req(v: Value) -> Request {
    serde_json::from_value(v).expect("requête valide")
}

/// Le cycle exact de la mission, déroulé à la main (sans `Store::run`) pour rendre la
/// frontière visible : le module n'est appelé qu'avec des `Request` dont `rows` a été
/// peuplé par l'adaptateur, et ses `Effect`s ne sont appliqués que par l'adaptateur.
#[test]
fn aller_retour_create_list_get_update_delete() {
    let mut store = Store::new();

    // 1. create : le module émet un effet `put`, ne touche aucune base.
    let resp = PlanModule::handle(req(json!({
        "op": "create", "entity": "plan",
        "input": {"title": "Parité", "description": "un plan"}
    })));
    let Response::Ok { effects, .. } = &resp else {
        panic!("create a échoué : {resp:?}")
    };
    assert_eq!(effects.len(), 1);
    assert!(matches!(&effects[0], Effect::Put { entity, .. } if entity == "plan"));

    // 2. L'adaptateur écrit l'effet.
    store.apply(&resp);
    assert_eq!(store.len("plan"), 1);

    // 3. Relecture : l'adaptateur peuple `rows`, le module rend la même chose.
    let list = PlanModule::handle(store.populate(req(json!({"op": "list", "entity": "plan"}))));
    let Response::Ok { output, .. } = &list else {
        panic!("list a échoué : {list:?}")
    };
    let items = output["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Parité");
    assert_eq!(items[0]["description"], "un plan");
    assert_eq!(items[0]["status"], "draft"); // défaut appliqué par le module
    assert_eq!(items[0]["priority"], 0); // défaut appliqué par le module
    let id = items[0]["id"].as_str().unwrap().to_owned();
    assert!(!id.is_empty(), "l'adaptateur a généré l'id");

    // 4. get : même ligne, retrouvée par l'id.
    let get = PlanModule::handle(store.populate(req(json!({
        "op": "get", "entity": "plan", "input": {"id": id}
    }))));
    let Response::Ok { output, .. } = &get else {
        panic!("get a échoué : {get:?}")
    };
    assert_eq!(output["title"], "Parité");

    // 5. update : la ligne existante est fournie par l'adaptateur, le module fusionne.
    let upd = PlanModule::handle(store.populate(req(json!({
        "op": "update", "entity": "plan",
        "input": {"id": id, "priority": 95}
    }))));
    store.apply(&upd);
    assert_eq!(store.row("plan", &id).unwrap()["priority"], 95);
    assert_eq!(store.row("plan", &id).unwrap()["title"], "Parité");

    // 6. delete : un effet `delete`, la ligne disparaît.
    let del = PlanModule::handle(req(json!({
        "op": "delete", "entity": "plan", "input": {"id": id}
    })));
    store.apply(&del);
    assert_eq!(store.len("plan"), 0);
}

/// Les cinq entités passent par le *même* adaptateur, sans un seul branchement par nom :
/// la preuve que l'adaptateur ne connaît pas le domaine.
#[test]
fn les_cinq_entites_passent_par_le_meme_adaptateur() {
    let mut store = Store::new();
    let cas: [(&str, Value); 5] = [
        ("plan", json!({"title": "p", "description": "d"})),
        (
            "task",
            json!({"plan": "plan-1", "title": "t", "description": "d"}),
        ),
        ("step", json!({"task": "task-1", "description": "d"})),
        (
            "decision",
            json!({"task": "task-1", "description": "d", "rationale": "r", "alternatives": ["a"]}),
        ),
        (
            "constraint",
            json!({"plan": "plan-1", "constraint_type": "security", "description": "d"}),
        ),
    ];
    for (entity, input) in cas {
        let resp = PlanModule::handle(req(json!({
            "op": "create", "entity": entity, "input": input
        })));
        assert!(
            matches!(&resp, Response::Ok { .. }),
            "create {entity} a échoué : {resp:?}"
        );
        store.apply(&resp);
        assert_eq!(
            store.len(entity),
            1,
            "l'entité {entity} n'a pas été stockée"
        );
    }
}
