//! Le branchement du `Recorder` : une étape d'outil est poussée au journal dès
//! qu'elle est mesurée, et l'arrêt du run est annoncé avec la même identité.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_agent::{Run, Step, Stop};
use this_harness::{
    Cancel, Delta, Failure, Finish, Harness, Limits, Outcome, Recorder, Reply, Request, ToolCall,
    ToolSpec, Toolbox, Transport,
};

/// Un outil au premier tour, puis la conclusion.
struct Engine(RefCell<u32>);

#[async_trait(?Send)]
impl Transport for Engine {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let n = {
            let mut n = self.0.borrow_mut();
            *n += 1;
            *n
        };
        Ok(if n == 1 {
            Reply {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "t1".into(),
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

struct Tools;

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lire".into(),
            description: "lit".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("ok")
    }
}

/// Une doublure qui compte ce qu'on lui pousse.
#[derive(Default)]
struct Count {
    steps: RefCell<Vec<(String, String, usize)>>,
    stops: RefCell<Vec<(String, Stop, Option<Stop>)>>,
}

#[async_trait(?Send)]
impl Recorder for Count {
    async fn step(&self, run_id: &str, run: &Run, step: &Step) {
        self.steps
            .borrow_mut()
            .push((run_id.into(), step.tool.clone(), run.steps.len()));
    }
    async fn stopped(&self, run_id: &str, run: &Run, stop: &Stop) {
        self.stops
            .borrow_mut()
            .push((run_id.into(), stop.clone(), run.stop.clone()));
    }
}

fn harness(count: &Rc<Count>) -> Harness {
    Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(Engine(RefCell::new(0))),
        Rc::new(Tools),
        |_| {},
    )
    .with_recorder(count.clone())
}

#[test]
fn a_run_pushes_its_steps_and_its_stop() {
    let count = Rc::new(Count::default());
    let harness = harness(&count);
    let _ = futures::executor::block_on(harness.send("lis"));

    // Une étape, vue avec le run déjà à jour, sous l'identité du premier objectif.
    assert_eq!(
        *count.steps.borrow(),
        vec![("run-1".to_string(), "lire".to_string(), 1)]
    );
    let stops = count.stops.borrow();
    assert_eq!(stops.len(), 1, "un seul arrêt annoncé");
    assert_eq!(stops[0].0, "run-1");
    assert_eq!(Some(stops[0].1.clone()), stops[0].2, "run.stop == stop");
}

#[test]
fn each_goal_gets_its_own_run_id() {
    let count = Rc::new(Count::default());
    let harness = harness(&count);
    let _ = futures::executor::block_on(harness.send("un"));
    // Le moteur ne demande plus d'outil : le second objectif s'arrête sans étape.
    let _ = futures::executor::block_on(harness.send("deux"));

    let stops = count.stops.borrow();
    let ids: Vec<_> = stops.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(ids, ["run-1", "run-2"]);
}
