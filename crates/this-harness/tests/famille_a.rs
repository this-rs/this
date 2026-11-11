//! La famille A (Claude Code, Codex, ACP) : le moteur exécute l'outil pendant
//! `stream`, et ce qui remonte est un `ToolUse` plus une demande de permission.
//! La porte ([`Permission`]) rend au transport, hors-bande, la même décision
//! que `gate` et la même journalisation que `run_tool` — au lieu de les
//! contourner.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_agent::substrate::Grants;
use this_harness::{
    Cancel, Code, Declaration, Delta, Failure, Finish, Harness, Limits, Outcome, Permission,
    Recorder, Reply, Request, Run, Running, Step, Stop, ToolCall, ToolSpec, Toolbox, Transport,
};

/// Un moteur de la famille A. Au premier tour, il demande l'outil au CLI puis
/// consulte la porte ; sur refus, il journalise le refus (hors-bande) et renvoie
/// l'erreur au moteur. Le booléen dit si la porte a été consultée.
struct EngineFamilleA {
    gate_consulte: Rc<Cell<bool>>,
    permission: RefCell<Option<Rc<dyn Permission>>>,
}

#[async_trait(?Send)]
impl Transport for EngineFamilleA {
    fn set_permission(&self, permission: Rc<dyn Permission>) {
        *self.permission.borrow_mut() = Some(permission);
    }

    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let permission = self
            .permission
            .borrow()
            .clone()
            .expect("la porte a été posée avant le flux");
        let call = ToolCall {
            id: "t1".into(),
            name: "lire".into(),
            arguments: json!({}),
        };
        // Le CLI émet l'appel et demande la permission.
        self.gate_consulte.set(true);
        match permission.allows(&call.name) {
            Ok(()) => Ok(Reply {
                text: String::new(),
                calls: vec![call],
                finish: Finish::Tools,
            }),
            Err(motif) => {
                // Le refus est journalisé sous la même forme que `run_tool`…
                let outcome =
                    Outcome::refused(format!("outil non autorisé : {}", call.name), motif.clone());
                permission.record(&call, 0, &outcome).await;
                // …puis renvoyé au moteur : le run s'arrête en échec.
                Err(Failure::new(
                    Code::Provider,
                    format!("permission refusée : {motif}"),
                ))
            }
        }
    }
}

/// L'outil demandé par le moteur, déclaré (effet d'écriture) mais sans droit.
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
    fn declarations(&self) -> Vec<Declaration> {
        vec![Declaration::writes("lire")]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("ok")
    }
}

/// Une doublure qui retient ce que la porte lui a poussé.
#[derive(Default)]
struct Journal {
    steps: RefCell<Vec<Step>>,
    stops: RefCell<Vec<Stop>>,
}

#[async_trait(?Send)]
impl Recorder for Journal {
    async fn step(&self, _run_id: &str, _run: &Run, step: &Step) {
        self.steps.borrow_mut().push(step.clone());
    }
    async fn stopped(&self, _run_id: &str, _run: &Run, stop: &Stop) {
        self.stops.borrow_mut().push(stop.clone());
    }
}

fn banc() -> (Rc<EngineFamilleA>, Rc<Journal>, Harness) {
    let engine = Rc::new(EngineFamilleA {
        gate_consulte: Rc::new(Cell::new(false)),
        permission: RefCell::new(None),
    });
    let journal = Rc::new(Journal::default());
    // Aucun droit accordé : `gate` refuse « lire », comme il le ferait dans `run_tool`.
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        engine.clone(),
        Rc::new(Tools),
        |_| {},
    )
    .with_grants(Grants::new())
    .with_recorder(journal.clone());
    (engine, journal, harness)
}

#[test]
fn un_refus_de_la_porte_remonte_au_moteur_et_journalise_la_meme_etape() {
    let (engine, journal, harness) = banc();
    let result = futures::executor::block_on(harness.send("lis"));

    // (a) la porte a bien été consultée.
    assert!(engine.gate_consulte.get(), "le gate n'a pas été consulté");

    // (b) le refus remonte au moteur et arrête le run, avec le motif du gate.
    match result {
        Running::Ran(Stop::Failed(message)) => {
            assert!(message.contains("permission refusée"), "{message}");
            assert!(message.contains("aucune autorisation"), "{message}");
        }
        other => panic!("attendu un arrêt en échec, obtenu {other:?}"),
    }

    // (c) une étape est journalisée, avec la même forme que `run_tool`.
    let steps = journal.steps.borrow();
    assert_eq!(steps.len(), 1, "une seule étape journalisée");
    let step = &steps[0];
    assert_eq!(step.tool, "lire");
    assert!(step.refused, "le refus est marqué dans l'étape");
    assert!(
        step.motif
            .as_deref()
            .unwrap()
            .contains("aucune autorisation"),
        "motif : {:?}",
        step.motif
    );
    assert!(
        step.out_hash.is_some(),
        "l'empreinte de la sortie est posée"
    );
    assert!(!step.resume.is_empty(), "le résumé porte le texte du refus");

    // Le run arrêté est annoncé une fois, avec la même identité.
    assert_eq!(journal.stops.borrow().len(), 1);
}

/// Même moteur, mais la porte autorise : le moteur exécute l'outil lui-même et
/// journalise le résultat hors-bande ; la boucle ne rejoue pas l'outil (aucun
/// `ToolUse` émis par `run_tool`), et l'étape porte la même forme qu'un succès.
#[test]
fn un_outil_autorise_est_journalise_hors_bande_sans_rejeu() {
    struct EngineAutorise {
        permission: RefCell<Option<Rc<dyn Permission>>>,
        rejoue: Rc<Cell<bool>>,
    }
    #[async_trait(?Send)]
    impl Transport for EngineAutorise {
        fn set_permission(&self, permission: Rc<dyn Permission>) {
            *self.permission.borrow_mut() = Some(permission);
        }
        async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
            let permission = self.permission.borrow().clone().unwrap();
            let call = ToolCall {
                id: "t1".into(),
                name: "lire".into(),
                arguments: json!({}),
            };
            // Autorisé : le CLI exécute, on journalise le résultat, sans rendre
            // l'appel à la boucle (elle ne doit pas le rejouer).
            permission.record(&call, 3, &Outcome::ok("lu")).await;
            self.rejoue.set(true);
            Ok(Reply {
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            })
        }
    }

    let engine = Rc::new(EngineAutorise {
        permission: RefCell::new(None),
        rejoue: Rc::new(Cell::new(false)),
    });
    let journal = Rc::new(Journal::default());
    let mut grants = Grants::new();
    grants.put(this_agent::substrate::Grant::granted("lire", "test"));
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        engine.clone(),
        Rc::new(Tools),
        |_| {},
    )
    .with_grants(grants)
    .with_recorder(journal.clone());

    let result = futures::executor::block_on(harness.send("lis"));
    assert!(matches!(result, Running::Ran(Stop::Done)), "{result:?}");
    assert!(engine.rejoue.get(), "le moteur a exécuté l'outil lui-même");

    let steps = journal.steps.borrow();
    assert_eq!(steps.len(), 1);
    let step = &steps[0];
    assert_eq!(step.tool, "lire");
    assert!(!step.refused, "un succès n'est pas un refus");
    assert_eq!(step.failure, None);
    assert_eq!(step.ms, 3);
    assert!(step.out_hash.is_some());
}
