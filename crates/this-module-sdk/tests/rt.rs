//! Le moteur d'exécution de `export!` : `run` (le cœur testable hors wasm), et l'empaquetage
//! `(ptr << 32) | len` de `handle` et `manifest`.
//!
//! Hors wasm32 un pointeur ne tient pas dans un `i32` : on ne déréférence donc JAMAIS ici un
//! pointeur rendu par `alloc` ; on vérifie la moitié basse du résultat empaqueté, la longueur,
//! qui est exacte sur toute plateforme.

use serde_json::json;
use this_module_sdk::rt;
use this_module_sdk::*;

struct Echo;

impl Module for Echo {
    fn manifest() -> Manifest {
        Manifest::new("echo", "0.1.0")
            .entity(Entity::new("note", "notes").field(Field::string("title")))
    }
    fn handle(req: Request) -> Response {
        Response::ok(json!({
            "op": req.op,
            "name": req.name,
            "entity": req.entity,
            "input": req.input,
            "rows": req.rows.len(),
            "now_ms": req.ctx.now_ms,
            "locale": req.ctx.locale,
        }))
    }
}

fn output(r: Response) -> Value {
    match r {
        Response::Ok { output, .. } => output,
        Response::Err(e) => panic!("attendu un succès, reçu {e:?}"),
    }
}

fn error(r: Response) -> Error {
    match r {
        Response::Err(e) => e,
        Response::Ok { output, .. } => panic!("attendu un échec, reçu {output}"),
    }
}

#[test]
fn run_dispatches_a_full_request_to_the_module() {
    let req = br#"{
        "op": "call", "name": "archiver", "entity": "note",
        "input": {"id": "n1"},
        "rows": [{"id": "n1"}, {"id": "n2"}],
        "ctx": {"now_ms": 1234, "locale": "fr"}
    }"#;
    let out = output(rt::run::<Echo>(b"", req));
    assert_eq!(
        out,
        json!({
            "op": "call", "name": "archiver", "entity": "note",
            "input": {"id": "n1"}, "rows": 2, "now_ms": 1234, "locale": "fr"
        })
    );
}

#[test]
fn run_fills_the_defaults_of_a_minimal_request() {
    let out = output(rt::run::<Echo>(b"", br#"{"op":"list"}"#));
    assert_eq!(
        out,
        json!({
            "op": "list", "name": null, "entity": null,
            "input": null, "rows": 0, "now_ms": 0, "locale": ""
        })
    );
}

#[test]
fn run_rejects_unreadable_json() {
    let e = error(rt::run::<Echo>(b"get", b"{pas du json"));
    assert_eq!(e.code, "bad_request");
    assert!(
        e.message.starts_with("requête illisible : "),
        "{}",
        e.message
    );
    assert_eq!(e.field, None);
}

#[test]
fn run_rejects_empty_input_as_unreadable() {
    let e = error(rt::run::<Echo>(b"", b""));
    assert_eq!(e.code, "bad_request");
    assert!(
        e.message.starts_with("requête illisible : "),
        "{}",
        e.message
    );
}

#[test]
fn run_rejects_an_unknown_op_argument() {
    let e = error(rt::run::<Echo>(b"frobnicate", br#"{"op":"get"}"#));
    assert_eq!(e.code, "bad_request");
    assert_eq!(e.message, "opération inconnue : frobnicate");
}

#[test]
fn the_op_argument_overrides_the_op_field_of_the_body() {
    let out = output(rt::run::<Echo>(b"create", br#"{"op":"get"}"#));
    assert_eq!(out["op"], "create");
    // Même sans `op` dans le corps, l'argument suffit.
    let out = output(rt::run::<Echo>(b"delete", br#"{"input":{"k":1}}"#));
    assert_eq!(out["op"], "delete");
    assert_eq!(out["input"], json!({"k": 1}));
}

#[test]
fn an_empty_or_non_utf8_op_argument_falls_back_to_the_body() {
    let out = output(rt::run::<Echo>(b"", br#"{"op":"update"}"#));
    assert_eq!(out["op"], "update");
    let out = output(rt::run::<Echo>(&[0xff, 0xfe], br#"{"op":"update"}"#));
    assert_eq!(out["op"], "update");
}

#[test]
fn run_rejects_a_body_that_is_not_a_request() {
    // Pas d'`op` : le corps est du JSON valide mais pas une requête.
    let e = error(rt::run::<Echo>(b"", br#"{"entity":"note"}"#));
    assert_eq!(e.code, "bad_request");
    assert!(
        e.message.starts_with("requête invalide : "),
        "{}",
        e.message
    );
    assert!(e.message.contains("op"), "{}", e.message);
    // Un tableau n'est pas un objet : l'argument `op` ne peut pas s'y insérer.
    let e = error(rt::run::<Echo>(b"get", b"[1,2]"));
    assert_eq!(e.code, "bad_request");
    assert!(
        e.message.starts_with("requête invalide : "),
        "{}",
        e.message
    );
    // Un `op` du corps hors vocabulaire est refusé par la désérialisation.
    let e = error(rt::run::<Echo>(b"", br#"{"op":"frobnicate"}"#));
    assert_eq!(e.code, "bad_request");
    assert!(
        e.message.starts_with("requête invalide : "),
        "{}",
        e.message
    );
}

#[test]
fn alloc_gives_a_non_null_block_even_for_zero_or_negative_sizes() {
    // Un bloc de taille nulle est un pointeur « pendant » non nul et aligné (jamais lu).
    assert_ne!(rt::alloc(0), 0);
    assert_ne!(rt::alloc(-12), 0);
    assert_eq!(rt::alloc(0), rt::alloc(-1), "négatif = taille nulle");
}

fn packed_len(packed: i64) -> usize {
    (packed as u64 & 0xFFFF_FFFF) as usize
}

#[test]
fn manifest_packs_the_exact_length_of_the_serialized_manifest() {
    let expected = serde_json::to_vec(&Echo::manifest()).unwrap();
    assert_eq!(packed_len(rt::manifest::<Echo>()), expected.len());
}

#[test]
fn handle_packs_the_length_of_the_serialized_response() {
    // Entrées vides (longueur 0, aucun déréférencement) : la requête est illisible, et la
    // réponse d'échec est empaquetée avec sa longueur exacte.
    let packed = rt::handle::<Echo>(0, 0, 0, 0);
    let expected = serde_json::to_vec(&rt::run::<Echo>(b"", b"")).unwrap();
    assert_eq!(packed_len(packed), expected.len());
    // Des longueurs négatives valent des tampons vides, comme zéro.
    let packed = rt::handle::<Echo>(0, -1, 0, -7);
    assert_eq!(packed_len(packed), expected.len());
}

#[test]
fn a_second_call_replaces_the_previous_output() {
    let first = packed_len(rt::manifest::<Echo>());
    let second = packed_len(rt::handle::<Echo>(0, 0, 0, 0));
    assert_ne!(first, second, "deux réponses de tailles différentes");
    // Rappeler `manifest` redonne la même longueur : la sortie est reconstruite, pas cumulée.
    assert_eq!(packed_len(rt::manifest::<Echo>()), first);
}
