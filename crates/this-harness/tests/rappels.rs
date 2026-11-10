//! Le rappel cite, dit l'introuvable, reste synthétique et borné.
use std::cell::RefCell;
use std::rc::Rc;

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

/// Un port qui compte ses appels : la preuve qu'un rappel sans ancre ne tente
/// rien, et qu'avec des ancres il n'appelle qu'une fois, avec les ids dédupliqués.
struct Compteur(Rc<RefCell<Vec<Vec<String>>>>);

#[async_trait::async_trait(?Send)]
impl Recall for Compteur {
    async fn recall(&self, ids: &[String]) -> Vec<Fact> {
        self.0.borrow_mut().push(ids.to_vec());
        vec![]
    }
}

#[test]
fn sans_ancre_le_port_de_rappel_n_est_jamais_appele() {
    let appels = Rc::new(RefCell::new(Vec::new()));
    let port = Compteur(Rc::clone(&appels));
    // Rien à rappeler : ni id ni chemin — donc aucune tentative vers le port.
    let a = anchors(&[Message::user("bonjour"), Message::assistant("salut")]);
    assert_eq!(a, Anchors::default());
    assert!(block_on(rappel(&a, &port)).is_none());
    assert!(
        appels.borrow().is_empty(),
        "un rappel sans ancre ne doit pas toucher le port (aucune tentative réseau)"
    );
}

#[test]
fn avec_des_ancres_le_port_est_appele_une_fois_avec_les_ids_dedupliques() {
    let appels = Rc::new(RefCell::new(Vec::new()));
    let port = Compteur(Rc::clone(&appels));
    // conv() cite A deux fois : le port reçoit la liste dédupliquée, une fois.
    let a = anchors(&conv());
    block_on(rappel(&a, &port)).unwrap();
    assert_eq!(appels.borrow().len(), 1, "un seul appel au port");
    assert_eq!(appels.borrow()[0], [A, B, FAKE]);
}
