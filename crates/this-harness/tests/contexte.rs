//! Le contexte éphémère d'un tour : dans la requête, jamais dans le transcript.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_harness::{
    Cancel, Delta, Failure, Finish, Harness, Limits, Message, Outcome, Reply, Request, ToolCall,
    ToolSpec, Toolbox, Transport, TurnContext,
};

/// Un moteur qui récite un script et garde chaque requête reçue.
struct Script {
    replies: RefCell<VecDeque<Reply>>,
    seen: RefCell<Vec<Request>>,
}

#[async_trait(?Send)]
impl Transport for Script {
    async fn stream(
        &self,
        request: &Request,
        _cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        self.seen.borrow_mut().push(request.clone());
        let reply = self.replies.borrow_mut().pop_front().unwrap_or(Reply {
            text: "fini".to_string(),
            calls: vec![],
            finish: Finish::Stop,
        });
        delta(&reply.text);
        Ok(reply)
    }
}

struct Tools;

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lire".to_string(),
            description: "lit".to_string(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _call: &ToolCall) -> Outcome {
        Outcome::ok("lu")
    }
}

/// Un contexte qui compte ses appels et rend « tour N » (ou rien).
struct Compteur {
    appels: Cell<u32>,
    vide: bool,
    vus: RefCell<Vec<usize>>,
}

#[async_trait(?Send)]
impl TurnContext for Compteur {
    async fn before_turn(&self, transcript: &[Message]) -> Option<String> {
        self.appels.set(self.appels.get() + 1);
        self.vus.borrow_mut().push(transcript.len());
        (!self.vide).then(|| format!("mémoire du tour {}", self.appels.get()))
    }
}

fn script(replies: Vec<Reply>) -> Rc<Script> {
    Rc::new(Script {
        replies: RefCell::new(replies.into()),
        seen: RefCell::new(Vec::new()),
    })
}

fn compteur(vide: bool) -> Rc<Compteur> {
    Rc::new(Compteur {
        appels: Cell::new(0),
        vide,
        vus: RefCell::new(Vec::new()),
    })
}

fn harnais(transport: Rc<Script>, contexte: Option<Rc<Compteur>>) -> Harness {
    let harness = Harness::new(
        "tu es l'atelier",
        Limits::default(),
        transport,
        Rc::new(Tools),
        |_| {},
    );
    match contexte {
        Some(c) => harness.with_turn_context(c),
        None => harness,
    }
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

fn is_system(message: &Message) -> bool {
    matches!(message, Message::System { .. })
}

#[test]
fn contexte_dans_la_requete_avant_le_dernier_message_utilisateur_pas_dans_le_transcript() {
    let transport = script(vec![]);
    let contexte = compteur(false);
    let harness = harnais(transport.clone(), Some(contexte.clone()));
    harness.seed(vec![Message::user("avant"), Message::assistant("vu")]);
    run(harness.send("ajoute une alerte"));

    let seen = transport.seen.borrow();
    assert_eq!(seen.len(), 1);
    let thread = seen[0].thread();
    // consigne, user « avant », assistant, contexte, user « ajoute… »
    assert_eq!(thread.len(), 5, "{thread:?}");
    assert_eq!(thread[0], Message::system("tu es l'atelier"));
    assert_eq!(thread[3], Message::system("mémoire du tour 1"));
    assert_eq!(thread[4], Message::user("ajoute une alerte"));

    let transcript = harness.transcript();
    assert!(
        !transcript
            .iter()
            .any(|m| m.text().contains("mémoire du tour")),
        "{transcript:?}"
    );
    assert!(!transcript.iter().any(is_system), "{transcript:?}");
    // Le hook a lu le transcript tel qu'il était : 2 messages semés + la demande.
    assert_eq!(*contexte.vus.borrow(), vec![3]);
}

#[test]
fn contexte_none_ne_change_rien() {
    let avec = script(vec![]);
    let harness = harnais(avec.clone(), Some(compteur(true)));
    run(harness.send("bonjour"));
    let sans = script(vec![]);
    let temoin = harnais(sans.clone(), None);
    run(temoin.send("bonjour"));

    assert_eq!(avec.seen.borrow()[0], sans.seen.borrow()[0]);
    assert_eq!(
        avec.seen.borrow()[0].thread(),
        vec![Message::system("tu es l'atelier"), Message::user("bonjour")]
    );
    assert_eq!(harness.transcript(), temoin.transcript());
}

#[test]
fn contexte_recalcule_a_chaque_tour() {
    let transport = script(vec![Reply {
        text: String::new(),
        calls: vec![ToolCall {
            id: "a1".to_string(),
            name: "lire".to_string(),
            arguments: json!({}),
        }],
        finish: Finish::Tools,
    }]);
    let contexte = compteur(false);
    let harness = harnais(transport.clone(), Some(contexte.clone()));
    run(harness.send("lis puis conclus"));

    let seen = transport.seen.borrow();
    assert_eq!(seen.len(), 2, "un tour d'outil puis une conclusion");
    assert_eq!(contexte.appels.get(), 2);
    for (i, request) in seen.iter().enumerate() {
        let contextes: Vec<Message> = request
            .thread()
            .into_iter()
            .skip(1)
            .filter(is_system)
            .collect();
        assert_eq!(
            contextes,
            vec![Message::system(format!("mémoire du tour {}", i + 1))],
            "un seul contexte par requête, celui de son tour"
        );
    }
    // Au deuxième tour, le contexte reste avant le message utilisateur, et le
    // résultat d'outil le suit.
    let second = seen[1].thread();
    let pos_ctx = second.iter().skip(1).position(is_system).unwrap() + 1;
    assert_eq!(second[pos_ctx + 1], Message::user("lis puis conclus"));
    assert!(!harness.transcript().iter().any(is_system));
    // Le hook a vu grandir le transcript : demande, puis demande + appel + résultat.
    assert_eq!(*contexte.vus.borrow(), vec![1, 3]);
}
