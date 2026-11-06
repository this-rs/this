//! L'hôte déclare ses outils ([`Toolbox::declarations`]) : un `deploy_app` déclaré obtient,
//! à travers la boucle entière, le contrôle qu'`install_module` obtient sans rien déclarer.
//! Moteur, outils et horloge factices : rien ne sort de la machine.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use serde_json::json;

use this_agent::evidence::RAPPEL;
use this_harness::{
    Cancel, Clock, Declaration, Delta, Event, Failure, Finish, Harness, Limits, Outcome, Reply,
    Request, Running, Sleep, Stop, ToolCall, ToolSpec, Toolbox, Toolboxes, Transport,
};

#[derive(Default)]
struct FakeClock {
    now: Cell<f64>,
}

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

/// Un modèle scripté : à court de script, il répète « C'est fait. ».
struct Model {
    replies: RefCell<VecDeque<Reply>>,
}

fn says(text: &str) -> Reply {
    Reply {
        text: text.to_string(),
        calls: vec![],
        finish: Finish::Stop,
    }
}

fn calls(name: &str, id: &str) -> Reply {
    Reply {
        text: String::new(),
        calls: vec![ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({}),
        }],
        finish: Finish::Tools,
    }
}

#[async_trait(?Send)]
impl Transport for Model {
    async fn stream(
        &self,
        _request: &Request,
        _cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        let reply = self
            .replies
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| says("C'est fait."));
        delta(&reply.text);
        Ok(reply)
    }
}

/// `refuse` refuse toujours ; `row_create` réussit et change l'état observé.
struct Tools {
    refuse: &'static str,
    declarations: Vec<Declaration>,
}

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        [self.refuse, "row_create"]
            .iter()
            .map(|name| ToolSpec {
                name: name.to_string(),
                description: format!("outil {name}"),
                parameters: json!({"type": "object"}),
            })
            .collect()
    }
    fn declarations(&self) -> Vec<Declaration> {
        self.declarations.clone()
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        if call.name == self.refuse {
            Outcome::refused("refusé : quota atteint", "quota atteint")
        } else {
            Outcome::ok("ligne créée").observed("0", "1")
        }
    }
}

/// Le run : l'outil refusé, une écriture sans rapport réussie, puis le récit (deux fois,
/// pour voir l'issue après le rappel). Rend l'arrêt et les rappels montrés au modèle.
fn scenario(toolbox: Rc<dyn Toolbox>, refuse: &str, recit: &str) -> (Running, Vec<String>) {
    let model = Rc::new(Model {
        replies: RefCell::new(
            vec![
                calls(refuse, "a1"),
                calls("row_create", "a2"),
                says(recit),
                says(recit),
            ]
            .into(),
        ),
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let seen = log.clone();
    let harness = Harness::new(
        "tu es l'hôte",
        Limits::default(),
        model,
        toolbox,
        move |event| seen.borrow_mut().push(event),
    )
    .with_clock(Rc::new(FakeClock::default()));
    let stop = futures::executor::block_on(harness.send("fais-le"));
    let rappels = log
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Hint { text, .. } if text.contains(RAPPEL) => Some(text.clone()),
            _ => None,
        })
        .collect();
    (stop, rappels)
}

fn deploy_app() -> Declaration {
    Declaration::writes("deploy_app").words(["déployer", "deploy"])
}

#[test]
fn declarations_install_module_sans_declaration_reste_controle() {
    let tools = Rc::new(Tools {
        refuse: "install_module",
        declarations: vec![],
    });
    let (stop, rappels) = scenario(tools, "install_module", "J'ai installé le module.");
    assert_ne!(stop, Running::Ran(Stop::Done));
    assert_eq!(rappels.len(), 1, "{rappels:?}");
    assert!(rappels[0].contains("`install_module`"), "{rappels:?}");
}

#[test]
fn declarations_deploy_app_declare_obtient_le_meme_controle() {
    let tools = Rc::new(Tools {
        refuse: "deploy_app",
        declarations: vec![deploy_app()],
    });
    let (stop, rappels) = scenario(tools, "deploy_app", "J'ai déployé l'application.");
    assert!(
        matches!(&stop, Running::Ran(Stop::Refused(m)) if m.contains("quota")),
        "{stop:?}"
    );
    assert_eq!(rappels.len(), 1, "{rappels:?}");
    assert!(
        rappels[0].contains("`deploy_app`") && rappels[0].contains("quota atteint"),
        "{rappels:?}"
    );
}

#[test]
fn declarations_deploy_app_sans_declaration_passe_pour_fait() {
    // Le trou que la déclaration ferme : sans elle, « déployé » n'est pas lu.
    let tools = Rc::new(Tools {
        refuse: "deploy_app",
        declarations: vec![],
    });
    let (stop, rappels) = scenario(tools, "deploy_app", "J'ai déployé l'application.");
    assert_eq!(stop, Running::Ran(Stop::Done));
    assert!(rappels.is_empty(), "{rappels:?}");
}

#[test]
fn declarations_traversent_un_toolboxes() {
    // Les déclarations d'une part composée arrivent jusqu'à la décision.
    let part = Rc::new(Tools {
        refuse: "deploy_app",
        declarations: vec![deploy_app()],
    });
    let composes = Toolboxes::new().with(part);
    assert_eq!(composes.declarations(), vec![deploy_app()]);
    let (stop, rappels) = scenario(
        Rc::new(composes),
        "deploy_app",
        "I deployed the application.",
    );
    assert_ne!(stop, Running::Ran(Stop::Done));
    assert_eq!(rappels.len(), 1, "{rappels:?}");
    assert!(rappels[0].contains("`deploy_app`"), "{rappels:?}");
}
