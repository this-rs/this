//! Les résidus d'une passe adverse : un composant du hôte qui panique ou se tait ne doit pas
//! priver les autres, ni figer le run ; une annulation ne se perd pas entre deux runs.

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use serde_json::json;

use this_agent::{Run, Step, Stop};
use this_harness::{
    Cancel, Clock, Delta, Failure, Finish, Harness, Limits, Outcome, RECORDER_TIMEOUT_MS, Recorder,
    Reply, Request, Running, ToolCall, ToolSpec, Toolbox, Toolboxes, Transport,
};

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "d".into(),
        parameters: json!({"type": "object"}),
    }
}

/// Un outil au premier tour, puis la conclusion.
struct Engine(Cell<u32>);

#[async_trait(?Send)]
impl Transport for Engine {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.0.set(self.0.get() + 1);
        Ok(if self.0.get() % 2 == 1 {
            Reply {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "t".into(),
                    name: "lire".into(),
                    arguments: json!({}),
                }],
                finish: Finish::Tools,
            }
        } else {
            Reply {
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            }
        })
    }
}

struct Lire;

#[async_trait(?Send)]
impl Toolbox for Lire {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![spec("lire")]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("ok")
    }
}

// ---- 1. specs() qui panique -------------------------------------------------

struct Panique;

#[async_trait(?Send)]
impl Toolbox for Panique {
    fn specs(&self) -> Vec<ToolSpec> {
        panic!("specs défaillantes")
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("jamais")
    }
}

#[test]
fn a_part_whose_specs_panic_does_not_block_the_others() {
    let boxes = Toolboxes::new().with(Rc::new(Panique)).with(Rc::new(Lire));
    let names: Vec<_> = boxes.specs().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["lire"], "la part fautive est ignorée");
    boxes.declarations(); // ne panique pas non plus
    let out = futures::executor::block_on(boxes.call(&ToolCall {
        id: "1".into(),
        name: "lire".into(),
        arguments: json!({}),
    }));
    assert_eq!(out.text, "ok", "le routage des autres parts survit");
}

// ---- 2. Recorder qui panique / se tait ---------------------------------------

#[derive(Clone, Copy)]
enum Mode {
    Panique,
    Muet,
}

struct Mauvais(Mode);

#[async_trait(?Send)]
impl Recorder for Mauvais {
    async fn step(&self, _: &str, _: &Run, _: &Step) {
        self.agir().await
    }
    async fn stopped(&self, _: &str, _: &Run, _: &Stop) {
        self.agir().await
    }
}

impl Mauvais {
    async fn agir(&self) {
        match self.0 {
            Mode::Panique => panic!("enregistreur défaillant"),
            Mode::Muet => std::future::pending::<()>().await,
        }
    }
}

/// Un minuteur qui cède une fois puis échoit, et fait avancer l'heure du temps demandé.
struct Factice(Cell<f64>);

struct Cede(bool);
impl Future for Cede {
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

impl Clock for Factice {
    fn now_ms(&self) -> f64 {
        self.0.get()
    }
    fn sleep(&self, ms: u64) -> Option<this_harness::Sleep> {
        self.0.set(self.0.get() + ms as f64);
        Some(Box::pin(Cede(false)))
    }
}

fn run_with(recorder: Mauvais) -> (Running, f64) {
    let clock = Rc::new(Factice(Cell::new(0.0)));
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(Engine(Cell::new(0))),
        Rc::new(Lire),
        |_| {},
    )
    .with_clock(clock.clone())
    .with_recorder(Rc::new(recorder));
    let out = futures::executor::block_on(harness.send("lis"));
    (out, clock.0.get())
}

#[test]
fn a_panicking_recorder_does_not_stop_the_run() {
    let (out, _) = run_with(Mauvais(Mode::Panique));
    assert_eq!(out, Running::Ran(Stop::Done));
}

#[test]
fn a_silent_recorder_delays_the_run_by_a_bounded_time() {
    let (out, elapsed) = run_with(Mauvais(Mode::Muet));
    assert_eq!(out, Running::Ran(Stop::Done));
    // Deux appels (une étape, un arrêt), chacun borné : jamais plus.
    assert!(
        elapsed <= 2.0 * RECORDER_TIMEOUT_MS as f64 + 5_000.0,
        "le run n'attend pas un enregistreur muet ({elapsed} ms)"
    );
}

// ---- 3. annulation posée au repos (ignorée) --------------------------------------

#[test]
fn an_interrupt_between_two_runs_is_ignored_and_does_not_poison_the_next_one() {
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(Engine(Cell::new(0))),
        Rc::new(Lire),
        |_| {},
    );
    assert_eq!(
        futures::executor::block_on(harness.send("un")),
        Running::Ran(Stop::Done)
    );
    harness.handle().interrupt(); // entre deux runs
    assert_eq!(
        futures::executor::block_on(harness.send("deux")),
        Running::Ran(Stop::Done),
        "décision : une annulation au repos est ignorée, elle n'empoisonne pas le run suivant"
    );
    // Et le suivant non plus.
    assert_eq!(
        futures::executor::block_on(harness.send("trois")),
        Running::Ran(Stop::Done)
    );
}
