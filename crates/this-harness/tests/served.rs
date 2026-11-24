//! #288 : la paire SERVIE est dite à l'hôte (`Event::Served`), pas seulement la paire
//! demandée. Un moteur qui déclare le modèle qu'il a réellement servi doit le faire voir.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;

use this_harness::{
    Cancel, Declaration, Delta, Event, Failure, Finish, Harness, Limits, Outcome, Reply, Request,
    ToolCall, ToolSpec, Toolbox, Transport,
};

/// Répond une fois, en déclarant un modèle servi donné (`served`).
struct Sert {
    served: Option<&'static str>,
    fait: Cell<bool>,
}

#[async_trait(?Send)]
impl Transport for Sert {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.fait.replace(true) {
            return Ok(Reply {
                usage: None,
                served: None,
                text: String::new(),
                calls: vec![],
                finish: Finish::Stop,
            });
        }
        Ok(Reply {
            usage: None,
            served: self.served.map(str::to_string),
            text: "ok".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

/// Aucun outil : ce test ne porte que sur la paire servie.
struct AucunOutil;

#[async_trait(?Send)]
impl Toolbox for AucunOutil {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![]
    }
    fn declarations(&self) -> Vec<Declaration> {
        vec![]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("")
    }
}

fn servis(served: Option<&'static str>) -> Vec<Event> {
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = events.clone();
    let harness = Harness::new(
        "sys",
        Limits::default(),
        Rc::new(Sert {
            served,
            fait: Cell::new(false),
        }),
        Rc::new(AucunOutil),
        move |e| sink.borrow_mut().push(e),
    );
    futures::executor::block_on(harness.send("bonjour"));
    events.borrow().clone()
}

#[test]
fn le_modele_servi_est_dit_a_l_hote() {
    let events = servis(Some("modele-servi"));
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Served { served } if served == "modele-servi"
        )),
        "le modèle réellement servi doit être émis : {events:?}"
    );
}

/// Témoin : un moteur qui ne déclare rien ne fabrique pas de paire servie.
#[test]
fn sans_modele_declare_rien_n_est_emis() {
    let events = servis(None);
    assert!(
        !events.iter().any(|e| matches!(e, Event::Served { .. })),
        "aucun modèle déclaré, aucun Served : {events:?}"
    );
}
