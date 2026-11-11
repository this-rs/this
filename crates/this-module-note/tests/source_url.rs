//! Critère décisif : `source_url` n'a été ajouté qu'au descripteur.
use plan_store::Store;
use serde_json::{Value, json};
use this_module_sdk::{Module, Request, Response};

#[test]
fn source_url_optionnel_traverse_tout_sans_autre_changement() {
    let mut s = Store::new();
    let req = |v: Value| -> Request { serde_json::from_value(v).unwrap() };
    // Sans : toujours valide (optionnel), pas de clé écrite.
    s.run::<NoteM>(req(
        json!({"op":"create","entity":"note","input":{"note_type":"tip","content":"a"}}),
    ));
    // Avec : écrit, relu.
    let r = s.run::<NoteM>(req(json!({"op":"create","entity":"note",
        "input":{"note_type":"tip","content":"b","source_url":"https://exemple.org/x"}})));
    assert!(matches!(r, Response::Ok { .. }));
    let urls: Vec<_> = s
        .rows("note")
        .map(|r| r.get("source_url").cloned())
        .collect();
    assert!(urls.contains(&None) && urls.contains(&Some(json!("https://exemple.org/x"))));
    // L'écran le propose sans qu'on ait touché une vue.
    let m = NoteM::manifest();
    let form = m.views.iter().find(|v| v["name"] == "creer_note").unwrap();
    assert!(
        form["root"]["brick"]["fields"]
            .to_string()
            .contains("source_url")
    );
}
type NoteM = this_module_note::NoteModule;
