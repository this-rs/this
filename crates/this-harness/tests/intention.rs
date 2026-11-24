//! #284 : l'intention est journalisée AVANT l'appel d'outil, le résultat APRÈS.
//!
//! La fenêtre aveugle des effets (`docs/harnais-reprise-durable.md` §2(b), §5) : `ToolUse`
//! est émis avant l'appel et `Recorder::step` après — un arrêt brutal entre les deux ne
//! laissait aucune trace d'un effet pourtant peut-être produit. Le remède est d'écrire
//! l'intention (une étape `inconnu = true`) AVANT l'exécution. Ce test fige l'ordre et le
//! drapeau, au niveau du cœur portable (sans magasin) ; la reprise sur magasin réel est
//! couverte par `tests/reprise_crash.rs` (crate racine, feature `obrain`).

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
struct Moteur(RefCell<u32>);

#[async_trait(?Send)]
impl Transport for Moteur {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let n = {
            let mut n = self.0.borrow_mut();
            *n += 1;
            *n
        };
        Ok(if n == 1 {
            Reply {
                usage: None,
                served: None,
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
                usage: None,
                served: None,
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            }
        })
    }
}

/// L'outil écrit au monde : ici, il note seulement qu'il a tourné, dans le journal
/// d'ordre partagé avec l'enregistreur — c'est le « monde » observable de ce test.
struct Outil(Rc<RefCell<Vec<String>>>);

#[async_trait(?Send)]
impl Toolbox for Outil {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lire".into(),
            description: "lit".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        self.0.borrow_mut().push("outil".into());
        Outcome::ok("ok")
    }
}

/// L'enregistreur note, dans le MÊME journal d'ordre que l'outil, chaque appel — et garde
/// à part ce que l'intention et l'étape portaient comme `inconnu`.
struct Journal {
    ordre: Rc<RefCell<Vec<String>>>,
    intentions: RefCell<Vec<(String, bool)>>,
    etapes: RefCell<Vec<(String, bool)>>,
}

#[async_trait(?Send)]
impl Recorder for Journal {
    async fn intention(&self, _: &str, _: &Run, step: &Step) {
        self.ordre.borrow_mut().push("intention".into());
        self.intentions
            .borrow_mut()
            .push((step.tool.clone(), step.inconnu));
    }
    async fn step(&self, _: &str, _: &Run, step: &Step) {
        self.ordre.borrow_mut().push("etape".into());
        self.etapes
            .borrow_mut()
            .push((step.tool.clone(), step.inconnu));
    }
    async fn stopped(&self, _: &str, _: &Run, _: &Stop) {
        self.ordre.borrow_mut().push("arret".into());
    }
}

#[test]
fn l_intention_est_journalisee_avant_l_effet_et_l_etape_apres() {
    let ordre = Rc::new(RefCell::new(Vec::new()));
    let journal = Rc::new(Journal {
        ordre: ordre.clone(),
        intentions: RefCell::new(Vec::new()),
        etapes: RefCell::new(Vec::new()),
    });
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        Rc::new(Moteur(RefCell::new(0))),
        Rc::new(Outil(ordre.clone())),
        |_| {},
    )
    .with_recorder(journal.clone());

    let _ = futures::executor::block_on(harness.send("lis"));

    // L'ordre est la preuve : l'intention précède l'effet, l'étape le suit.
    assert_eq!(
        *ordre.borrow(),
        vec!["intention", "outil", "etape", "arret"],
        "intention avant l'appel, étape après, arrêt en dernier"
    );
    // L'intention porte `inconnu = true` ; l'étape acquittée porte `inconnu = false`.
    assert_eq!(
        *journal.intentions.borrow(),
        vec![("lire".to_string(), true)],
        "l'intention dit « résultat inconnu »"
    );
    assert_eq!(
        *journal.etapes.borrow(),
        vec![("lire".to_string(), false)],
        "l'étape acquittée dit « résultat connu »"
    );
}
