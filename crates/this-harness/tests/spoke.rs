//! Les contrats du flux par tour, sans réseau ni horloge.
//!
//! Ce qui a piégé l'app le 10/11 : elle posait `message.set("contenu", text)`
//! à chaque `Spoke`, croyant qu'un `Spoke` portait le texte du RUN entier. Sur
//! un run de trois tours, le troisième effaçait les deux premiers. Le harnais,
//! lui, émet **un `Spoke` par tour**, et ce `Spoke` porte **le texte de son
//! tour seulement** — jamais le cumul. Ces tests l'épinglent.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_harness::{
    Cancel, Delta, Event, Failure, Finish, Harness, Limits, Outcome, Reply, Request, Running, Stop,
    ToolCall, ToolSpec, Toolbox, Transport,
};

/// Une réplique de moteur pour un tour : le texte définitif, les morceaux de
/// flux poussés par `delta` (dans l'ordre), et ce que le tour demande ensuite.
struct Tour {
    texte: &'static str,
    morceaux: Vec<&'static str>,
    appels: Vec<ToolCall>,
    finish: Finish,
}

/// Un moteur scripté : il récite les tours dans l'ordre, en poussant chaque
/// morceau par `delta` avant de rendre la réponse du tour.
struct Script {
    tours: RefCell<VecDeque<Tour>>,
}

impl Script {
    fn new(tours: Vec<Tour>) -> Rc<Self> {
        Rc::new(Self {
            tours: RefCell::new(tours.into()),
        })
    }
}

#[async_trait(?Send)]
impl Transport for Script {
    async fn stream(
        &self,
        _request: &Request,
        _cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        let tour = self.tours.borrow_mut().pop_front().unwrap_or(Tour {
            texte: "fini",
            morceaux: vec!["fini"],
            appels: vec![],
            finish: Finish::Stop,
        });
        for morceau in &tour.morceaux {
            delta(morceau);
        }
        Ok(Reply {
            text: tour.texte.to_string(),
            calls: tour.appels,
            finish: tour.finish,
        })
    }
}

/// Un noyau d'outils qui réussit et observe son effet : chaque appel est une
/// écriture aboutie, donc un tour qui conclut ensuite se termine en `Done`.
struct Outils;

#[async_trait(?Send)]
impl Toolbox for Outils {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "install_module".to_string(),
            description: "installe un module".to_string(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _call: &ToolCall) -> Outcome {
        Outcome::ok("installé").observed("avant", "après")
    }
}

fn appel() -> ToolCall {
    ToolCall {
        id: "c".to_string(),
        name: "install_module".to_string(),
        arguments: json!({}),
    }
}

type Log = Rc<RefCell<Vec<Event>>>;

fn monter(tours: Vec<Tour>) -> (Harness, Log) {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let vu = log.clone();
    let harness = Harness::new(
        "tu es l'atelier",
        Limits::default(),
        Script::new(tours),
        Rc::new(Outils),
        move |event| vu.borrow_mut().push(event),
    );
    (harness, log)
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

/// Le flux texte seul : chaque delta et chaque spoke, dans l'ordre d'arrivée.
fn flux(log: &Log) -> Vec<String> {
    log.borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Delta { text } => Some(format!("delta:{text}")),
            Event::Spoke { text } => Some(format!("spoke:{text}")),
            _ => None,
        })
        .collect()
}

fn spokes(log: &Log) -> Vec<String> {
    log.borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Spoke { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Un run de trois tours émet, pour chaque tour, ses deltas puis UN `Spoke`,
/// dans l'ordre des tours — jamais un `Spoke` unique à la fin.
#[test]
fn un_run_de_plusieurs_tours_emet_deltas_puis_un_spoke_par_tour_dans_l_ordre() {
    let (harness, log) = monter(vec![
        Tour {
            texte: "premier tour",
            morceaux: vec!["premier ", "tour"],
            appels: vec![appel()],
            finish: Finish::Tools,
        },
        Tour {
            texte: "deuxième tour",
            morceaux: vec!["deuxième ", "tour"],
            appels: vec![appel()],
            finish: Finish::Tools,
        },
        Tour {
            texte: "dernier tour",
            morceaux: vec!["dernier ", "tour"],
            appels: vec![],
            finish: Finish::Stop,
        },
    ]);
    assert_eq!(run(harness.send("raconte")), Running::Ran(Stop::Done));
    assert_eq!(
        flux(&log),
        [
            "delta:premier ",
            "delta:tour",
            "spoke:premier tour",
            "delta:deuxième ",
            "delta:tour",
            "spoke:deuxième tour",
            "delta:dernier ",
            "delta:tour",
            "spoke:dernier tour",
        ]
    );
}

/// Le `Spoke` du tour N porte le texte du tour N **seul** — pas le cumul des
/// tours précédents. C'est précisément ce qui a piégé l'app.
#[test]
fn le_spoke_du_tour_n_porte_son_texte_seul_pas_le_cumul() {
    let (harness, log) = monter(vec![
        Tour {
            texte: "un",
            morceaux: vec!["un"],
            appels: vec![appel()],
            finish: Finish::Tools,
        },
        Tour {
            texte: "deux",
            morceaux: vec!["deux"],
            appels: vec![appel()],
            finish: Finish::Tools,
        },
        Tour {
            texte: "trois",
            morceaux: vec!["trois"],
            appels: vec![],
            finish: Finish::Stop,
        },
    ]);
    run(harness.send("compte"));
    // Trois tours, trois Spoke : « un », « deux », « trois ». Si le harnais
    // cumulait, on lirait « un », « un deux », « un deux trois ».
    assert_eq!(spokes(&log), ["un", "deux", "trois"]);
}

/// Un tour sans texte (un appel d'outil seul) émet **un** `Spoke` — vide — et
/// non une absence. C'est le comportement réel du harnais, figé ici.
#[test]
fn un_tour_sans_texte_emet_un_spoke_vide() {
    let (harness, log) = monter(vec![
        Tour {
            texte: "",
            morceaux: vec![],
            appels: vec![appel()],
            finish: Finish::Tools,
        },
        Tour {
            texte: "fini",
            morceaux: vec!["fini"],
            appels: vec![],
            finish: Finish::Stop,
        },
    ]);
    run(harness.send("installe"));
    assert_eq!(spokes(&log), ["", "fini"]);
}
