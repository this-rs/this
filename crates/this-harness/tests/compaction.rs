//! La compaction : résumer les tours anciens au lieu de les jeter.
//!
//! POURQUOI ce banc : une compaction qui rate en silence est pire qu'une jetée — elle
//! annonce un résumé qui n'existe pas. Chaque test regarde ce que le MOTEUR a reçu, pas
//! seulement ce que le harnais dit avoir fait.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_harness::{
    Cancel, Code, Compactor, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome,
    Reply, Request, ToolCall, ToolSpec, Toolbox, Transport, estimate_tokens,
};

const BUDGET: usize = 400;

/// Un moteur qui garde le fil de chaque requête et conclut aussitôt.
#[derive(Default)]
struct Engine {
    seen: RefCell<Vec<Vec<Message>>>,
}

#[async_trait(?Send)]
impl Transport for Engine {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.seen.borrow_mut().push(request.thread());
        Ok(Reply {
            text: "fini".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

struct NoTools;

#[async_trait(?Send)]
impl Toolbox for NoTools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lire".into(),
            description: "lit".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("ok")
    }
}

/// Un compacteur scripté : rend `answer`, note ce qu'on lui a donné.
struct Scripted {
    answer: Result<Message, Failure>,
    got: RefCell<Vec<Vec<Message>>>,
    calls: Cell<u32>,
}

impl Scripted {
    fn new(answer: Result<Message, Failure>) -> Rc<Self> {
        Rc::new(Self {
            answer,
            got: RefCell::new(Vec::new()),
            calls: Cell::new(0),
        })
    }
}

#[async_trait(?Send)]
impl Compactor for Scripted {
    async fn compact(&self, anciens: &[Message]) -> Result<Message, Failure> {
        self.calls.set(self.calls.get() + 1);
        self.got.borrow_mut().push(anciens.to_vec());
        self.answer.clone()
    }
}

fn gros(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}

/// Dix tours (question + réponse), ~100 jetons chacun, l'un d'eux avec un groupe d'outils.
fn fil_de_dix_tours() -> Vec<Message> {
    let mut fil = Vec::new();
    for i in 0..10 {
        fil.push(Message::user(format!("q{i} {}", gros('a', 180))));
        if i == 3 {
            fil.push(Message::Assistant {
                content: String::new(),
                calls: vec![
                    ToolCall {
                        id: "t1".into(),
                        name: "lire".into(),
                        arguments: json!({"chemin": "a"}),
                    },
                    ToolCall {
                        id: "t2".into(),
                        name: "lire".into(),
                        arguments: json!({"chemin": "b"}),
                    },
                ],
            });
            fil.push(Message::tool("t1", gros('x', 100)));
            fil.push(Message::tool("t2", gros('y', 100)));
        }
        fil.push(Message::assistant(format!("r{i} {}", gros('b', 180))));
    }
    fil
}

struct Run {
    engine: Rc<Engine>,
    log: Rc<RefCell<Vec<Event>>>,
    transcript: Vec<Message>,
}

fn run(compactor: Option<Rc<dyn Compactor>>) -> Run {
    let engine = Rc::new(Engine::default());
    let log = Rc::new(RefCell::new(Vec::new()));
    let journal = log.clone();
    let mut harness = Harness::new(
        "consigne",
        Limits {
            max_tours: 4,
            max_relances: 0,
            budget_ms: None,
            context_budget: Some(BUDGET),
            ..Limits::default()
        },
        engine.clone(),
        Rc::new(NoTools),
        move |e| journal.borrow_mut().push(e),
    );
    if let Some(c) = compactor {
        harness = harness.with_compactor(c);
    }
    harness.seed(fil_de_dix_tours());
    let _ = futures::executor::block_on(harness.send("la vraie question"));
    Run {
        engine,
        log,
        transcript: harness.transcript(),
    }
}

fn events(run: &Run, name: &str) -> Vec<Event> {
    run.log
        .borrow()
        .iter()
        .filter(|e| e.name() == name)
        .cloned()
        .collect()
}

fn pas_d_orphelin(fil: &[Message]) {
    let mut ouverts: Vec<String> = Vec::new();
    for m in fil {
        match m {
            Message::Assistant { calls, .. } => {
                ouverts = calls.iter().map(|c| c.id.clone()).collect()
            }
            Message::Tool { id, .. } => assert!(ouverts.contains(id), "réponse orpheline {id}"),
            _ => ouverts.clear(),
        }
    }
}

fn resume() -> Message {
    Message::user("RÉSUMÉ des tours anciens")
}

#[test]
fn sans_compacteur_on_jette_et_trimmed_le_dit() {
    let r = run(None);
    assert!(events(&r, "compacted").is_empty());
    let trimmed = events(&r, "trimmed");
    assert!(!trimmed.is_empty(), "un Trimmed était attendu");
    let envoye = r.engine.seen.borrow()[0].clone();
    assert!(estimate_tokens(&envoye) <= BUDGET);
    // Les plus anciens ont disparu sans laisser de trace dans le fil.
    assert!(!envoye.iter().any(|m| m.text().starts_with("q0 ")));
    assert!(!envoye.iter().any(|m| m.text().contains("RÉSUMÉ")));
    pas_d_orphelin(&envoye);
}

#[test]
fn avec_un_compacteur_le_fil_envoye_est_resume_plus_queue() {
    let c = Scripted::new(Ok(resume()));
    let r = run(Some(c.clone()));

    let compacted = events(&r, "compacted");
    assert_eq!(compacted.len(), 1, "un seul remplacement : {compacted:?}");
    let Event::Compacted {
        messages,
        tokens,
        summary_tokens,
    } = compacted[0].clone()
    else {
        unreachable!()
    };
    assert!(messages > 0 && summary_tokens < tokens);
    assert_eq!(c.calls.get(), 1);
    assert_eq!(c.got.borrow()[0].len(), messages);
    // Le compacteur a reçu les PLUS ANCIENS : la première question y est.
    assert!(c.got.borrow()[0][0].text().starts_with("q0 "));

    let envoye = r.engine.seen.borrow()[0].clone();
    assert!(estimate_tokens(&envoye) <= BUDGET, "sous le budget");
    assert!(
        envoye
            .iter()
            .any(|m| m.text() == "RÉSUMÉ des tours anciens"),
        "le résumé est dans le fil envoyé"
    );
    assert!(!envoye.iter().any(|m| m.text().starts_with("q0 ")));
    assert_eq!(
        envoye.last().map(Message::text),
        Some("la vraie question"),
        "la question reste la dernière"
    );
    // Rien n'est perdu en silence : ce qui a été retiré a été résumé, pas jeté.
    assert!(events(&r, "trimmed").is_empty());
    // Le résumé est dans le transcript : il ne sera pas recalculé au tour suivant.
    assert!(r.transcript.iter().any(|m| m.text().contains("RÉSUMÉ")));
    assert!(r.transcript.len() < fil_de_dix_tours().len());
}

#[test]
fn compacteur_en_echec_repli_sur_la_jetee_et_le_run_finit() {
    let c = Scripted::new(Err(Failure::new(Code::Unreachable, "panne")));
    let r = run(Some(c.clone()));
    assert_eq!(c.calls.get(), 1, "il a bien été essayé");
    assert!(events(&r, "compacted").is_empty());
    assert!(!events(&r, "trimmed").is_empty(), "Trimmed le dit");
    let envoye = r.engine.seen.borrow()[0].clone();
    assert!(estimate_tokens(&envoye) <= BUDGET);
    assert!(
        r.log
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::Stopped { .. })),
        "le run finit normalement"
    );
    assert_eq!(r.engine.seen.borrow().len(), 1);
}

#[test]
fn resume_invalide_ou_trop_gros_repli_sur_la_jetee() {
    let invalides = [
        Message::user("   "),
        Message::system("consigne"),
        Message::tool("t1", "orphelin"),
        Message::Assistant {
            content: "x".into(),
            calls: vec![ToolCall::default()],
        },
        // Plus gros que ce qu'il remplace : compacter ferait grossir le fil.
        Message::user(gros('z', 20_000)),
    ];
    for message in invalides {
        let r = run(Some(Scripted::new(Ok(message.clone()))));
        assert!(events(&r, "compacted").is_empty(), "{message:?}");
        assert!(!events(&r, "trimmed").is_empty(), "{message:?}");
        let envoye = r.engine.seen.borrow()[0].clone();
        assert!(estimate_tokens(&envoye) <= BUDGET);
        pas_d_orphelin(&envoye);
    }
}

#[test]
fn le_prefixe_resume_ne_coupe_jamais_un_groupe_d_outils() {
    // Budget choisi pour que la frontière tombe autour du groupe d'outils (tour 3) :
    // on balaie plusieurs budgets, l'invariant doit tenir pour chacun.
    let mut groupes_resumes = 0;
    for budget in (150..900).step_by(25) {
        let c = Scripted::new(Ok(Message::user("RÉSUMÉ")));
        let engine = Rc::new(Engine::default());
        let harness = Harness::new(
            "consigne",
            Limits {
                max_tours: 4,
                max_relances: 0,
                budget_ms: None,
                context_budget: Some(budget),
                ..Limits::default()
            },
            engine.clone(),
            Rc::new(NoTools),
            |_| {},
        )
        .with_compactor(c.clone());
        harness.seed(fil_de_dix_tours());
        let _ = futures::executor::block_on(harness.send("la vraie question"));
        for anciens in c.got.borrow().iter() {
            if anciens.iter().any(|m| matches!(m, Message::Tool { .. })) {
                groupes_resumes += 1;
            }
            pas_d_orphelin(anciens);
            // Un groupe est entier : autant d'appels que de réponses.
            let appels: usize = anciens
                .iter()
                .map(|m| match m {
                    Message::Assistant { calls, .. } => calls.len(),
                    _ => 0,
                })
                .sum();
            let reponses = anciens
                .iter()
                .filter(|m| matches!(m, Message::Tool { .. }))
                .count();
            assert_eq!(appels, reponses, "budget {budget} : groupe coupé");
        }
        for envoye in engine.seen.borrow().iter() {
            pas_d_orphelin(envoye);
        }
        pas_d_orphelin(&harness.transcript());
    }
    assert!(
        groupes_resumes > 0,
        "le balayage doit résumer au moins une fois le groupe d'outils, sinon il ne prouve rien"
    );
}
