//! Sérialisation exacte, mot pour mot, de `docs/modules-wasm-abi.md` (§4-§6, §9).

use serde_json::json;
use this_module_sdk::*;

fn s<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap()
}

#[test]
fn response_ok_section_5() {
    let r = Response::ok_with(json!({"a":1}), vec![Effect::log(Level::Info, "x")]);
    assert_eq!(
        s(&r),
        r#"{"ok":true,"output":{"a":1},"effects":[{"kind":"log","level":"info","message":"x"}]}"#
    );
    assert_eq!(
        s(&Response::ok(json!({}))),
        r#"{"ok":true,"output":{},"effects":[]}"#
    );
}

#[test]
fn response_err_section_5() {
    let r = Response::field_err("invalid_title", "le titre est vide", "title");
    assert_eq!(
        s(&r),
        r#"{"ok":false,"error":{"code":"invalid_title","message":"le titre est vide","field":"title"}}"#
    );
    assert_eq!(
        s(&Response::err("c", "m")),
        r#"{"ok":false,"error":{"code":"c","message":"m"}}"#
    );
}

#[test]
fn effects_section_6() {
    assert_eq!(
        s(&Effect::put("task", json!({"id":"1"}))),
        r#"{"kind":"put","entity":"task","body":{"id":"1"}}"#
    );
    assert_eq!(
        s(&Effect::delete("task", "1")),
        r#"{"kind":"delete","entity":"task","id":"1"}"#
    );
    assert_eq!(
        s(&Effect::emit("t.done", json!({"n":2}))),
        r#"{"kind":"emit","topic":"t.done","payload":{"n":2}}"#
    );
    for (l, n) in [
        (Level::Info, "info"),
        (Level::Warn, "warn"),
        (Level::Error, "error"),
    ] {
        assert_eq!(
            s(&Effect::log(l, "m")),
            format!(r#"{{"kind":"log","level":"{n}","message":"m"}}"#)
        );
    }
}

#[test]
fn request_section_4_roundtrip() {
    let raw = r#"{"op":"call","name":"add_task","entity":"task","input":{"x":1},"rows":[{"id":"a"}],
        "ctx":{"now_ms":1762000000000,"tenant":"t","account":"u","locale":"fr","config":{"k":true}}}"#;
    let r: Request = serde_json::from_str(raw).unwrap();
    assert_eq!(r.op, Op::Call);
    assert_eq!(r.name.as_deref(), Some("add_task"));
    assert_eq!(r.ctx.now_ms, 1762000000000);
    assert_eq!(r.row("a"), Some(&json!({"id":"a"})));
    assert_eq!(r.config("k"), Some(&json!(true)));
    for op in ["create", "get", "list", "update", "delete", "call"] {
        let r: Request = serde_json::from_str(&format!(r#"{{"op":"{op}"}}"#)).unwrap();
        assert_eq!(serde_json::to_value(r.op).unwrap(), json!(op));
    }
}

#[test]
fn manifest_json_shape() {
    let m = Manifest::new("taches", "0.1.0")
        .entity(
            Entity::new("tache", "taches")
                .field(Field::string("titre").transform(Transform::trim()))
                .field(Field::bool("fait").default(json!(false)))
                .field(Field::int("priorite").optional()),
        )
        .operation(OperationDecl::new("terminer", "Terminer"));
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(
        v,
        json!({
          "format": 1, "name": "taches", "version": "0.1.0",
          "entities": [{
            "name": "tache", "plural": "taches", "kind": "data",
            "fields": [
              {"name":"titre","ty":"string","transforms":[{"rule":"trim"}]},
              {"name":"fait","ty":"bool","default":false},
              {"name":"priorite","ty":"i32","optional":true}
            ],
            "permissions": {"create":"authenticated","delete":"authenticated","get":"authenticated",
                            "list":"authenticated","update":"authenticated"}
          }],
          "operations": [{"name":"terminer","label":"Terminer","input":{},"output":{}}]
        })
    );
}

struct Echo;
impl Module for Echo {
    fn manifest() -> Manifest {
        Manifest::new("echo", "0.0.1")
    }
    fn handle(req: Request) -> Response {
        Response::ok(json!({"op": serde_json::to_value(req.op).unwrap()}))
    }
}

#[test]
fn dispatch_is_total() {
    let r = this_module_sdk::rt::run::<Echo>(b"create", br#"{"op":"get"}"#);
    assert_eq!(
        s(&r),
        r#"{"ok":true,"output":{"op":"create"},"effects":[]}"#
    );
    assert!(
        matches!(this_module_sdk::rt::run::<Echo>(b"create", b"pas du json"), Response::Err(e) if e.code == "bad_request")
    );
    assert!(
        matches!(this_module_sdk::rt::run::<Echo>(b"fly", b"{}"), Response::Err(e) if e.code == "bad_request")
    );
}
