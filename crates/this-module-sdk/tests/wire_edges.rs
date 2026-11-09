//! Les bords du fil : défauts, valeurs inconnues, mauvais types, `null`, aller-retour.

use serde_json::{Value, json};
use this_module_sdk::*;

fn de<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, String> {
    serde_json::from_str(raw).map_err(|e| e.to_string())
}

#[test]
fn op_parse_knows_the_six_verbs_and_nothing_else() {
    let all = [
        ("create", Op::Create),
        ("get", Op::Get),
        ("list", Op::List),
        ("update", Op::Update),
        ("delete", Op::Delete),
        ("call", Op::Call),
    ];
    for (s, op) in all {
        assert_eq!(Op::parse(s), Some(op));
    }
    for bad in ["", "Create", "CALL", " get", "frobnicate"] {
        assert_eq!(Op::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn a_minimal_request_takes_every_default() {
    let r: Request = de(r#"{"op":"list"}"#).unwrap();
    assert_eq!(r.op, Op::List);
    assert_eq!(r.name, None);
    assert_eq!(r.entity, None);
    assert_eq!(r.input, Value::Null);
    assert!(r.rows.is_empty());
    assert_eq!(r.ctx, Ctx::default());
    assert_eq!(r.ctx.now_ms, 0);
    assert_eq!(r.ctx.tenant, None);
    assert_eq!(r.ctx.account, None);
    assert_eq!(r.ctx.locale, "");
    assert_eq!(r.ctx.config, Value::Null);
    assert!(!r.ctx.truncated);
}

#[test]
fn null_tenant_and_account_are_normal() {
    let r: Request =
        de(r#"{"op":"get","ctx":{"tenant":null,"account":null,"truncated":true}}"#).unwrap();
    assert_eq!(r.ctx.tenant, None);
    assert_eq!(r.ctx.account, None);
    assert!(r.ctx.truncated);
}

#[test]
fn an_empty_ctx_object_is_the_default_ctx() {
    let r: Request = de(r#"{"op":"get","ctx":{}}"#).unwrap();
    assert_eq!(r.ctx, Ctx::default());
}

#[test]
fn a_null_ctx_is_refused_where_an_object_is_expected() {
    let e = de::<Request>(r#"{"op":"get","ctx":null}"#).unwrap_err();
    assert!(e.contains("invalid type: null"), "{e}");
}

#[test]
fn a_null_op_or_a_missing_op_is_refused() {
    let e = de::<Request>(r#"{"op":null}"#).unwrap_err();
    assert_eq!(e, "expected value at line 1 column 7");
    let e = de::<Request>(r#"{}"#).unwrap_err();
    assert!(e.contains("missing field `op`"), "{e}");
}

#[test]
fn an_unknown_op_names_the_variant_and_lists_the_known_ones() {
    let e = de::<Request>(r#"{"op":"frobnicate"}"#).unwrap_err();
    assert!(e.contains("unknown variant `frobnicate`"), "{e}");
    assert!(e.contains("`create`") && e.contains("`call`"), "{e}");
}

#[test]
fn wrong_field_types_are_refused() {
    let e = de::<Request>(r#"{"op":"get","rows":{"a":1}}"#).unwrap_err();
    assert!(e.contains("invalid type: map"), "{e}");
    let e = de::<Request>(r#"{"op":"get","ctx":{"now_ms":"hier"}}"#).unwrap_err();
    assert!(e.contains("invalid type: string"), "{e}");
    let e = de::<Request>(r#"{"op":"get","ctx":{"truncated":"oui"}}"#).unwrap_err();
    assert!(e.contains("invalid type: string"), "{e}");
    let e = de::<Request>(r#"{"op":"get","name":3}"#).unwrap_err();
    assert!(e.contains("invalid type: integer"), "{e}");
}

#[test]
fn request_round_trips_exactly() {
    let r = Request {
        op: Op::Call,
        name: Some("archiver".into()),
        entity: Some("note".into()),
        input: json!({"id": "n1", "deep": [1, null, {"k": false}]}),
        rows: vec![json!({"id": "n1"}), json!({"id": "n2", "x": 1.5})],
        ctx: Ctx {
            now_ms: -5,
            tenant: Some("t".into()),
            account: None,
            locale: "fr".into(),
            config: json!({"max": 3}),
            truncated: true,
        },
    };
    let text = serde_json::to_string(&r).unwrap();
    let back: Request = serde_json::from_str(&text).unwrap();
    assert_eq!(back, r);
}

#[test]
fn request_helpers_return_none_when_nothing_matches() {
    let r: Request =
        de(r#"{"op":"get","input":[1,2],"rows":[{"id":"a"},{"id":7},{"x":1}]}"#).unwrap();
    // `input` n'est pas un objet : aucun champ.
    assert_eq!(r.input_field("a"), None);
    assert_eq!(r.config("k"), None);
    assert_eq!(r.row("a"), Some(&json!({"id": "a"})));
    // Une ligne dont l'id n'est pas une chaîne, ou sans id, n'est jamais trouvée.
    assert_eq!(r.row("7"), None);
    assert_eq!(r.row("absent"), None);
    let r: Request = de(r#"{"op":"get","input":{"x":null}}"#).unwrap();
    assert_eq!(r.input_field("x"), Some(&Value::Null));
    assert_eq!(r.input_field("y"), None);
}

#[test]
fn effects_round_trip_and_unknown_kinds_are_refused() {
    let all = vec![
        Effect::put("task", json!({"id": "1"})),
        Effect::delete("task", "1"),
        Effect::emit("t.done", json!(null)),
        Effect::log(Level::Warn, "m"),
    ];
    let back: Vec<Effect> = serde_json::from_str(&serde_json::to_string(&all).unwrap()).unwrap();
    assert_eq!(back, all);
    let e = de::<Effect>(r#"{"kind":"explode"}"#).unwrap_err();
    assert!(e.contains("unknown variant `explode`"), "{e}");
    let e = de::<Effect>(r#"{"kind":"log","level":"fatal","message":"m"}"#).unwrap_err();
    assert!(e.contains("unknown variant `fatal`"), "{e}");
    let e = de::<Effect>(r#"{"kind":"delete","entity":"t"}"#).unwrap_err();
    assert!(e.contains("missing field `id`"), "{e}");
}

#[test]
fn error_field_is_absent_when_none_and_optional_when_read() {
    let e = Error::new("c", "m");
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"code":"c","message":"m"}"#
    );
    let back: Error = de(r#"{"code":"c","message":"m"}"#).unwrap();
    assert_eq!(back, e);
    let with = Error::new("c", "m").field("title");
    assert_eq!(with.field.as_deref(), Some("title"));
    let back: Error = de(&serde_json::to_string(&with).unwrap()).unwrap();
    assert_eq!(back, with);
    let e = de::<Error>(r#"{"code":"c"}"#).unwrap_err();
    assert!(e.contains("missing field `message`"), "{e}");
}

#[test]
fn response_round_trips_both_ways() {
    let ok = Response::ok_with(json!({"a": [1]}), vec![Effect::delete("t", "1")]);
    let back: Response = de(&serde_json::to_string(&ok).unwrap()).unwrap();
    assert_eq!(back, ok);
    let err = Response::field_err("c", "m", "f");
    let back: Response = de(&serde_json::to_string(&err).unwrap()).unwrap();
    assert_eq!(back, err);
}

#[test]
fn response_ok_defaults_output_and_effects() {
    let r: Response = de(r#"{"ok":true}"#).unwrap();
    assert_eq!(
        r,
        Response::Ok {
            output: Value::Null,
            effects: vec![]
        }
    );
}

#[test]
fn response_without_ok_or_with_a_non_boolean_ok_is_refused() {
    let e = de::<Response>(r#"{"output":1}"#).unwrap_err();
    assert!(e.contains("`ok` manquant"), "{e}");
    let e = de::<Response>(r#"{"ok":"yes"}"#).unwrap_err();
    assert!(e.contains("`ok` manquant"), "{e}");
    let e = de::<Response>(r#"{"ok":null}"#).unwrap_err();
    assert!(e.contains("`ok` manquant"), "{e}");
    // Ni un tableau ni un scalaire n'est une réponse.
    assert!(de::<Response>("[]").is_err());
}

#[test]
fn response_error_branch_refuses_a_missing_or_null_error() {
    let e = de::<Response>(r#"{"ok":false}"#).unwrap_err();
    assert!(e.contains("invalid type: null"), "{e}");
    let e = de::<Response>(r#"{"ok":false,"error":{"code":"c"}}"#).unwrap_err();
    assert!(e.contains("missing field `message`"), "{e}");
}

#[test]
fn response_ok_branch_refuses_malformed_effects() {
    let e = de::<Response>(r#"{"ok":true,"effects":"x"}"#).unwrap_err();
    assert!(e.contains("invalid type: string"), "{e}");
    // Une liste d'effets tronquée : le dernier élément est incomplet.
    let e = de::<Response>(r#"{"ok":true,"effects":[{"kind":"delete","entity":"t","id":"1"},{"kind":"put","entity":"t"}]}"#)
        .unwrap_err();
    assert!(e.contains("missing field `body`"), "{e}");
}

#[test]
fn response_effect_builder_only_touches_a_success() {
    let r = Response::ok(json!(1))
        .effect(Effect::log(Level::Info, "a"))
        .effect(Effect::delete("t", "1"));
    match r {
        Response::Ok { effects, .. } => assert_eq!(effects.len(), 2),
        other => panic!("{other:?}"),
    }
    let r = Response::err("c", "m").effect(Effect::log(Level::Info, "a"));
    assert_eq!(r, Response::err("c", "m"));
}

#[test]
fn an_error_converts_into_a_failed_response() {
    let r: Response = Error::new("c", "m").field("f").into();
    assert_eq!(r, Response::field_err("c", "m", "f"));
}

#[test]
fn ctx_default_has_empty_everything() {
    let c = Ctx::default();
    assert_eq!((c.now_ms, c.locale.as_str(), c.truncated), (0, "", false));
    let back: Ctx = de(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(back, c);
}
