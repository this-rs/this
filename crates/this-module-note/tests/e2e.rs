//! Bout en bout : `handle` → effets → `plan-store::Store` (NON modifié) → relecture.
//! `plan-store` n'a jamais vu `note` ni `rfc`.

use plan_store::Store;
use serde_json::{Value, json};
use this_descriptor::{CheckProfile, CustomRegistry, ModuleDescriptor};
use this_module_note::NoteModule;
use this_module_sdk::{Module, Request, Response};

fn req(v: Value) -> Request {
    serde_json::from_value(v).expect("requête valide")
}

fn ok(r: Response) -> Value {
    match r {
        Response::Ok { output, .. } => output,
        other => panic!("ok attendu : {other:?}"),
    }
}

#[test]
fn le_manifeste_passe_check_stored() {
    let json = serde_json::to_string(&NoteModule::manifest()).unwrap();
    let d = ModuleDescriptor::from_json(&json).expect("relu par le vrai descripteur");
    d.check(
        &CustomRegistry::default(),
        CheckProfile::Stored {
            allow_public: false,
        },
    )
    .unwrap_or_else(|e| panic!("check(Stored) : {e:?}"));
    assert_eq!(d.entities.len(), 2);
}

#[test]
fn note_aller_retour_complet() {
    let mut s = Store::new();
    let contenu = "x".repeat(5000); // contenu long
    let out = ok(s.run::<NoteModule>(req(json!({
        "op": "create", "entity": "note",
        "input": {"note_type": "gotcha", "content": contenu, "importance": "critical",
                  "tags": ["a", "b"], "confidence": 0.9, "valid_until": "2027-01-01T00:00:00Z"}
    }))));
    assert_eq!(out["status"], "active"); // défaut du descripteur
    assert_eq!(out["energy"], 1.0);
    assert_eq!(s.len("note"), 1);

    let id = s.rows("note").next().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // get : les types inédits (Text long, liste, f64, datetime, enum) traversent intacts.
    let got = ok(s.run::<NoteModule>(req(
        json!({"op": "get", "entity": "note", "input": {"id": id}}),
    )));
    assert_eq!(got["content"].as_str().unwrap().len(), 5000);
    assert_eq!(got["tags"], json!(["a", "b"]));
    assert_eq!(got["confidence"], 0.9);
    assert_eq!(got["valid_until"], "2027-01-01T00:00:00Z");
    assert_eq!(got["note_type"], "gotcha");

    // update : transition de cycle de vie + relation de succession (auto-référence).
    ok(s.run::<NoteModule>(req(json!({
        "op": "update", "entity": "note",
        "input": {"id": id, "status": "stale", "superseded_by": "note-2"}
    }))));
    let row = s.row("note", &id).unwrap();
    assert_eq!(row["status"], "stale");
    assert_eq!(row["superseded_by"], "note-2");
    assert_eq!(row["content"].as_str().unwrap().len(), 5000);

    // delete
    ok(s.run::<NoteModule>(req(
        json!({"op": "delete", "entity": "note", "input": {"id": id}}),
    )));
    assert_eq!(s.len("note"), 0);
}

#[test]
fn rfc_aller_retour_complet_avec_relation_vers_note() {
    let mut s = Store::new();
    ok(s.run::<NoteModule>(req(json!({
        "op": "create", "entity": "note",
        "input": {"note_type": "rfc", "content": "texte de la RFC"}
    }))));
    let note_id = s.rows("note").next().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    ok(s.run::<NoteModule>(req(json!({
        "op": "create", "entity": "rfc",
        "input": {"note": note_id, "title": "Store générique", "problem": "p",
                  "proposed_solution": "s", "alternatives": "a", "impact": "i"}
    }))));
    let rid = s.rows("rfc").next().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(s.row("rfc", &rid).unwrap()["status"], "draft");
    assert_eq!(s.row("rfc", &rid).unwrap()["note"], json!(note_id));

    // draft → proposed → accepted → implemented
    for etat in ["proposed", "accepted", "implemented"] {
        ok(s.run::<NoteModule>(req(json!({
            "op": "update", "entity": "rfc", "input": {"id": rid, "status": etat}
        }))));
    }
    let lu = ok(s.run::<NoteModule>(req(
        json!({"op": "get", "entity": "rfc", "input": {"id": rid}}),
    )));
    assert_eq!(lu["status"], "implemented");
    assert_eq!(lu["title"], "Store générique");
    assert_eq!(s.len("rfc"), 1);
}

#[test]
fn une_variante_hors_vocabulaire_est_refusee_sans_effet() {
    let mut s = Store::new();
    let r = s.run::<NoteModule>(req(json!({
        "op": "create", "entity": "note",
        "input": {"note_type": "poeme", "content": "c"}
    })));
    assert!(matches!(r, Response::Err { .. }), "{r:?}");
    assert_eq!(s.len("note"), 0);
}

#[test]
fn la_liste_trie_l_importance_dans_l_ordre_des_variantes() {
    let mut s = Store::new();
    for imp in ["high", "low", "critical", "medium"] {
        ok(s.run::<NoteModule>(req(json!({
            "op": "create", "entity": "note",
            "input": {"note_type": "tip", "content": imp, "importance": imp}
        }))));
    }
    let out = ok(s.run::<NoteModule>(req(json!({"op": "list", "entity": "note"}))));
    let ordre: Vec<&str> = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["importance"].as_str().unwrap())
        .collect();
    assert_eq!(ordre, ["low", "medium", "high", "critical"]);
}

#[test]
fn l_ecran_est_derive_des_champs_du_descripteur() {
    let m = NoteModule::manifest();
    let form = m.views.iter().find(|v| v["name"] == "creer_note").unwrap();
    let champs: Vec<&str> = form["root"]["brick"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field"].as_str().unwrap())
        .collect();
    // saisissables : texte / enum / liste ; pas de compteur, score, date ni relation.
    for attendu in ["note_type", "content", "importance", "tags", "created_by"] {
        assert!(champs.contains(&attendu), "{attendu} absent de {champs:?}");
    }
    for exclu in ["energy", "valid_from", "supersedes", "status"] {
        assert!(!champs.contains(&exclu), "{exclu} ne doit pas être saisi");
    }
    let liste = m.views.iter().find(|v| v["name"] == "notes").unwrap();
    assert_eq!(
        liste["root"]["brick"]["sort"],
        json!([{"field": "importance", "dir": "asc"}])
    );
}
