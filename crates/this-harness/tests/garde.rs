//! Une session honnête : elle n'annonce jamais un succès qu'elle n'a pas eu, et
//! elle ne reste jamais bloquée sur un transport qui ne rend pas la main.
//!
//! Le temps est celui d'une horloge factice : chaque attente avance l'heure de
//! sa durée et cède la main une fois, comme un vrai minuteur — dix secondes de
//! grâce ou deux minutes de silence se jouent en quelques microsecondes.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use serde_json::json;

use this_harness::{
    Cancel, Clock, Code, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome, Reply,
    Request, Running, Sleep, Stop, ToolCall, ToolSpec, Toolbox, Transport,
};

// ---- L'horloge factice -------------------------------------------------------

#[derive(Default)]
struct FakeClock {
    now: Cell<f64>,
}

/// Cède la main une fois, puis rend : l'autre branche de la course est sondée.
struct YieldOnce(bool);

impl Future for YieldOnce {
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

impl Clock for FakeClock {
    fn now_ms(&self) -> f64 {
        self.now.get()
    }
    fn sleep(&self, ms: u64) -> Option<Sleep> {
        self.now.set(self.now.get() + ms as f64);
        Some(Box::pin(YieldOnce(false)))
    }
}

// ---- Le moteur scripté -------------------------------------------------------

enum Act {
    /// Répond tout de suite.
    Reply(Reply),
    /// Échoue tout de suite.
    Fail(Failure),
    /// Montre un morceau de texte, puis échoue.
    SayThenFail(&'static str, Failure),
    /// Montre éventuellement un morceau, s'annule éventuellement, puis ne rend
    /// plus jamais la main.
    Hang {
        say: Option<&'static str>,
        cancel: bool,
    },
    /// Parle par morceaux espacés de `gap_ms`, puis conclut.
    Slow {
        parts: Vec<&'static str>,
        gap_ms: u64,
    },
}

struct Engine {
    acts: RefCell<VecDeque<Act>>,
    calls: Cell<u32>,
    clock: Rc<FakeClock>,
}

impl Engine {
    fn new(clock: Rc<FakeClock>, acts: Vec<Act>) -> Rc<Self> {
        Rc::new(Self {
            acts: RefCell::new(acts.into()),
            calls: Cell::new(0),
            clock,
        })
    }
}

fn reply(text: &str) -> Reply {
    Reply {
        text: text.to_string(),
        calls: vec![],
        finish: Finish::Stop,
    }
}

#[async_trait(?Send)]
impl Transport for Engine {
    async fn stream(
        &self,
        _request: &Request,
        cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        self.calls.set(self.calls.get() + 1);
        let act = self
            .acts
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Act::Reply(reply("fini")));
        match act {
            Act::Reply(reply) => {
                delta(&reply.text);
                Ok(reply)
            }
            Act::Fail(failure) => Err(failure),
            Act::SayThenFail(text, failure) => {
                delta(text);
                Err(failure)
            }
            Act::Hang { say, cancel: stop } => {
                if let Some(text) = say {
                    delta(text);
                }
                if stop {
                    cancel.cancel();
                }
                std::future::pending::<()>().await;
                unreachable!()
            }
            Act::Slow { parts, gap_ms } => {
                let mut said = String::new();
                for part in parts {
                    self.clock.sleep(gap_ms).expect("minuteur").await;
                    delta(part);
                    said.push_str(part);
                }
                Ok(reply(&said))
            }
        }
    }
}

struct Tools;

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "install_module".to_string(),
            description: "installe un module".to_string(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _call: &ToolCall) -> Outcome {
        Outcome::ok("installé")
    }
}

type Log = Rc<RefCell<Vec<Event>>>;

fn build(clock: &Rc<FakeClock>, engine: &Rc<Engine>, limits: Limits) -> (Harness, Log) {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let seen = log.clone();
    let harness = Harness::new(
        "tu es l'atelier",
        limits,
        engine.clone(),
        Rc::new(Tools),
        move |event| seen.borrow_mut().push(event),
    )
    .with_clock(clock.clone());
    (harness, log)
}

fn run<F: Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

fn stopped(log: &Log) -> Vec<(Stop, Option<String>)> {
    log.borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Stopped { stop, code, .. } => Some((stop.clone(), code.clone())),
            _ => None,
        })
        .collect()
}

fn hints(log: &Log) -> Vec<String> {
    log.borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Hint { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ---- Rien à faire n'est pas « terminé » --------------------------------------

#[test]
fn un_pump_sans_objectif_n_annonce_pas_termine() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(clock.clone(), vec![]);
    let (harness, log) = build(&clock, &engine, Limits::default());

    // Un message vide n'entre pas dans la file : aucun objectif ne s'ouvre.
    assert_eq!(run(harness.send("   ")), Running::Idle);
    // Une reprise sur une file vide ne fait rien non plus.
    assert_eq!(run(harness.resume()), None);

    assert!(stopped(&log).is_empty(), "aucun Stopped(Done) inventé");
    assert_eq!(engine.calls.get(), 0, "aucun tour n'a eu lieu");
    assert!(!harness.handle().busy(), "la session est libre");

    // Et la session sert encore : le message suivant tourne normalement.
    assert_eq!(run(harness.send("bonjour")), Running::Ran(Stop::Done));
}

// ---- La grâce après une annulation -------------------------------------------

#[test]
fn un_transport_sourd_a_l_annulation_est_abandonne_apres_la_grace() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(
        clock.clone(),
        vec![Act::Hang {
            say: Some("je commence "),
            cancel: true,
        }],
    );
    let (harness, log) = build(&clock, &engine, Limits::default());
    let started = clock.now_ms();

    assert_eq!(run(harness.send("parle")), Running::Ran(Stop::Interrupted));
    assert_eq!(
        stopped(&log),
        vec![(Stop::Interrupted, Some("aborted".to_string()))]
    );
    let waited = clock.now_ms() - started;
    assert!(
        (10_000.0..12_000.0).contains(&waited),
        "abandon après la grâce de 10 s, pas avant : {waited} ms"
    );
    assert!(
        hints(&log).iter().any(|h| h.contains("annulation")),
        "{:?}",
        hints(&log)
    );
    assert!(!harness.handle().busy(), "la session est libre");
}

// ---- Le chien de garde du silence --------------------------------------------

#[test]
fn un_flux_muet_est_interrompu_en_disant_pourquoi() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(
        clock.clone(),
        vec![Act::Hang {
            say: Some("bonjour "),
            cancel: false,
        }],
    );
    let (harness, log) = build(&clock, &engine, Limits::default());

    assert_eq!(run(harness.send("parle")), Running::Ran(Stop::Interrupted));
    assert_eq!(
        stopped(&log),
        vec![(Stop::Interrupted, Some("timeout".to_string()))]
    );
    let hint = hints(&log).join("\n");
    assert!(
        hint.contains("Aucun événement du flux depuis 120 s"),
        "{hint}"
    );
    // Ce qui a été montré reste dans le fil, sans appel orphelin.
    assert_eq!(
        harness.transcript().last(),
        Some(&Message::Assistant {
            content: "bonjour ".to_string(),
            calls: vec![]
        })
    );
    assert!(clock.now_ms() >= 120_000.0);
}

#[test]
fn le_silence_tolere_se_regle_dans_les_bornes() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(
        clock.clone(),
        vec![Act::Hang {
            say: None,
            cancel: false,
        }],
    );
    let limits = Limits {
        silence_ms: Some(5_000),
        ..Limits::default()
    };
    let (harness, log) = build(&clock, &engine, limits);

    assert_eq!(run(harness.send("parle")), Running::Ran(Stop::Interrupted));
    assert!(hints(&log).iter().any(|h| h.contains("depuis 5 s")));
    assert!(clock.now_ms() < 7_000.0, "{}", clock.now_ms());
}

#[test]
fn un_flux_lent_mais_vivant_n_est_pas_coupe() {
    let clock = Rc::new(FakeClock::default());
    // Trois morceaux espacés de 60 s : trois minutes au total, mais jamais deux
    // minutes de silence.
    let engine = Engine::new(
        clock.clone(),
        vec![Act::Slow {
            parts: vec!["un ", "deux ", "trois"],
            gap_ms: 60_000,
        }],
    );
    let limits = Limits {
        budget_ms: None,
        ..Limits::default()
    };
    let (harness, log) = build(&clock, &engine, limits);

    assert_eq!(run(harness.send("parle")), Running::Ran(Stop::Done));
    assert!(clock.now_ms() >= 180_000.0);
    assert!(hints(&log).is_empty(), "{:?}", hints(&log));
}

#[test]
fn l_horloge_du_systeme_arme_vraiment_la_garde() {
    // Sans horloge factice : le minuteur natif (un fil) doit réveiller la boucle.
    struct Deaf;
    #[async_trait(?Send)]
    impl Transport for Deaf {
        async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let seen = log.clone();
    let harness = Harness::new(
        "s",
        Limits {
            silence_ms: Some(50),
            ..Limits::default()
        },
        Rc::new(Deaf),
        Rc::new(Tools),
        move |event| seen.borrow_mut().push(event),
    );
    let started = std::time::Instant::now();
    assert_eq!(run(harness.send("parle")), Running::Ran(Stop::Interrupted));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

// ---- La relance bornée -------------------------------------------------------

#[test]
fn une_limite_de_debit_sans_rien_vu_est_relancee() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(
        clock.clone(),
        vec![
            Act::Fail(Failure::http(429)),
            Act::Fail(Failure::new(Code::Timeout, "délai dépassé")),
        ],
    );
    let (harness, log) = build(&clock, &engine, Limits::default());

    assert_eq!(run(harness.send("parle")), Running::Ran(Stop::Done));
    assert_eq!(engine.calls.get(), 3, "deux relances, puis la réponse");
    let hints = hints(&log);
    assert_eq!(hints.len(), 2, "{hints:?}");
    assert!(
        hints[0].contains("1/3") && hints[1].contains("2/3"),
        "{hints:?}"
    );
    // Backoff : 1 s puis 2 s.
    assert!(clock.now_ms() >= 3_000.0, "{}", clock.now_ms());
    // Un seul tour : la relance rejoue le même tour.
    let turns = log
        .borrow()
        .iter()
        .filter(|e| matches!(e, Event::Turn { .. }))
        .count();
    assert_eq!(turns, 1);
}

#[test]
fn la_relance_est_bornee_a_trois() {
    let clock = Rc::new(FakeClock::default());
    let timeout = || Act::Fail(Failure::new(Code::Timeout, "délai dépassé"));
    let engine = Engine::new(
        clock.clone(),
        vec![timeout(), timeout(), timeout(), timeout(), timeout()],
    );
    let (harness, log) = build(&clock, &engine, Limits::default());

    match run(harness.send("parle")) {
        Running::Ran(Stop::Failed(motif)) => assert!(motif.contains("délai"), "{motif}"),
        autre => panic!("attendu Failed, obtenu {autre:?}"),
    }
    assert_eq!(engine.calls.get(), 4, "un essai et trois relances");
    assert_eq!(hints(&log).len(), 3);
    assert_eq!(stopped(&log)[0].1.as_deref(), Some("timeout"));
}

#[test]
fn aucune_relance_apres_du_texte_montre() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(
        clock.clone(),
        vec![Act::SayThenFail("je commence ", Failure::http(429))],
    );
    let (harness, log) = build(&clock, &engine, Limits::default());

    assert!(matches!(
        run(harness.send("parle")),
        Running::Ran(Stop::Failed(_))
    ));
    assert_eq!(engine.calls.get(), 1, "un texte déjà lu ne se rejoue pas");
    assert!(hints(&log).is_empty(), "{:?}", hints(&log));
}

#[test]
fn aucune_relance_apres_un_outil() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(
        clock.clone(),
        vec![
            Act::Reply(Reply {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "a1".to_string(),
                    name: "install_module".to_string(),
                    arguments: json!({}),
                }],
                finish: Finish::Tools,
            }),
            Act::Fail(Failure::new(Code::Timeout, "délai dépassé")),
        ],
    );
    let (harness, log) = build(&clock, &engine, Limits::default());

    assert!(matches!(
        run(harness.send("installe")),
        Running::Ran(Stop::Failed(_))
    ));
    assert_eq!(
        engine.calls.get(),
        2,
        "un effet déjà produit ne se rejoue pas"
    );
    assert!(
        !hints(&log).iter().any(|h| h.contains("tentative")),
        "{:?}",
        hints(&log)
    );
}

#[test]
fn une_panne_durable_n_est_pas_relancee() {
    let clock = Rc::new(FakeClock::default());
    let engine = Engine::new(clock.clone(), vec![Act::Fail(Failure::http(503))]);
    let (harness, log) = build(&clock, &engine, Limits::default());

    assert!(matches!(
        run(harness.send("parle")),
        Running::Ran(Stop::Failed(_))
    ));
    assert_eq!(engine.calls.get(), 1);
    assert!(hints(&log).is_empty());
}
