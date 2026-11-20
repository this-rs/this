//! Les ports mis en erreur : `Toolboxes`, `Cancel`, `Recorders`.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use futures::executor::block_on;
use serde_json::json;

use this_agent::{Run, Step, Stop};
use this_harness::{
    Cancel, Clock, Compactor, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome,
    Recorder, Reply, Request, Running, Sleep, ToolCall, ToolSpec, Toolbox, Toolboxes, Transport,
    TurnContext,
};

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: String::new(),
        parameters: json!({}),
    }
}
fn call(name: &str) -> ToolCall {
    ToolCall {
        id: "1".into(),
        name: name.into(),
        arguments: json!({}),
    }
}

/// Une part qui expose `names`, répond `who`, et compte ses `specs()`.
struct Part {
    names: Vec<String>,
    who: &'static str,
    specs_calls: Cell<usize>,
    saw_cancel: Cell<bool>,
}
impl Part {
    fn new(who: &'static str, names: &[&str]) -> Rc<Self> {
        Rc::new(Self {
            names: names.iter().map(|n| n.to_string()).collect(),
            who,
            specs_calls: Cell::new(0),
            saw_cancel: Cell::new(false),
        })
    }
}
#[async_trait(?Send)]
impl Toolbox for Part {
    fn specs(&self) -> Vec<ToolSpec> {
        self.specs_calls.set(self.specs_calls.get() + 1);
        self.names.iter().map(|n| spec(n)).collect()
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok(self.who)
    }
    async fn call_with_cancel(&self, _: &ToolCall, c: &Cancel) -> Outcome {
        self.saw_cancel.set(c.is_cancelled());
        Outcome::ok(self.who)
    }
}

/// DÉFAUT : deux parts qui déclarent le même outil → le modèle recevait deux fois le
/// même nom (un fournisseur rejette des noms d'outil dupliqués : 400 sur tout le tour).
#[test]
fn un_outil_declare_deux_fois_n_est_annonce_qu_une_fois_et_la_premiere_part_gagne() {
    let a = Part::new("a", &["x", "y"]);
    let b = Part::new("b", &["x", "z"]);
    let tb = Toolboxes::new().with(a).with(b);
    let names: Vec<String> = tb.specs().into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["x", "y", "z"], "doublon annoncé : {names:?}");
    assert_eq!(block_on(tb.call(&call("x"))).text, "a", "la première gagne");
}

/// `call` et `call_with_cancel` routent vers la même part, et le drapeau arrive.
#[test]
fn call_et_call_with_cancel_routent_pareil_et_le_drapeau_arrive() {
    let a = Part::new("a", &["x"]);
    let b = Part::new("b", &["x", "z"]);
    let tb = Toolboxes::new().with(a.clone()).with(b.clone());
    let c = Cancel::new();
    c.cancel();
    assert_eq!(block_on(tb.call(&call("x"))).text, "a");
    assert_eq!(block_on(tb.call_with_cancel(&call("x"), &c)).text, "a");
    assert!(a.saw_cancel.get(), "le drapeau du run n'est pas transmis");
    assert!(!b.saw_cancel.get());
    // Un outil inconnu est refusé, jamais une panique.
    assert!(block_on(tb.call(&call("nope"))).refused.is_some());
}

/// Le coût d'un appel : ≤ un `specs()` par part (pas de quadratique cachée), même avec
/// 10 000 outils par part, et on s'arrête à la part qui possède l'outil.
#[test]
fn un_appel_ne_relit_pas_les_specs_plus_d_une_fois_par_part() {
    let big: Vec<String> = (0..10_000).map(|i| format!("t{i}")).collect();
    let refs: Vec<&str> = big.iter().map(String::as_str).collect();
    let a = Part::new("a", &refs);
    let b = Part::new("b", &["autre"]);
    let tb = Toolboxes::new().with(a.clone()).with(b.clone());
    let t = std::time::Instant::now();
    for _ in 0..20 {
        block_on(tb.call(&call("t9999")));
    }
    assert_eq!(a.specs_calls.get(), 20);
    assert_eq!(
        b.specs_calls.get(),
        0,
        "part après le propriétaire interrogée"
    );
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    // 10 000 doublons annoncés une seule fois.
    let c = Part::new("c", &refs);
    let both = Toolboxes::new().with(a).with(c);
    assert_eq!(both.specs().len(), 10_000);
}

/// Le drapeau d'annulation : partagé par les clones, effacé par `reset`.
#[test]
fn le_drapeau_est_partage_par_les_clones_et_reset_le_remet_a_zero() {
    let c = Cancel::new();
    let gardé = c.clone(); // un transport qui garde le drapeau pour un tour suivant
    c.cancel();
    assert!(gardé.is_cancelled());
    c.reset();
    // Une annulation tardive d'un run précédent ne vaut pas pour le suivant : elle est effacée.
    assert!(!gardé.is_cancelled());
}

struct Rec(&'static str, Rc<RefCell<Vec<String>>>);
#[async_trait(?Send)]
impl Recorder for Rec {
    async fn step(&self, id: &str, _: &Run, _: &Step) {
        self.1.borrow_mut().push(format!("{}:{id}", self.0));
    }
    async fn stopped(&self, id: &str, _: &Run, _: &Stop) {
        self.1.borrow_mut().push(format!("{}:stop:{id}", self.0));
    }
}

#[test]
fn les_enregistreurs_sont_appeles_dans_l_ordre_d_ajout() {
    use this_harness::Recorders;
    let log = Rc::new(RefCell::new(Vec::new()));
    let rs = Recorders::new()
        .with(Rc::new(Rec("a", log.clone())))
        .with(Rc::new(Rec("b", log.clone())));
    let run = Run::new(this_agent::Goal::new("g"), 3);
    block_on(rs.step("r", &run, &Step::tool("t", 1)));
    block_on(rs.stopped("r", &run, &Stop::Done));
    assert_eq!(*log.borrow(), vec!["a:r", "b:r", "a:stop:r", "b:stop:r"]);
}

// ===== Ports de préparation pendus : `turn_context` et `compact` (#291) =====

const POLLS: usize = 2000;

/// Pilote une future avec un nombre de sondages borné : `Some(sortie)` quand
/// elle rend, `None` si elle n'a pas rendu dans les `n` sondages — au lieu de
/// pendre indéfiniment sur un `block_on` (le cas rouge doit échouer, pas geler).
fn poll_for<F: Future>(fut: &mut Pin<Box<F>>, n: usize) -> Option<F::Output> {
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..n {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return Some(out);
        }
    }
    None
}

/// Une horloge de test : `sleep` avance le temps et rend la main une fois.
#[derive(Default)]
struct Horloge {
    now: Cell<f64>,
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
impl Clock for Horloge {
    fn now_ms(&self) -> f64 {
        self.now.get()
    }
    fn sleep(&self, ms: u64) -> Option<Sleep> {
        self.now.set(self.now.get() + ms as f64);
        Some(Box::pin(Cede(false)))
    }
}

/// Un transport qui compte ses appels et répond « fait » (fin normale).
struct Compte(RefCell<usize>);
#[async_trait(?Send)]
impl Transport for Compte {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        *self.0.borrow_mut() += 1;
        Ok(Reply {
            text: "fait".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

/// Un outillage vide.
struct Rien;
#[async_trait(?Send)]
impl Toolbox for Rien {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok(String::new())
    }
}

/// Un hook de contexte qui ne rend jamais la main.
struct ContextePendu;
#[async_trait(?Send)]
impl TurnContext for ContextePendu {
    async fn before_turn(&self, _: &[Message]) -> Option<String> {
        std::future::pending().await
    }
}

/// Un compacteur qui ne rend jamais la main.
struct CompacteurPendu;
#[async_trait(?Send)]
impl Compactor for CompacteurPendu {
    async fn compact(&self, _: &[Message]) -> Result<Message, Failure> {
        std::future::pending().await
    }
}

type Journal = Rc<RefCell<Vec<Event>>>;

fn arrets(log: &Journal) -> Vec<Stop> {
    log.borrow()
        .iter()
        .filter_map(|e| match e {
            Event::Stopped { stop, .. } => Some(stop.clone()),
            _ => None,
        })
        .collect()
}

/// #291 : un hook de contexte qui ne rend jamais la main ne doit pas retenir le
/// run après `interrupt()`. Le transport n'est jamais lancé.
#[test]
fn le_contexte_de_tour_pendu_n_retient_pas_l_interruption() {
    let appels = Rc::new(Compte(RefCell::new(0)));
    let log: Journal = Rc::default();
    let journal = log.clone();
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        appels.clone(),
        Rc::new(Rien),
        move |e| journal.borrow_mut().push(e),
    )
    .with_clock(Rc::new(Horloge::default()))
    .with_turn_context(Rc::new(ContextePendu));
    let handle = harness.handle();

    let mut fut = Box::pin(harness.send("question"));
    assert!(
        poll_for(&mut fut, 2).is_none(),
        "le run a fini seul : le hook n'a pas été atteint ou n'est pas pendant"
    );
    handle.interrupt();
    let fini = poll_for(&mut fut, POLLS);
    assert!(fini.is_some(), "le run ne se termine pas après interrupt()");
    assert_eq!(fini, Some(Running::Ran(Stop::Interrupted)));
    assert!(!handle.busy(), "busy doit retomber");
    assert_eq!(arrets(&log), vec![Stop::Interrupted], "un seul Stopped");
    assert_eq!(
        *appels.0.borrow(),
        0,
        "le transport ne doit jamais être appelé"
    );
}

/// #291 : un compacteur qui ne rend jamais la main ne doit pas retenir le run
/// après `interrupt()` — même avec de quoi compacter. Le transport n'est jamais lancé.
#[test]
fn le_compacteur_pendu_n_retient_pas_l_interruption() {
    let appels = Rc::new(Compte(RefCell::new(0)));
    let log: Journal = Rc::default();
    let journal = log.clone();
    let harness = Harness::new(
        "consigne",
        Limits {
            context_budget: Some(100),
            ..Limits::default()
        },
        appels.clone(),
        Rc::new(Rien),
        move |e| journal.borrow_mut().push(e),
    )
    .with_clock(Rc::new(Horloge::default()))
    .with_compactor(Rc::new(CompacteurPendu));
    // De quoi retirer : le vieux message dépasse le budget effectif.
    harness.seed(vec![Message::user("a".repeat(400))]);
    let handle = harness.handle();

    let mut fut = Box::pin(harness.send("question"));
    assert!(
        poll_for(&mut fut, 2).is_none(),
        "le run a fini seul : le compacteur n'a pas été atteint ou n'est pas pendant"
    );
    handle.interrupt();
    let fini = poll_for(&mut fut, POLLS);
    assert!(fini.is_some(), "le run ne se termine pas après interrupt()");
    assert_eq!(fini, Some(Running::Ran(Stop::Interrupted)));
    assert!(!handle.busy());
    assert_eq!(arrets(&log), vec![Stop::Interrupted]);
    assert_eq!(
        *appels.0.borrow(),
        0,
        "le transport ne doit jamais être appelé"
    );
}

/// #291 : un compacteur pendu SANS interruption est borné par l'horloge : passé
/// le délai, on se replie sur `fit` et le tour part quand même (le transport est appelé).
#[test]
fn le_compacteur_pendu_sans_interruption_se_replie_sur_fit() {
    let appels = Rc::new(Compte(RefCell::new(0)));
    let harness = Harness::new(
        "consigne",
        Limits {
            context_budget: Some(100),
            ..Limits::default()
        },
        appels.clone(),
        Rc::new(Rien),
        |_| {},
    )
    .with_clock(Rc::new(Horloge::default()))
    .with_compactor(Rc::new(CompacteurPendu));
    harness.seed(vec![Message::user("a".repeat(400))]);

    let mut fut = Box::pin(harness.send("question"));
    let fini = poll_for(&mut fut, POLLS);
    assert!(fini.is_some(), "le run doit se replier sur fit et finir");
    assert_eq!(fini, Some(Running::Ran(Stop::Done)));
    assert_eq!(
        *appels.0.borrow(),
        1,
        "le transport doit être appelé une seule fois (repli sur fit)"
    );
}
