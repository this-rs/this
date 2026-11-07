//! Mise en erreur du fil (wire.rs, context.rs) : chaque test reproduit un défaut
//! ou verrouille un comportement qu'on n'a pas réussi à casser.

use serde_json::json;
use this_harness::{Code, Failure, Message, ToolCall, estimate_tokens, fit, removable};

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "outil".into(),
        arguments: json!({}),
    }
}
fn assistant(calls: Vec<ToolCall>) -> Message {
    Message::Assistant {
        content: String::new(),
        calls,
    }
}

/// Aucun Tool gardé sans un Assistant gardé qui l'appelle.
fn no_orphan(kept: &[Message]) -> bool {
    kept.iter().enumerate().all(|(i, m)| match m {
        Message::Tool { id, .. } => kept[..i].iter().any(
            |p| matches!(p, Message::Assistant { calls, .. } if calls.iter().any(|c| &c.id == id)),
        ),
        _ => true,
    })
}

// ---- estimation ----

#[test]
fn cjk_text_is_not_estimated_at_a_quarter_token_per_char() {
    // Défaut : 4 caractères par jeton pour tout. Le CJK coûte ~1 jeton/caractère :
    // 4000 caractères étaient comptés 1000 jetons, et `fit` ne coupait rien.
    let t = "你".repeat(4000);
    assert!(estimate_tokens(&[Message::user(t)]) >= 4000);
}

#[test]
fn emoji_are_not_estimated_at_a_quarter_token_per_char() {
    let t = "😀".repeat(2000);
    assert!(estimate_tokens(&[Message::user(t)]) >= 2000);
}

#[test]
fn latin_estimate_is_unchanged() {
    assert_eq!(estimate_tokens(&[Message::user("abcd")]), 4 + 1);
    assert_eq!(estimate_tokens(&[Message::user("")]), 4);
    assert_eq!(estimate_tokens(&[Message::user("a")]), 4 + 1);
}

#[test]
fn huge_inputs_do_not_overflow() {
    let big = Message::user("x".repeat(10 * 1024 * 1024));
    assert!(estimate_tokens(std::slice::from_ref(&big)) > 2_000_000);
    let many = vec![big; 3];
    assert!(estimate_tokens(&many) > 6_000_000);
    let huge_args = Message::Assistant {
        content: String::new(),
        calls: vec![ToolCall {
            id: "a".into(),
            name: "n".into(),
            arguments: json!({"k": "y".repeat(5 * 1024 * 1024)}),
        }],
    };
    assert!(estimate_tokens(&[huge_args]) > 1_000_000);
}

// ---- fit / removable ----

#[test]
fn fit_extreme_budgets() {
    let t = vec![
        Message::system("s"),
        Message::user("q1"),
        Message::assistant("r"),
        Message::user("q2"),
    ];
    let zero = fit(&t, 0);
    assert!(zero.dropped.over_budget);
    assert!(zero.kept.contains(&Message::user("q2")) && zero.kept.contains(&Message::system("s")));
    let max = fit(&t, usize::MAX);
    assert_eq!(max.kept, t);
    assert!(max.dropped.is_empty() && !max.dropped.over_budget);
    assert!(fit(&[], 0).kept.is_empty());
}

#[test]
fn fit_is_linear_on_a_huge_thread() {
    let mut t = vec![Message::user("q")];
    for i in 0..50_000 {
        t.push(assistant(vec![call(&format!("c{i}"))]));
        t.push(Message::tool(format!("c{i}"), "ok"));
    }
    let start = std::time::Instant::now();
    let f = fit(&t, 100_000);
    let r = removable(&t, 100_000);
    assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
    assert!(no_orphan(&f.kept));
    assert_eq!(f.dropped.messages, r.indices.len());
}

#[test]
fn odd_shapes_never_leave_an_orphan_tool_the_input_did_not_have() {
    // Assistant sans appels, deux Assistant d'affilée, groupes successifs (un Tool en
    // tête serait déjà orphelin dans l'entrée : voir `check_thread`).
    let t = vec![
        Message::user("q"),
        assistant(vec![call("a"), call("b")]),
        Message::tool("a", "1"),
        Message::tool("b", "2"),
        assistant(vec![call("c")]),
        Message::tool("c", "3"),
        assistant(vec![]),
        Message::assistant("fin"),
        Message::user("q2"),
    ];
    for budget in 0..60 {
        let f = fit(&t, budget);
        let kept_ids: Vec<_> = f
            .kept
            .iter()
            .filter(|m| matches!(m, Message::Tool { .. }))
            .collect();
        // un Tool gardé a son Assistant gardé : les groupes contigus
        for (i, m) in f.kept.iter().enumerate() {
            if let Message::Tool { id, .. } = m {
                let has = f.kept[..i].iter().any(|p| {
                    matches!(p, Message::Assistant { calls, .. } if calls.iter().any(|c| &c.id == id))
                });
                assert!(
                    has,
                    "budget {budget}: Tool {id} orphelin ({} gardés)",
                    kept_ids.len()
                );
            }
        }
        let r = removable(&t, budget);
        assert_eq!(r.indices.len(), f.dropped.messages);
    }
}

#[test]
fn a_system_message_between_a_call_and_its_answer_does_not_orphan_the_answer() {
    // Défaut : le groupe s'arrêtait au Système intercalé ; la réponse Tool devenait
    // une unité à part, gardable alors que son Assistant était retiré.
    let t = vec![
        Message::user("q"),
        assistant(vec![call("a")]),
        Message::system("contexte"),
        Message::tool("a", "res"),
        Message::user("q2"),
    ];
    for budget in 0..40 {
        let f = fit(&t, budget);
        assert!(no_orphan(&f.kept), "budget {budget}: {:?}", f.kept);
        let r = removable(&t, budget);
        let left: Vec<_> = t
            .iter()
            .enumerate()
            .filter(|(i, _)| !r.indices.contains(i))
            .map(|(_, m)| m.clone())
            .collect();
        assert!(no_orphan(&left), "removable, budget {budget}");
    }
}

// ---- construction : refuser le fil invalide ----

#[test]
fn check_thread_refuses_what_the_engine_would_refuse() {
    use this_harness::ThreadDefect as D;
    let ok = vec![
        Message::user("q"),
        assistant(vec![call("a"), call("b")]),
        Message::tool("a", "1"),
        Message::tool("b", "2"),
    ];
    assert_eq!(Message::check_thread(&ok), Ok(()));
    assert_eq!(Message::check_thread(&[]), Ok(()));

    let orphan = [Message::user("q"), Message::tool("a", "x")];
    assert_eq!(
        Message::check_thread(&orphan),
        Err(D::OrphanTool { index: 1 })
    );

    let empty = [assistant(vec![call("")])];
    assert_eq!(Message::check_thread(&empty), Err(D::EmptyId { index: 0 }));

    let empty_tool = [assistant(vec![call("a")]), Message::tool("", "x")];
    assert_eq!(
        Message::check_thread(&empty_tool),
        Err(D::EmptyId { index: 1 })
    );

    let dup = [assistant(vec![call("a"), call("a")])];
    assert_eq!(
        Message::check_thread(&dup),
        Err(D::DuplicateCallId { index: 0 })
    );

    let twice = [
        assistant(vec![call("a")]),
        Message::tool("a", "1"),
        Message::tool("a", "2"),
    ];
    assert_eq!(
        Message::check_thread(&twice),
        Err(D::OrphanTool { index: 2 })
    );

    let wrong = [assistant(vec![call("a")]), Message::tool("zzz", "1")];
    assert_eq!(
        Message::check_thread(&wrong),
        Err(D::OrphanTool { index: 1 })
    );
}

// ---- sérialisation ----

#[test]
fn every_variant_round_trips() {
    let t = vec![
        Message::system(""),
        Message::user("é\u{0}\n\"\\"),
        Message::hint("h"),
        Message::user_with_images("i", vec![]),
        assistant(vec![call("a")]),
        Message::assistant("x"),
        Message::tool("a", "r"),
    ];
    let s = serde_json::to_string(&t).unwrap();
    let back: Vec<Message> = serde_json::from_str(&s).unwrap();
    assert_eq!(back, t);
}

#[test]
fn hostile_json_is_an_error_not_a_message() {
    for bad in [
        r#"{"role":"tool"}"#,
        r#"{"role":"tool","content":"x"}"#,
        r#"{"role":"admin","content":"x"}"#,
        r#"{"content":"x"}"#,
        r#"{"role":"assistant","calls":[{"name":"n"}]}"#,
        r#"{"role":"user"}"#,
        r#"null"#,
    ] {
        assert!(serde_json::from_str::<Message>(bad).is_err(), "{bad}");
    }
    // Un Assistant sans contenu ni appels est lisible (valeurs par défaut).
    let m: Message = serde_json::from_str(r#"{"role":"assistant"}"#).unwrap();
    assert_eq!(m, Message::assistant(""));
}

#[test]
fn ten_megabytes_round_trip() {
    let m = Message::tool("a", "z".repeat(10 * 1024 * 1024));
    let s = serde_json::to_string(&m).unwrap();
    assert_eq!(serde_json::from_str::<Message>(&s).unwrap(), m);
}

// ---- Failure / Code ----

#[test]
fn code_as_str_matches_serde_for_every_variant() {
    for c in [
        Code::Unreachable,
        Code::Http,
        Code::Truncated,
        Code::Unreadable,
        Code::Timeout,
        Code::Aborted,
        Code::Provider,
    ] {
        assert_eq!(serde_json::to_value(c).unwrap(), json!(c.as_str()));
        let back: Code = serde_json::from_value(json!(c.as_str())).unwrap();
        assert_eq!(back, c);
    }
}

#[test]
fn failure_extremes() {
    for status in [0u16, 65535] {
        let f = Failure::http(status);
        let back: Failure = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }
    assert!(
        serde_json::from_str::<Failure>(r#"{"code":"http","message":"x","status":65536}"#).is_err()
    );
    assert!(serde_json::from_str::<Failure>(r#"{"code":"nouveau","message":"x"}"#).is_err());
    let big = Failure::new(Code::Provider, "m".repeat(10 * 1024 * 1024));
    assert_eq!(big.to_string().len(), 10 * 1024 * 1024);
}
