//! Le rappel cite, dit l'introuvable, reste synthétique et borné.
use futures::executor::block_on;
use serde_json::json;
use this_harness::{Anchors, Fact, Message, Recall, ToolCall, anchors, rappel};

const A: &str = "#plan:11111111-1111-1111-1111-111111111111";
const B: &str = "#note:22222222-2222-2222-2222-222222222222";
const FAKE: &str = "#task:99999999-9999-9999-9999-999999999999";

struct Scripted(Vec<Fact>);
#[async_trait::async_trait(?Send)]
impl Recall for Scripted {
    async fn recall(&self, ids: &[String]) -> Vec<Fact> {
        self.0
            .iter()
            .filter(|f| ids.contains(&f.id))
            .cloned()
            .collect()
    }
}
fn fact(id: &str, kind: &str, text: &str) -> Fact {
    Fact {
        id: id.into(),
        kind: kind.into(),
        fact: text.into(),
        as_of: "2026-11-08".into(),
        source: "obrain".into(),
    }
}
fn call(name: &str, args: serde_json::Value) -> Message {
    Message::Assistant {
        content: String::new(),
        calls: vec![ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args,
        }],
    }
}
fn conv() -> Vec<Message> {
    vec![
        Message::user(format!("regarde {A}")),
        Message::assistant(format!("la note {B} dit X, et {FAKE} aussi ; encore {A}")),
        call(
            "bash",
            json!({"command": "cargo test", "file": "src/lib.rs", "docs": "https://example.com/a/b.md"}),
        ),
    ]
}

#[test]
fn anchors_are_literal_deduped_and_ordered() {
    let a = anchors(&conv());
    let ids: Vec<&str> = a.cited.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, [A, B, FAKE]);
    assert_eq!(a.cited[1].message, 1);
    assert_eq!(a.files, [("src/lib.rs".to_string(), 2)], "URL ignorée");
    assert_eq!(a.tools, [("bash".to_string(), 1)]);
    assert_eq!(a.commands[0].0, "cargo test");
}

#[test]
fn malformed_ids_and_names_are_not_found() {
    let a = anchors(&[Message::user("#plan:pas-un-uuid et le plan « refonte »")]);
    assert!(a.cited.is_empty());
}

#[test]
fn found_ids_come_back_and_the_invented_one_is_flagged() {
    let r = Scripted(vec![
        fact(A, "plan", "en cours, 94 %"),
        fact(B, "note", "gotcha"),
    ]);
    let m = block_on(rappel(&anchors(&conv()), &r)).unwrap();
    let t = m.text();
    assert!(t.contains(A) && t.contains("en cours, 94 %") && t.contains("obrain"));
    assert!(t.contains(B));
    let line = t
        .lines()
        .find(|l| l.starts_with(&format!("- {FAKE}")))
        .unwrap();
    assert!(line.contains("INTROUVABLE"), "{line}");
    assert!(
        !t.lines()
            .find(|l| l.contains(A))
            .unwrap()
            .contains("INTROUVABLE")
    );
    // Les deux côtés apparaissent : fait du graphe et extrait de la conversation.
    assert!(t.contains("conversation (message 0)"));
}

#[test]
fn the_message_is_synthetic() {
    let m = block_on(rappel(&anchors(&conv()), &Scripted(vec![]))).unwrap();
    assert!(m.is_synthetic());
}

#[test]
fn a_hundred_entities_are_truncated_and_said() {
    let msgs: Vec<Message> = (0..100)
        .map(|n| Message::user(format!("#note:00000000-0000-0000-0000-{n:012x}")))
        .collect();
    let m = block_on(rappel(&anchors(&msgs), &Scripted(vec![]))).unwrap();
    let t = m.text();
    assert!(t.contains("coupé"), "{t}");
    assert!(t.contains("Entités citées (100)"));
    assert!(t.chars().count() <= 9000);
}

#[test]
fn nothing_to_recall_produces_nothing() {
    let a = anchors(&[Message::user("bonjour"), Message::assistant("salut")]);
    assert_eq!(a, Anchors::default());
    assert!(block_on(rappel(&a, &Scripted(vec![]))).is_none());
}
