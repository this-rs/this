//! #285 : un délai exigé sur un hôte SANS minuteur ne doit jamais être annoncé comme appliqué.
//!
//! `Limits::default()` demande un délai (`silence_ms`), mais une horloge sans minuteur ne peut
//! pas le tenir. Décision enregistrée : garantie DÉGRADÉE EXPLICITE — le run continue, et un
//! signal dit que le délai n'est pas appliqué (on ne refuse pas chaque run d'un hôte wasm).

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use async_trait::async_trait;

use this_harness::{
    Cancel, Clock, Delta, Event, Failure, Finish, Harness, HintKind, Limits, Outcome, Reply,
    Request, Running, Sleep, Stop, ToolSpec, Toolbox, Transport,
};

struct Repond;

#[async_trait(?Send)]
impl Transport for Repond {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        Ok(Reply {
            served: None,
            text: "ok".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

struct Vide;

#[async_trait(?Send)]
impl Toolbox for Vide {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![]
    }
    async fn call(&self, _: &this_harness::ToolCall) -> Outcome {
        Outcome::ok("")
    }
}

/// Une horloge sans minuteur : `sleep` ne rend rien, comme wasm32 sans injection.
struct SansMinuteur;

impl Clock for SansMinuteur {
    fn now_ms(&self) -> f64 {
        0.0
    }
    fn sleep(&self, _: u64) -> Option<Sleep> {
        None
    }
    fn has_timer(&self) -> bool {
        false
    }
}

/// Une horloge qui a un minuteur (le défaut du trait) : aucun signal ne doit sortir.
struct AvecMinuteur;

struct Immediat;
impl Future for Immediat {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }
}

impl Clock for AvecMinuteur {
    fn now_ms(&self) -> f64 {
        0.0
    }
    fn sleep(&self, _: u64) -> Option<Sleep> {
        Some(Box::pin(Immediat))
    }
}

fn jouer(horloge: Rc<dyn Clock>) -> (Running, Vec<String>) {
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = events.clone();
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(Repond),
        Rc::new(Vide),
        move |e| sink.borrow_mut().push(e),
    )
    .with_clock(horloge);
    let fin = futures::executor::block_on(harness.send("bonjour"));
    let textes = events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            Event::Hint {
                text,
                kind: HintKind::Other,
            } => Some(text.clone()),
            _ => None,
        })
        .collect();
    (fin, textes)
}

#[test]
fn sans_minuteur_le_delai_non_applique_est_signale_et_le_run_continue() {
    let (fin, hints) = jouer(Rc::new(SansMinuteur));
    assert_eq!(
        fin,
        Running::Ran(Stop::Done),
        "garantie dégradée : le run continue"
    );
    assert!(
        hints
            .iter()
            .any(|h| h.contains("minuteur") && h.contains("délai")),
        "le délai non appliqué doit être dit à l'hôte : {hints:?}"
    );
}

#[test]
fn avec_minuteur_aucun_signal_de_delai_non_applique() {
    let (fin, hints) = jouer(Rc::new(AvecMinuteur));
    assert_eq!(fin, Running::Ran(Stop::Done));
    assert!(
        !hints.iter().any(|h| h.contains("minuteur")),
        "rien à signaler quand le délai est tenu : {hints:?}"
    );
}
