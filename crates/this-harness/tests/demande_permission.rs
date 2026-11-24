//! #286 : une demande de permission. Un outil sans AUCUN grant ne s'exécute pas : la
//! personne est interrogée (`PermissionRequest`), et sa réponse vaut pour les appels SUIVANTS.
//! Un refus explicite (`Grant::refused`) ne redemande jamais : la décision est respectée.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_agent::substrate::{Grant, Grants};
use this_harness::{
    Cancel, Declaration, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome, Reply,
    Request, Running, ToolCall, ToolSpec, Toolbox, Transport,
};

const OUTIL: &str = "ecrire";

/// Demande l'outil à chaque tour, puis conclut une fois le résultat reçu.
struct Demande;

#[async_trait(?Send)]
impl Transport for Demande {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if matches!(request.thread().last(), Some(Message::Tool { .. })) {
            return Ok(Reply {
                served: None,
                text: "fini".into(),
                calls: vec![],
                finish: Finish::Stop,
            });
        }
        Ok(Reply {
            served: None,
            text: String::new(),
            calls: vec![ToolCall {
                id: "c1".into(),
                name: OUTIL.into(),
                arguments: json!({}),
            }],
            finish: Finish::Tools,
        })
    }
}

struct Ecrire {
    effets: Rc<Cell<u32>>,
}

#[async_trait(?Send)]
impl Toolbox for Ecrire {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: OUTIL.into(),
            description: "écrit".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    fn declarations(&self) -> Vec<Declaration> {
        vec![Declaration::writes(OUTIL)]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        self.effets.set(self.effets.get() + 1);
        Outcome::ok("écrit")
    }
}

/// Un hôte qui a installé des grants, mais aucun pour `ecrire`.
fn hote(effets: Rc<Cell<u32>>, grants: Grants) -> (Harness, Rc<RefCell<Vec<Event>>>) {
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = events.clone();
    let harness = Harness::new(
        "consigne",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        Rc::new(Demande),
        Rc::new(Ecrire { effets }),
        move |e| sink.borrow_mut().push(e),
    )
    .with_grants(grants);
    (harness, events)
}

fn grants_sans_ecrire() -> Grants {
    let mut g = Grants::new();
    g.put(Grant::granted("autre", "user"));
    g
}

#[test]
fn un_outil_sans_grant_demande_la_personne_et_ne_s_execute_pas() {
    let effets = Rc::new(Cell::new(0));
    let (h, events) = hote(effets.clone(), grants_sans_ecrire());
    futures::executor::block_on(h.send("écris"));
    assert_eq!(
        effets.get(),
        0,
        "sans autorisation, l'outil ne s'exécute pas"
    );
    assert_eq!(
        h.pending_permissions().len(),
        1,
        "la personne est interrogée"
    );
    assert_eq!(h.pending_permissions()[0].1, OUTIL);
    assert!(
        events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::PermissionRequest { tool, .. } if tool == OUTIL)),
        "la demande est émise à l'hôte"
    );
}

#[test]
fn repondre_oui_autorise_les_appels_suivants() {
    let effets = Rc::new(Cell::new(0));
    let (h, events) = hote(effets.clone(), grants_sans_ecrire());
    futures::executor::block_on(h.send("écris"));
    let (id, _) = h.pending_permissions()[0].clone();
    assert!(
        h.answer_permission(id, true),
        "une demande connue reçoit sa réponse"
    );
    assert!(h.pending_permissions().is_empty(), "la demande est close");
    assert!(
        events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::PermissionAnswered { granted: true, .. })),
        "la réponse est émise"
    );
    let r = futures::executor::block_on(h.send("encore"));
    assert!(matches!(r, Running::Ran(_)));
    assert_eq!(effets.get(), 1, "après un oui, l'appel suivant s'exécute");
}

#[test]
fn repondre_non_refuse_sans_redemander() {
    let effets = Rc::new(Cell::new(0));
    let (h, _) = hote(effets.clone(), grants_sans_ecrire());
    futures::executor::block_on(h.send("écris"));
    let (id, _) = h.pending_permissions()[0].clone();
    assert!(h.answer_permission(id, false));
    futures::executor::block_on(h.send("encore"));
    assert_eq!(effets.get(), 0, "un refus est respecté");
    assert!(
        h.pending_permissions().is_empty(),
        "un refus explicite ne rouvre pas de demande"
    );
}

#[test]
fn une_reponse_a_un_id_inconnu_est_rejetee() {
    let effets = Rc::new(Cell::new(0));
    let (h, _) = hote(effets, grants_sans_ecrire());
    futures::executor::block_on(h.send("écris"));
    let avant = h.pending_permissions();
    assert!(
        !h.answer_permission(9999, true),
        "un id inconnu n'est pas accepté"
    );
    assert_eq!(h.pending_permissions(), avant, "et ne change rien");
}
