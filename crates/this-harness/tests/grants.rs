//! La porte d'autorisation du harnais : ce qu'un outil déclare n'est pas ce qu'il a le droit de faire.
//!
//! POURQUOI ce banc : un agent qui écrit ses propres modules peut réécrire sa politique ;
//! sans garde posée dans la boucle, la sécurité tient à la seule bonne foi du `Toolbox`.
//! Chaque test compte les appels réels de la doublure : un refus qui « dit non » mais
//! exécute quand même serait pire que pas de refus.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_agent::evidence::Declaration;
use this_agent::graph::HarnessDoc;
use this_agent::substrate::{Grant, Grants};
use this_harness::{
    Cancel, Delta, Event, Failure, Finish, Harness, Limits, Outcome, Reply, Request, ToolCall,
    ToolSpec, Toolbox, Transport,
};

/// Un moteur qui demande `tool` une fois, puis conclut.
struct AsksOnce {
    tool: &'static str,
    asked: Cell<bool>,
}

#[async_trait(?Send)]
impl Transport for AsksOnce {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.asked.replace(true) {
            return Ok(Reply {
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            });
        }
        Ok(Reply {
            text: String::new(),
            calls: vec![ToolCall {
                id: "c1".into(),
                name: self.tool.into(),
                arguments: json!({}),
            }],
            finish: Finish::Tools,
        })
    }
}

/// Deux outils exposés ; seul `declared_tool` porte une déclaration d'effet.
struct Tools {
    calls: Cell<u32>,
}

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        ["declared_tool", "ghost_tool"]
            .into_iter()
            .map(|name| ToolSpec {
                name: name.into(),
                description: name.into(),
                parameters: json!({"type": "object"}),
            })
            .collect()
    }
    fn declarations(&self) -> Vec<Declaration> {
        vec![Declaration::writes("declared_tool")]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        self.calls.set(self.calls.get() + 1);
        Outcome::ok("exécuté")
    }
}

/// Lance un run où le modèle demande `tool` ; rend (appels réels, événements).
fn drive(tool: &'static str, grants: Option<Grants>) -> (u32, Vec<Event>) {
    let tools = Rc::new(Tools {
        calls: Cell::new(0),
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        Rc::new(AsksOnce {
            tool,
            asked: Cell::new(false),
        }),
        tools.clone(),
        move |e| sink.borrow_mut().push(e),
    );
    if let Some(grants) = grants {
        harness = harness.with_grants(grants);
    }
    futures::executor::block_on(harness.send("go"));
    let events = log.borrow().clone();
    (tools.calls.get(), events)
}

fn result_of(events: &[Event]) -> (bool, String) {
    events
        .iter()
        .find_map(|e| match e {
            Event::ToolResult { refused, text, .. } => Some((*refused, text.clone())),
            _ => None,
        })
        .expect("un ToolResult a été émis")
}

#[test]
fn un_outil_non_declare_est_refuse_et_jamais_appele() {
    let mut grants = Grants::new();
    grants.put(Grant::granted("declared_tool", "user"));
    // Même un droit explicite ne rattrape pas l'absence de déclaration d'effet.
    grants.put(Grant::granted("ghost_tool", "user"));
    let (calls, events) = drive("ghost_tool", Some(grants));
    assert_eq!(calls, 0, "le call de la doublure ne doit jamais partir");
    let (refused, _) = result_of(&events);
    assert!(refused);
}

#[test]
fn un_outil_declare_mais_non_autorise_est_refuse_pour_cette_raison() {
    let mut grants = Grants::new();
    grants.put(Grant::refused("declared_tool", "user", "module non signé"));
    let (calls, events) = drive("declared_tool", Some(grants));
    assert_eq!(calls, 0);
    let (refused, text) = result_of(&events);
    assert!(refused);
    // Le motif de l'autorisation, pas un refus générique.
    let motif = format!("{events:?}");
    assert!(motif.contains("module non signé"), "{motif}");
    assert!(text.contains("declared_tool"));
}

#[test]
fn un_outil_sans_aucune_autorisation_est_refuse_fail_closed() {
    let (calls, events) = drive("declared_tool", Some(Grants::new()));
    assert_eq!(calls, 0);
    assert!(result_of(&events).0);
}

#[test]
fn un_outil_declare_et_autorise_passe() {
    let mut grants = Grants::new();
    grants.put(Grant::granted("declared_tool", "user"));
    let (calls, events) = drive("declared_tool", Some(grants));
    assert_eq!(calls, 1);
    assert!(!result_of(&events).0);
}

#[test]
fn sans_autorisation_installee_tout_passe_comme_avant() {
    for tool in ["declared_tool", "ghost_tool"] {
        let (calls, events) = drive(tool, None);
        assert_eq!(calls, 1, "{tool} : le harnais ne fait rien respecter");
        assert!(!result_of(&events).0);
    }
}

#[test]
fn une_composition_du_graphe_porte_les_quatre_bornes() {
    let json = r#"{"id":"h","name":"n","version":1,"brief":"b","slots":[],
        "max_tours":5,"max_relances":1,
        "silence_ms":11,"no_progress_ms":22,"tool_timeout_ms":33,"context_budget":44}"#;
    let doc: HarnessDoc = serde_json::from_str(json).unwrap();
    let limits = Limits::from_doc(&doc);
    assert_eq!(limits.silence_ms, Some(11));
    assert_eq!(limits.no_progress_ms, Some(22));
    assert_eq!(limits.tool_timeout_ms, Some(33));
    assert_eq!(limits.context_budget, Some(44));

    // Un document ancien (sans ces champs) garde les constantes : additif.
    let old: HarnessDoc = serde_json::from_str(
        r#"{"id":"h","name":"n","version":1,"brief":"b","slots":[],"max_tours":5,"max_relances":1}"#,
    )
    .unwrap();
    let d = Limits::default();
    let l = Limits::from_doc(&old);
    assert_eq!(l.context_budget, d.context_budget);
    assert_eq!(l.silence_ms, d.silence_ms);
    assert_eq!(l.no_progress_ms, d.no_progress_ms);
    assert_eq!(l.tool_timeout_ms, d.tool_timeout_ms);
}

// ---------------------------------------------------------------------------------------
// Les autorisations tenues par l'OUTILLAGE (`Toolbox::grants`).
//
// POURQUOI : un hôte dont les outils apparaissent en cours de session (un module installé
// après la construction du harnais) ne peut pas figer ses autorisations à la construction.
// Un ensemble figé les refuserait tous — juste en forme, faux en fait.
// ---------------------------------------------------------------------------------------

/// Le même outillage, mais qui tient ses autorisations à jour et les rend au harnais.
struct ToolsADroits {
    calls: Cell<u32>,
    grants: RefCell<Grants>,
}

#[async_trait(?Send)]
impl Toolbox for ToolsADroits {
    fn specs(&self) -> Vec<ToolSpec> {
        ["declared_tool", "ghost_tool"]
            .into_iter()
            .map(|name| ToolSpec {
                name: name.into(),
                description: name.into(),
                parameters: json!({"type": "object"}),
            })
            .collect()
    }
    fn declarations(&self) -> Vec<Declaration> {
        vec![Declaration::writes("declared_tool")]
    }
    fn grants(&self) -> Option<Grants> {
        Some(self.grants.borrow().clone())
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        self.calls.set(self.calls.get() + 1);
        Outcome::ok("exécuté")
    }
}

/// Un moteur qui redemande l'outil à CHAQUE tour : le cas d'un run relancé après une
/// installation de module.
struct AsksToujours {
    tool: &'static str,
}

#[async_trait(?Send)]
impl Transport for AsksToujours {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        Ok(Reply {
            text: String::new(),
            calls: vec![ToolCall {
                id: "c1".into(),
                name: self.tool.into(),
                arguments: json!({}),
            }],
            finish: Finish::Tools,
        })
    }
}

fn harnais_adroits(
    tools: Rc<ToolsADroits>,
    statique: Option<Grants>,
) -> (Harness, Rc<RefCell<Vec<Event>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        Rc::new(AsksOnce {
            tool: "declared_tool",
            asked: Cell::new(false),
        }),
        tools,
        move |e| sink.borrow_mut().push(e),
    );
    if let Some(grants) = statique {
        harness = harness.with_grants(grants);
    }
    (harness, log)
}

#[test]
fn l_outillage_autorise_sans_qu_on_appelle_with_grants() {
    let mut grants = Grants::new();
    grants.put(Grant::granted("declared_tool", "app"));
    let tools = Rc::new(ToolsADroits {
        calls: Cell::new(0),
        grants: RefCell::new(grants),
    });
    let (mut harness, log) = harnais_adroits(tools.clone(), None);
    futures::executor::block_on(harness.send("go"));
    assert_eq!(
        tools.calls.get(),
        1,
        "la porte doit être active par l'outillage seul"
    );
    assert!(!result_of(&log.borrow()).0);
}

#[test]
fn l_outillage_prime_sur_les_autorisations_figees() {
    // Le statique autorise ; l'outillage, lui, a retiré le droit. C'est le plus récent qui
    // vaut — sinon un module désinstallé garderait son droit jusqu'au redémarrage.
    let mut statique = Grants::new();
    statique.put(Grant::granted("declared_tool", "app"));
    let tools = Rc::new(ToolsADroits {
        calls: Cell::new(0),
        grants: RefCell::new(Grants::new()),
    });
    let (mut harness, log) = harnais_adroits(tools.clone(), Some(statique));
    futures::executor::block_on(harness.send("go"));
    assert_eq!(tools.calls.get(), 0);
    assert!(result_of(&log.borrow()).0);
}

#[test]
fn un_outil_qui_apparait_en_cours_de_session_n_est_pas_refuse() {
    // Le cas mesuré d'un module installé pendant la session : refusé au premier tour
    // (aucun droit), puis exécuté dès que l'outillage en pose un — sans reconstruire le
    // harnais.
    let tools = Rc::new(ToolsADroits {
        calls: Cell::new(0),
        grants: RefCell::new(Grants::new()),
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        Rc::new(AsksToujours {
            tool: "declared_tool",
        }),
        tools.clone(),
        move |e| sink.borrow_mut().push(e),
    );
    futures::executor::block_on(harness.send("go"));
    assert_eq!(tools.calls.get(), 0, "sans droit, rien ne part");
    assert!(result_of(&log.borrow()).0);

    tools
        .grants
        .borrow_mut()
        .put(Grant::granted("declared_tool", "module installé"));
    log.borrow_mut().clear();
    futures::executor::block_on(harness.send("go"));
    // Le moteur redemande à chaque tour : ce qui compte est qu'il soit servi, pas le compte.
    assert!(
        tools.calls.get() > 0,
        "le droit posé après coup doit être vu"
    );
    assert!(!result_of(&log.borrow()).0);
}

#[test]
fn un_outil_sans_declaration_reste_refuse_meme_autorise_par_l_outillage() {
    let mut grants = Grants::new();
    grants.put(Grant::granted("ghost_tool", "app"));
    let tools = Rc::new(ToolsADroits {
        calls: Cell::new(0),
        grants: RefCell::new(grants),
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        Rc::new(AsksOnce {
            tool: "ghost_tool",
            asked: Cell::new(false),
        }),
        tools.clone(),
        move |e| sink.borrow_mut().push(e),
    );
    let _ = &mut harness;
    futures::executor::block_on(harness.send("go"));
    assert_eq!(tools.calls.get(), 0);
    assert!(
        result_of(&log.borrow()).0,
        "une déclaration manquante ne vaut pas un droit"
    );
}
