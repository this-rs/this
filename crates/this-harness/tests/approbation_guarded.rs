//! `Harness::from_approved_doc_guarded` : approbation ET autorisation en un seul geste.
//!
//! POURQUOI : `from_approved_doc` vérifie l'approbation mais n'installe aucune
//! autorisation ; un hôte qui l'utilise seul croit être gardé. Chaque test compte les
//! appels réels (moteur, outil) : un refus qui laisse passer l'appel serait pire que rien.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_agent::DefaultPolicy;
use this_agent::approval::Approval;
use this_agent::evidence::Declaration;
use this_agent::graph::{HarnessDoc, Slot, SlotKind};
use this_agent::substrate::{Grant, Grants, Verdict};
use this_harness::{
    Cancel, Delta, Event, Failure, Finish, Harness, Outcome, Reply, Request, ToolCall, ToolSpec,
    Toolbox, Transport,
};

/// Demande `declared_tool` une fois, puis conclut ; compte les tours.
struct Engine {
    streams: Cell<u32>,
}

#[async_trait(?Send)]
impl Transport for Engine {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let n = self.streams.get();
        self.streams.set(n + 1);
        if n == 0 {
            return Ok(Reply {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "declared_tool".into(),
                    arguments: json!({}),
                }],
                finish: Finish::Tools,
            });
        }
        Ok(Reply {
            text: "fini".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

struct Tools {
    calls: Cell<u32>,
}

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "declared_tool".into(),
            description: "d".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    fn declarations(&self) -> Vec<Declaration> {
        vec![Declaration::writes("declared_tool")]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        self.calls.set(self.calls.get() + 1);
        Outcome::ok("exécuté")
    }
}

fn doc() -> HarnessDoc {
    HarnessDoc {
        id: "h".into(),
        name: "atelier".into(),
        version: 1,
        brief: "tu es l'atelier".into(),
        slots: vec![Slot {
            kind: SlotKind::Policy,
            module: "pol".into(),
            version: "1.0.0".into(),
            entry: "main".into(),
        }],
        max_tours: 12,
        max_relances: 3,
        budget_ms: Some(1000),
        approval: None,
        silence_ms: None,
        no_progress_ms: None,
        tool_timeout_ms: None,
        context_budget: None,
    }
}

fn approuve(mut d: HarnessDoc) -> (HarnessDoc, Vec<Approval>) {
    let a = Approval {
        by: "alice".into(),
        at_ms: 1,
        fingerprint: d.fingerprint(),
    };
    d.approval = Some(a.clone());
    (d, vec![a])
}

fn granted() -> Grants {
    let mut g = Grants::new();
    g.put(Grant::granted("declared_tool", "user"));
    g
}

struct Banc {
    engine: Rc<Engine>,
    tools: Rc<Tools>,
    log: Rc<RefCell<Vec<Event>>>,
}

fn banc() -> Banc {
    Banc {
        engine: Rc::new(Engine {
            streams: Cell::new(0),
        }),
        tools: Rc::new(Tools {
            calls: Cell::new(0),
        }),
        log: Rc::new(RefCell::new(Vec::new())),
    }
}

fn build(
    b: &Banc,
    d: &HarnessDoc,
    trusted: &[Approval],
    grants: Grants,
) -> Result<Harness, Verdict> {
    let sink = b.log.clone();
    Harness::from_approved_doc_guarded(
        d,
        trusted,
        grants,
        b.engine.clone(),
        b.tools.clone(),
        Rc::new(DefaultPolicy),
        move |e| sink.borrow_mut().push(e),
    )
}

#[test]
fn composition_non_approuvee_aucun_harnais_n_existe() {
    let b = banc();
    // Même avec des autorisations larges, sans approbation : refus, rien d'appelé.
    let r = build(&b, &doc(), &[], granted());
    match r.err().expect("pas de harnais") {
        Verdict::Refuse(m) => assert!(m.contains("aucune approbation"), "{m}"),
        Verdict::Allow => panic!("un refus était attendu"),
    }
    assert_eq!(b.engine.streams.get(), 0);
    assert_eq!(b.tools.calls.get(), 0);
    assert!(b.log.borrow().is_empty());
}

#[test]
fn composition_approuvee_outil_non_autorise_refuse_avant_l_appel() {
    let b = banc();
    let (d, trusted) = approuve(doc());
    // Aucune autorisation accordée : le harnais existe, mais l'outil ne s'exécute pas.
    let harness = build(&b, &d, &trusted, Grants::new()).expect("approuvée");
    futures::executor::block_on(harness.send("go"));
    assert_eq!(b.tools.calls.get(), 0, "l'outil n'a jamais été appelé");
    assert!(
        b.log
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::ToolResult { refused: true, .. })),
        "le refus est un résultat d'outil en échec : {:?}",
        b.log.borrow()
    );
}

#[test]
fn composition_approuvee_et_outil_autorise_s_execute() {
    let b = banc();
    let (d, trusted) = approuve(doc());
    let harness = build(&b, &d, &trusted, granted()).expect("approuvée");
    futures::executor::block_on(harness.send("go"));
    assert_eq!(b.tools.calls.get(), 1, "autorisé : il s'exécute");
}
