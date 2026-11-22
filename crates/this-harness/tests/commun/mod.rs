//! Scénarios de robustesse PARTAGÉS : écrits une fois ici, exécutés en natif
//! (`tests/robustesse.rs`) et dans un navigateur (`tests/navigateur.rs`, wasm32).
//!
//! Aucun `catch_unwind`, aucun minuteur réel : un scénario n'entre ici que s'il a le même
//! sens sur les deux cibles (voir `docs/harnais-garanties-runtime.md`).
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use serde_json::json;

use this_agent::{Run, Step, Stop};
use this_harness::{
    Cancel, Clock, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome, Recorder,
    Reply, Request, Running, Sleep, ToolCall, ToolSpec, Toolbox, Transport,
};

pub const POLLS: usize = 2000;

/// Pilote une future avec un nombre de sondages borné.
pub fn bounded<F: Future>(fut: F) -> F::Output {
    let mut fut = Box::pin(fut);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..POLLS {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
    panic!("la future ne se résout pas en {POLLS} sondages");
}

#[derive(Default)]
pub struct Fake {
    pub now: Cell<f64>,
}
pub struct Yield(bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
impl Clock for Fake {
    fn now_ms(&self) -> f64 {
        self.now.get()
    }
    fn sleep(&self, ms: u64) -> Option<Sleep> {
        self.now.set(self.now.get() + ms as f64);
        Some(Box::pin(Yield(false)))
    }
}

pub type Script = Box<dyn Fn(&Request, &Cancel) -> Result<Reply, Failure>>;

pub struct Scripted {
    pub acts: RefCell<VecDeque<Script>>,
    pub fallback: Script,
}
pub fn say(text: &str) -> Reply {
    Reply {
        text: text.into(),
        calls: vec![],
        finish: Finish::Stop,
    }
}
pub fn call(id: &str) -> Reply {
    Reply {
        text: String::new(),
        calls: vec![ToolCall {
            id: id.into(),
            name: "t".into(),
            arguments: json!({}),
        }],
        finish: Finish::Tools,
    }
}
pub fn transport(acts: Vec<Script>) -> Rc<Scripted> {
    Rc::new(Scripted {
        acts: RefCell::new(acts.into()),
        fallback: Box::new(|_, _| Ok(say("fini"))),
    })
}
#[async_trait(?Send)]
impl Transport for Scripted {
    async fn stream(&self, r: &Request, c: &Cancel, _d: Delta) -> Result<Reply, Failure> {
        let act = self.acts.borrow_mut().pop_front();
        match act {
            Some(a) => a(r, c),
            None => (self.fallback)(r, c),
        }
    }
}

pub struct Tools {
    pub panics: bool,
    pub text: String,
}
#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "t".into(),
            description: String::new(),
            parameters: json!({"type":"object"}),
        }]
    }
    async fn call(&self, _c: &ToolCall) -> Outcome {
        if self.panics {
            panic!("outil en panique");
        }
        Outcome::ok(self.text.clone())
    }
}

pub type Log = Rc<RefCell<Vec<Event>>>;
pub fn harness(limits: Limits, t: Rc<dyn Transport>, tools: Tools) -> (Harness, Log) {
    let log: Log = Rc::default();
    let sink = log.clone();
    let h = Harness::new("sys", limits, t, Rc::new(tools), move |e| {
        sink.borrow_mut().push(e)
    })
    .with_clock(Rc::new(Fake::default()));
    (h, log)
}
pub fn ok_tools() -> Tools {
    Tools {
        panics: false,
        text: "ok".into(),
    }
}
pub fn stops(log: &Log) -> usize {
    log.borrow()
        .iter()
        .filter(|e| matches!(e, Event::Stopped { .. }))
        .count()
}

/// Chaque appel d'outil du fil a sa réponse, et chaque réponse son appel.
pub fn thread_is_valid(thread: &[Message]) -> bool {
    let mut open: Vec<String> = Vec::new();
    for m in thread {
        match m {
            Message::Assistant { calls, .. } => open.extend(calls.iter().map(|c| c.id.clone())),
            Message::Tool { id, .. } => {
                let Some(i) = open.iter().position(|o| o == id) else {
                    return false;
                };
                open.remove(i);
            }
            _ => {}
        }
    }
    open.is_empty()
}

pub struct NoTimer;
impl Clock for NoTimer {
    fn now_ms(&self) -> f64 {
        0.0
    }
    fn sleep(&self, _: u64) -> Option<Sleep> {
        None
    }
}

pub struct SlowRecorder;
#[async_trait(?Send)]
impl Recorder for SlowRecorder {
    async fn step(&self, _: &str, _: &Run, _: &Step) {}
    async fn stopped(&self, _: &str, _: &Run, _: &Stop) {
        std::future::pending::<()>().await
    }
}

/// ne disait jamais l'arrêt : un run sans `Stopped`.
pub fn un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret() {
    let (h, log) = harness(Limits::default(), transport(vec![]), ok_tools());
    let h = h
        .with_recorder(Rc::new(SlowRecorder))
        .with_clock(Rc::new(NoTimer));
    {
        let mut fut = Box::pin(h.send("go"));
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..50 {
            // Sans minuteur (`NoTimer`), la borne de 2 s de l'enregistreur ne coupe pas : le run
            // est bien en vol au lâcher. Si un minuteur le terminait avant, la garantie testée
            // resterait la même (busy rendu, un seul Stopped) quelle que soit la raison d'arrêt.
            if fut.as_mut().poll(&mut cx).is_ready() {
                break;
            }
        }
    }
    // Quelle que soit la raison d'arrêt (fin normale avant le lâcher, ou abandon) :
    // la place est rendue et le run n'est pas resté muet.
    assert!(!h.handle().busy());
    assert_eq!(stops(&log), 1, "un Stopped exactement");
}

pub fn un_transport_muet_en_boucle_est_borne() {
    let t = Rc::new(Scripted {
        acts: RefCell::new(VecDeque::new()),
        fallback: Box::new(|_, _| Ok(say(""))),
    });
    let (h, log) = harness(Limits::default(), t, ok_tools());
    bounded(h.send("go"));
    assert_eq!(stops(&log), 1);
}

pub fn mille_appels_dans_un_tour_restent_ranges() {
    let t = transport(vec![Box::new(|_, _| {
        Ok(Reply {
            text: String::new(),
            calls: (0..1000)
                .map(|i| ToolCall {
                    id: format!("c{i}"),
                    name: "t".into(),
                    arguments: json!({}),
                })
                .collect(),
            finish: Finish::Tools,
        })
    })]);
    let (h, log) = harness(Limits::default(), t, ok_tools());
    bounded(h.send("go"));
    assert!(thread_is_valid(&h.transcript()));
    assert_eq!(stops(&log), 1);
}

pub fn interrupt_avant_le_run_n_empoisonne_pas_le_suivant() {
    let (h, _l) = harness(Limits::default(), transport(vec![]), ok_tools());
    let hd = h.handle();
    hd.interrupt();
    hd.interrupt();
    assert!(matches!(bounded(h.send("go")), Running::Ran(Stop::Done)));
    assert!(!hd.is_cancelled());
}

pub fn interrupt_pendant_un_outil_puis_nouveau_send() {
    let hd_cell: Rc<RefCell<Option<this_harness::Handle>>> = Rc::default();
    let c = hd_cell.clone();
    let t = transport(vec![Box::new(move |_, _| {
        c.borrow().as_ref().unwrap().interrupt();
        Ok(call("c1"))
    })]);
    let (h, _l) = harness(Limits::default(), t, ok_tools());
    *hd_cell.borrow_mut() = Some(h.handle());
    assert!(matches!(
        bounded(h.send("a")),
        Running::Ran(Stop::Cancelled)
    ));
    assert!(thread_is_valid(&h.transcript()));
    assert!(matches!(bounded(h.send("b")), Running::Ran(Stop::Done)));
}
