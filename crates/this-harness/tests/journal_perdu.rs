//! #284 : un pas de journal ABANDONNÉ (panique ou délai dépassé) est signalé à l'hôte.
//!
//! `Recorder` reste best-effort : son échec ne doit pas arrêter le run. Mais le perdre
//! SANS RIEN DIRE laisserait croire que le journal est complet. Chaque pas perdu émet un
//! `Hint` ; un enregistreur sain n'en émet aucun.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use serde_json::json;

use this_agent::{Run, Step, Stop};
use this_harness::{
    Cancel, Clock, Delta, Event, Failure, Finish, Harness, Limits, Outcome, Permission, Recorder,
    Reply, Request, Running, ToolCall, ToolSpec, Toolbox, Transport,
};

/// Demande l'outil, puis conclut : une étape d'outil, puis un arrêt.
struct Moteur(Cell<u32>);

#[async_trait(?Send)]
impl Transport for Moteur {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.0.set(self.0.get() + 1);
        Ok(if self.0.get() % 2 == 1 {
            Reply {
                served: None,
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
                served: None,
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
        vec![ToolSpec {
            name: "lire".into(),
            description: "d".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("ok")
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Sain,
    Panique,
    Muet,
}

struct Enregistreur(Mode);

#[async_trait(?Send)]
impl Recorder for Enregistreur {
    async fn step(&self, _: &str, _: &Run, _: &Step) {
        self.agir().await
    }
    async fn stopped(&self, _: &str, _: &Run, _: &Stop) {
        self.agir().await
    }
}

impl Enregistreur {
    async fn agir(&self) {
        match self.0 {
            Mode::Sain => {}
            Mode::Panique => panic!("enregistreur défaillant"),
            Mode::Muet => std::future::pending::<()>().await,
        }
    }
}

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

/// Une horloge dont le minuteur expire au prochain poll : le délai « passe » aussitôt.
struct Factice(Cell<f64>);

impl Clock for Factice {
    fn now_ms(&self) -> f64 {
        self.0.get()
    }
    fn sleep(&self, ms: u64) -> Option<this_harness::Sleep> {
        self.0.set(self.0.get() + ms as f64);
        Some(Box::pin(Cede(false)))
    }
}

/// Joue un run avec cet enregistreur et rend le texte des `Hint` émis.
fn hints_pour(mode: Mode) -> (Running, Vec<String>) {
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = events.clone();
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(Moteur(Cell::new(0))),
        Rc::new(Lire),
        move |e| sink.borrow_mut().push(e),
    )
    .with_clock(Rc::new(Factice(Cell::new(0.0))))
    .with_recorder(Rc::new(Enregistreur(mode)));
    let fin = futures::executor::block_on(harness.send("lis"));
    let textes = events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            Event::Hint { text, .. } if text.starts_with("journal") => Some(text.clone()),
            _ => None,
        })
        .collect();
    (fin, textes)
}

#[test]
fn un_enregistreur_qui_panique_est_signale_pour_chaque_pas_perdu() {
    let (fin, hints) = hints_pour(Mode::Panique);
    assert_eq!(fin, Running::Ran(Stop::Done), "le run continue quand même");
    assert_eq!(hints.len(), 2, "une étape et un arrêt perdus : {hints:?}");
    assert!(hints.iter().any(|h| h.contains("étape")), "{hints:?}");
    assert!(hints.iter().any(|h| h.contains("arrêt")), "{hints:?}");
    assert!(hints.iter().all(|h| h.contains("panique")), "{hints:?}");
}

#[test]
fn un_enregistreur_muet_est_signale_comme_delai_depasse() {
    let (fin, hints) = hints_pour(Mode::Muet);
    assert_eq!(fin, Running::Ran(Stop::Done), "le run continue quand même");
    assert_eq!(hints.len(), 2, "une étape et un arrêt perdus : {hints:?}");
    assert!(hints.iter().all(|h| h.contains("délai")), "{hints:?}");
}

/// Témoin : un enregistreur sain ne fabrique aucun signalement.
#[test]
fn un_enregistreur_sain_n_emet_aucun_signalement_de_journal() {
    let (fin, hints) = hints_pour(Mode::Sain);
    assert_eq!(fin, Running::Ran(Stop::Done));
    assert!(hints.is_empty(), "rien n'est perdu : {hints:?}");
}

/// Un moteur de la famille A : il exécute son outil LUI-MÊME pendant le flux et le déclare
/// à la porte (`Permission::record`). C'est un chemin distinct de la boucle : le pas perdu
/// doit y être signalé aussi.
struct MoteurInterne {
    permission: RefCell<Option<Rc<dyn Permission>>>,
}

#[async_trait(?Send)]
impl Transport for MoteurInterne {
    fn set_permission(&self, permission: Rc<dyn Permission>) {
        *self.permission.borrow_mut() = Some(permission);
    }

    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let permission = self.permission.borrow().clone().expect("porte posée");
        let call = ToolCall {
            id: "t".into(),
            name: "lire".into(),
            arguments: json!({}),
        };
        permission.record(&call, 2, &Outcome::ok("ok")).await;
        Ok(Reply {
            served: None,
            text: "fini".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

#[test]
fn un_pas_perdu_sur_le_chemin_de_la_famille_a_est_signale_aussi() {
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = events.clone();
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(MoteurInterne {
            permission: RefCell::new(None),
        }),
        Rc::new(Lire),
        move |e| sink.borrow_mut().push(e),
    )
    .with_clock(Rc::new(Factice(Cell::new(0.0))))
    .with_recorder(Rc::new(Enregistreur(Mode::Panique)));

    futures::executor::block_on(harness.send("lis"));

    let etapes = events
        .borrow()
        .iter()
        .filter(|e| matches!(e, Event::Hint { text, .. } if text.contains("perte d'étape")))
        .count();
    assert_eq!(
        etapes,
        1,
        "l'étape que le moteur a jouée lui-même est signalée perdue : {:?}",
        events.borrow()
    );
}
