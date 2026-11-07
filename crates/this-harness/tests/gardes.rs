//! Les garde-fous du harnais, chacun mesuré avant d'être corrigé.
//!
//! Chaque test est écrit pour ÉCHOUER sans son correctif : une future lâchée
//! doit rendre la place, un puits réentrant ne doit pas paniquer, un budget de
//! temps doit compter les tours du modèle, une politique branchée doit rester
//! bornée, une annulation doit atteindre les outils, et le code d'une panne
//! doit être la forme exacte que serde écrit sur le fil.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use serde_json::json;

use this_agent::{Decision, Policy, Run as AgentRun, Stop};
use this_harness::{
    Cancel, Code, Delta, Event, Failure, Finish, Handle, Harness, Limits, Outcome, Reply, Request,
    Running, ToolCall, ToolSpec, Toolbox, Transport,
};

// ---- Les doublures -----------------------------------------------------------

/// Une boîte à outils vide : aucun outil. Sert aux tests qui n'en jouent aucun.
struct Rien;

#[async_trait(?Send)]
impl Toolbox for Rien {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("rien")
    }
}

/// Un outil instantané : il répond sans attendre, pour isoler le temps du
/// modèle de celui des outils.
struct OutilInstant;

#[async_trait(?Send)]
impl Toolbox for OutilInstant {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "install_module".into(),
            description: "installe un module".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("ok")
    }
}

/// Un outil qui attend quelque chose (une permission, un `fetch`) : il surcharge
/// `call_with_cancel` pour s'arrêter quand `cancel` est posé, et note ce qu'il a
/// reçu.
struct OutilQuiAttend {
    a_utilise_annulation: Rc<Cell<bool>>,
    a_recu_annulation: Rc<Cell<bool>>,
}

#[async_trait(?Send)]
impl Toolbox for OutilQuiAttend {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "attendre".into(),
            description: "un outil qui attend".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        // L'ancien chemin : sans annulation, un outil qui attend ne rend jamais
        // la main. Il ne doit plus être emprunté par le harnais.
        Outcome::ok("l'outil attend toujours")
    }
    async fn call_with_cancel(&self, _: &ToolCall, cancel: &Cancel) -> Outcome {
        self.a_utilise_annulation.set(true);
        self.a_recu_annulation.set(cancel.is_cancelled());
        if cancel.is_cancelled() {
            Outcome::failed("annulé", "l'outil s'est arrêté à l'annulation")
        } else {
            Outcome::ok("l'outil attend toujours")
        }
    }
}

/// Une future qui rend `Pending` une fois, puis `Ready` : elle force le harnais
/// à rendre la main entre deux tours, pour qu'un poll borné avance d'un tour à
/// la fois au lieu de tout faire d'un bloc.
struct UnTourDeMain(bool);

impl Future for UnTourDeMain {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

// ---- Les aides ---------------------------------------------------------------

fn run<F: Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

fn poll_once<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    future.as_mut().poll(&mut cx)
}

// ---- 1. Une future lâchée rend la place -------------------------------------

/// Un transport dont le premier appel ne se résoudra jamais, et les suivants
/// concluent : le premier `send` reste suspendu, c'est le lâcher de la future
/// qui doit libérer la place.
struct LachePuisRepond(Cell<usize>);

#[async_trait(?Send)]
impl Transport for LachePuisRepond {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let n = self.0.get();
        self.0.set(n + 1);
        if n == 0 {
            std::future::pending::<Result<Reply, Failure>>().await
        } else {
            Ok(Reply {
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            })
        }
    }
}

#[test]
fn une_future_abandonnee_rend_la_place_et_le_dit() {
    let log: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = log.clone();
    let harness = Harness::new(
        "s",
        Limits::default(),
        Rc::new(LachePuisRepond(Cell::new(0))),
        Rc::new(Rien),
        move |event| seen.borrow_mut().push(event),
    );
    let handle = harness.handle();

    // Un seul poll, un waker qui ne fait rien, puis on lâche la future.
    let mut future = Box::pin(harness.send("premier"));
    assert!(
        poll_once(&mut future).is_pending(),
        "le transport ne répond pas"
    );
    drop(future);

    // La place est rendue, et l'abandon est dit.
    assert!(!handle.busy(), "busy doit retomber même sans fin normale");
    let interrompu = log.borrow().iter().any(|event| {
        matches!(
            event,
            Event::Stopped {
                stop: Stop::Interrupted,
                ..
            }
        )
    });
    assert!(interrompu, "un abandon est un Stopped {{ Interrupted }}");

    // Et un second envoi tourne vraiment : il n'est pas tenu derrière un `busy`
    // fantôme.
    assert_eq!(run(harness.send("second")), Running::Ran(Stop::Done));
}

// ---- 2. Un puits réentrant ne panique pas ------------------------------------

#[test]
fn un_puits_reentrant_ne_panique_pas() {
    struct Conclut;
    #[async_trait(?Send)]
    impl Transport for Conclut {
        async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
            Ok(Reply {
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            })
        }
    }
    let holder: Rc<RefCell<Option<Handle>>> = Rc::new(RefCell::new(None));
    let log: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
    let sink_holder = holder.clone();
    let sink_log = log.clone();
    let harness = Harness::new(
        "s",
        Limits::default(),
        Rc::new(Conclut),
        Rc::new(Rien),
        move |event| {
            // Le geste naturel sur `Stopped` : renvoyer le message tenu. Sans
            // correctif, cette émission réentrante panique (RefCell déjà
            // emprunté).
            if let Event::Stopped { .. } = &event
                && let Some(handle) = sink_holder.borrow().as_ref()
            {
                handle.enqueue("tenu");
            }
            sink_log.borrow_mut().push(event);
        },
    );
    *holder.borrow_mut() = Some(harness.handle());

    run(harness.send("parle"));

    // Aucun panic, et l'événement `Pending` arrive APRÈS le `Stopped`.
    let names: Vec<&str> = log.borrow().iter().map(Event::name).collect();
    assert_eq!(
        names[names.len() - 2],
        "stopped",
        "ordre : stopped puis pending"
    );
    assert_eq!(
        names[names.len() - 1],
        "pending",
        "l'enfilement sur Stopped émet un Pending"
    );
}

// ---- 3. Le budget de temps compte les tours du modèle ------------------------

/// Un modèle lent : chaque tour dort `duree_ms` et demande un outil.
struct Lent {
    duree_ms: u64,
    appels: Rc<Cell<usize>>,
}

#[async_trait(?Send)]
impl Transport for Lent {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.appels.set(self.appels.get() + 1);
        std::thread::sleep(std::time::Duration::from_millis(self.duree_ms));
        Ok(Reply {
            text: String::new(),
            calls: vec![ToolCall {
                id: "a1".into(),
                name: "install_module".into(),
                arguments: json!({}),
            }],
            finish: Finish::Tools,
        })
    }
}

#[test]
fn le_budget_de_temps_compte_les_tours_du_modele() {
    let appels = Rc::new(Cell::new(0));
    let transport = Lent {
        duree_ms: 20,
        appels: appels.clone(),
    };
    let harness = Harness::new(
        "s",
        Limits {
            max_tours: 40,
            max_relances: 0,
            budget_ms: Some(10),
        },
        Rc::new(transport),
        Rc::new(OutilInstant),
        |_| {},
    );

    let stop = match run(harness.send("boucle")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };
    assert_eq!(stop, Stop::Budget);
    assert!(
        appels.get() < 5,
        "le budget de temps doit arrêter avant 5 tours, pas laisser {} appels au modèle",
        appels.get()
    );
}

// ---- 4. Une politique sans fin reste bornée ----------------------------------

/// Une politique qui rend toujours `Continue`, sans jamais consulter les bornes.
struct SansBorne;

impl Policy for SansBorne {
    fn after_turn(&self, _: &mut AgentRun, _: bool, _: bool) -> Decision {
        Decision::Continue
    }
}

/// Un transport qui rend la main entre deux tours (pour le poll borné).
struct ToujoursContinue {
    appels: Rc<Cell<usize>>,
}

#[async_trait(?Send)]
impl Transport for ToujoursContinue {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.appels.set(self.appels.get() + 1);
        UnTourDeMain(false).await;
        Ok(Reply {
            text: String::new(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

#[test]
fn une_politique_sans_fin_reste_bornee_par_la_ceinture() {
    let appels = Rc::new(Cell::new(0));
    let transport = ToujoursContinue {
        appels: appels.clone(),
    };
    let harness = Harness::new(
        "s",
        Limits {
            max_tours: 3,
            max_relances: 0,
            budget_ms: None,
        },
        Rc::new(transport),
        Rc::new(Rien),
        |_| {},
    )
    .with_policy(Rc::new(SansBorne));

    // Sans la ceinture, ce run ne se résoudrait jamais (mesuré : tué à 180 s).
    // On poll un nombre borné de fois et on exige la fin.
    let mut future = Box::pin(harness.send("boucle"));
    let mut fini = None;
    for _ in 0..20 {
        match poll_once(&mut future) {
            Poll::Ready(issue) => {
                fini = Some(issue);
                break;
            }
            Poll::Pending => {}
        }
    }
    let issue = fini.expect("la ceinture doit borner la politique");
    assert_eq!(issue, Running::Ran(Stop::Budget));
    assert_eq!(
        appels.get(),
        3,
        "max_tours = 3 : trois tours, pas un de plus"
    );
}

// ---- 5. L'annulation atteint les outils --------------------------------------

#[test]
fn l_annulation_atteint_les_outils() {
    struct Demande;
    #[async_trait(?Send)]
    impl Transport for Demande {
        async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
            Ok(Reply {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "a1".into(),
                    name: "attendre".into(),
                    arguments: json!({}),
                }],
                finish: Finish::Tools,
            })
        }
    }
    let a_utilise_annulation = Rc::new(Cell::new(false));
    let a_recu_annulation = Rc::new(Cell::new(false));
    let holder: Rc<RefCell<Option<Handle>>> = Rc::new(RefCell::new(None));
    let outil = OutilQuiAttend {
        a_utilise_annulation: a_utilise_annulation.clone(),
        a_recu_annulation: a_recu_annulation.clone(),
    };
    let sink_holder = holder.clone();
    let harness = Harness::new(
        "s",
        Limits::default(),
        Rc::new(Demande),
        Rc::new(outil),
        move |event| {
            // L'interface voit l'outil démarrer et interrompt : le geste exact
            // de la touche Échap pendant qu'un outil attend.
            if let Event::ToolUse { .. } = &event
                && let Some(handle) = sink_holder.borrow().as_ref()
            {
                handle.interrupt();
            }
        },
    );
    *holder.borrow_mut() = Some(harness.handle());

    let stop = match run(harness.send("attends")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };
    assert_eq!(stop, Stop::Cancelled);
    assert!(
        a_utilise_annulation.get(),
        "call_with_cancel doit être routé"
    );
    assert!(
        a_recu_annulation.get(),
        "l'annulation doit atteindre l'outil"
    );
}

// ---- 6. Le code d'une panne est la forme exacte que serde écrit --------------

#[test]
fn code_as_str_est_exactement_ce_que_serde_ecrit() {
    // Chaque variante, explicitement : en ajouter une oblige à la déclarer ici
    // aussi — c'est le garde-fou. Si `as_str` diverge de serde (une variante à
    // deux mots écrirait `rate_limited` là où `Debug` dirait `ratelimited`), ce
    // test tombe.
    let toutes = [
        Code::Unreachable,
        Code::Http,
        Code::Truncated,
        Code::Unreadable,
        Code::Timeout,
        Code::Aborted,
        Code::Provider,
    ];
    for code in toutes {
        let ecrit = serde_json::to_string(&code).unwrap();
        assert_eq!(
            ecrit,
            format!("\"{}\"", code.as_str()),
            "`as_str()` doit être la forme exacte que serde écrit pour {code:?}"
        );
    }
}

// ---- 7. La ceinture tient aussi le budget, contre une politique qui l'efface ----

/// Une politique qui retire le budget du run et continue : exactement ce qu'une
/// politique de module peut faire avec le `&mut Run` qu'elle reçoit.
struct EffaceLeBudget;

impl Policy for EffaceLeBudget {
    fn after_turn(&self, run: &mut AgentRun, _: bool, _: bool) -> Decision {
        run.budget_ms = None;
        Decision::Continue
    }
}

/// La ceinture lit `Limits` (la composition), pas `run` (que la politique
/// reçoit en `&mut`) : sans cette distinction, une politique pouvait desserrer
/// le budget de temps tout en restant bornée sur les tours — deux bornes dont
/// une seule tenait.
#[test]
fn la_ceinture_borne_le_budget_meme_avec_une_politique_qui_l_ignore() {
    let appels = Rc::new(Cell::new(0));
    let transport = Lent {
        duree_ms: 20,
        appels: appels.clone(),
    };
    let harness = Harness::new(
        "s",
        Limits {
            max_tours: 40,
            max_relances: 0,
            budget_ms: Some(10),
        },
        Rc::new(transport),
        Rc::new(OutilInstant),
        |_| {},
    )
    .with_policy(Rc::new(EffaceLeBudget));

    let stop = match run(harness.send("boucle")) {
        Running::Ran(stop) => stop,
        autre => panic!("attendu Ran, obtenu {autre:?}"),
    };
    assert_eq!(stop, Stop::Budget, "le budget de la composition tient");
    assert!(
        appels.get() < 5,
        "la politique a beau effacer `run.budget_ms`, la ceinture arrête : {} appels",
        appels.get()
    );
}
