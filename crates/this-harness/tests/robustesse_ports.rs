//! Les ports mis en erreur : `Toolboxes`, `Cancel`, `Recorders`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use futures::executor::block_on;
use serde_json::json;

use this_agent::{Run, Step, Stop};
use this_harness::{Cancel, Outcome, Recorder, ToolCall, ToolSpec, Toolbox, Toolboxes};

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
