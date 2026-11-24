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
        usage: None,
        served: None,
        text: text.into(),
        calls: vec![],
        finish: Finish::Stop,
    }
}
pub fn call(id: &str) -> Reply {
    Reply {
        usage: None,
        served: None,
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

pub struct SlowRecorder(pub Rc<Cell<bool>>);
#[async_trait(?Send)]
impl Recorder for SlowRecorder {
    async fn step(&self, _: &str, _: &Run, _: &Step) {}
    async fn stopped(&self, _: &str, _: &Run, _: &Stop) {
        self.0.set(true);
        std::future::pending::<()>().await
    }
}

/// ne disait jamais l'arrêt : un run sans `Stopped`.
pub fn un_run_lache_pendant_l_enregistreur_dit_quand_meme_son_arret() {
    let (h, log) = harness(Limits::default(), transport(vec![]), ok_tools());
    let entered = Rc::new(Cell::new(false));
    let h = h
        .with_recorder(Rc::new(SlowRecorder(entered.clone())))
        .with_clock(Rc::new(NoTimer));
    {
        let mut fut = Box::pin(h.send("go"));
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..50 {
            assert!(
                fut.as_mut().poll(&mut cx).is_pending(),
                "le run ne doit pas finir avant le lâcher"
            );
            if entered.get() {
                break;
            }
        }
        assert!(entered.get(), "Recorder::stopped doit avoir été atteint");
    }
    // Le run a bien été lâché pendant l'attente de Recorder::stopped.
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
    // « Borné » : ce que le test annonçait sans l'affirmer. Le run s'arrête EN ÉCHEC, avec la
    // raison nommée, et après un nombre fini de tours — pas seulement « il a fini ». Une
    // mutation qui ferait rendre `Stop::Done` à un tour muet laissait l'ancienne version verte.
    let arret = log
        .borrow()
        .iter()
        .find_map(|e| match e {
            Event::Stopped { stop, turns, .. } => Some((stop.clone(), *turns)),
            _ => None,
        })
        .expect("un événement Stopped");
    assert_eq!(
        arret.0,
        Stop::Failed("le modèle n'a rendu ni texte ni outil".to_string()),
        "un transport muet n'est ni un succès ni un budget : c'est un échec nommé"
    );
    // Le nombre EXACT, relevé puis compté d'après le code : le tour muet initial (1), puis une
    // relance par `Limits::default().max_relances` (3 : `session.rs`, copié dans le `Run` à
    // chaque envoi), puis au 4e tour muet le compteur de relances est épuisé et le run s'arrête.
    // Soit 1 + 3 = 4, et non 3 : la politique propre de `this-agent` accorde 2 relances, mais le
    // harnais la remplace par `Limits::max_relances`. Une borne `<=` laissait passer une
    // mutation qui relancerait une fois de plus ou de moins.
    assert_eq!(
        arret.1,
        1 + Limits::default().max_relances,
        "tour muet initial + une relance par `max_relances`, puis l'arrêt (attendu 4)"
    );
    assert_eq!(
        Limits::default().max_relances,
        3,
        "le décompte ci-dessus suppose 3 relances"
    );
}

pub fn mille_appels_dans_un_tour_restent_ranges() {
    let t = transport(vec![Box::new(|_, _| {
        Ok(Reply {
            usage: None,
            served: None,
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
    let thread = h.transcript();
    assert!(thread_is_valid(&thread));
    assert_eq!(
        thread
            .iter()
            .map(|m| match m {
                Message::Assistant { calls, .. } => calls.len(),
                _ => 0,
            })
            .sum::<usize>(),
        1000
    );
    assert_eq!(
        thread
            .iter()
            .filter(|m| matches!(m, Message::Tool { .. }))
            .count(),
        1000
    );
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
    struct PendingTool(Rc<Cell<bool>>);
    #[async_trait(?Send)]
    impl Toolbox for PendingTool {
        fn specs(&self) -> Vec<ToolSpec> {
            ok_tools().specs()
        }
        async fn call(&self, _: &ToolCall) -> Outcome {
            self.0.set(true);
            std::future::pending().await
        }
    }
    let entered = Rc::new(Cell::new(false));
    let t = transport(vec![Box::new(|_, _| {
        Ok(Reply {
            usage: None,
            served: None,
            text: String::new(),
            calls: vec![call("c1").calls.remove(0), call("c2").calls.remove(0)],
            finish: Finish::Tools,
        })
    })]);
    let h = Harness::new(
        "sys",
        Limits::default(),
        t,
        Rc::new(PendingTool(entered.clone())),
        |_| {},
    )
    .with_clock(Rc::new(Fake::default()));
    let hd = h.handle();
    let mut fut = Box::pin(h.send("a"));
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..POLLS {
        assert!(
            fut.as_mut().poll(&mut cx).is_pending(),
            "le run ne doit pas finir avant l'interruption"
        );
        if entered.get() {
            break;
        }
    }
    assert!(entered.get(), "l'outil doit être entré avant interruption");
    hd.interrupt();
    let result = bounded(fut);
    assert!(
        matches!(result, Running::Ran(Stop::Interrupted)),
        "{result:?}"
    );
    let thread = h.transcript();
    assert!(thread_is_valid(&thread), "{thread:?}");
    assert_eq!(
        thread
            .iter()
            .filter(|m| matches!(m, Message::Tool { .. }))
            .count(),
        2,
        "les deux appels ont une réponse"
    );
    assert!(matches!(bounded(h.send("b")), Running::Ran(Stop::Done)));
}
