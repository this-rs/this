//! #284 : reprise après un crash ENTRE l'effet et son acquittement.
//!
//! Un snapshot pris (ou sauvegardé par l'hôte) pendant qu'un outil tournait contient un
//! appel d'outil SANS réponse. Restauré dans un processus neuf, ce fil ne doit pas partir
//! tel quel au moteur : les moteurs refusent un `tool_call` sans réponse, et l'app
//! « paraît cassée pour de bon ». Il ne doit pas non plus rejouer l'outil : l'effet a
//! peut-être eu lieu. L'appel reçoit une réponse qui dit que l'effet est INCONNU.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_harness::{
    Cancel, Declaration, Delta, Event, Failure, Finish, Harness, HintKind, Limits, Message,
    Outcome, Reply, Request, ToolCall, ToolSpec, Toolbox, Transport,
};

/// Garde le fil de chaque requête et conclut aussitôt.
#[derive(Default)]
struct Espion {
    fils: RefCell<Vec<Vec<Message>>>,
}

#[async_trait(?Send)]
impl Transport for Espion {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.fils.borrow_mut().push(request.thread());
        Ok(Reply {
            served: None,
            text: "ok".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

/// Un outil qui écrit au monde : `effets` compte les exécutions réelles.
struct Ecrire {
    effets: Cell<u32>,
}

#[async_trait(?Send)]
impl Toolbox for Ecrire {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "ecrire".into(),
            description: "écrit".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    fn declarations(&self) -> Vec<Declaration> {
        vec![Declaration::writes("ecrire")]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        self.effets.set(self.effets.get() + 1);
        Outcome::ok("écrit")
    }
}

/// Un fil laissé ouvert par un crash : la demande, l'appel d'outil, et AUCUNE réponse.
fn fil_apres_crash() -> Vec<Message> {
    vec![
        Message::user("écris le fichier"),
        Message::Assistant {
            content: String::new(),
            calls: vec![ToolCall {
                id: "c1".into(),
                name: "ecrire".into(),
                arguments: json!({}),
            }],
        },
    ]
}

fn reprendre() -> (Rc<Espion>, Rc<Ecrire>) {
    let moteur = Rc::new(Espion::default());
    let outil = Rc::new(Ecrire {
        effets: Cell::new(0),
    });
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        moteur.clone(),
        outil.clone(),
        |_| {},
    );
    let mut sauve = harness.snapshot();
    sauve.transcript = fil_apres_crash();
    harness.restore_trusted(sauve);
    futures::executor::block_on(harness.send("et ensuite ?"));
    (moteur, outil)
}

#[test]
fn un_appel_sans_reponse_apres_crash_recoit_une_reponse_qui_dit_l_effet_inconnu() {
    let (moteur, _) = reprendre();
    let fil = moteur.fils.borrow()[0].clone();
    let reponse = fil.iter().find_map(|m| match m {
        Message::Tool { id, content } if id == "c1" => Some(content.clone()),
        _ => None,
    });
    let reponse = reponse.unwrap_or_else(|| {
        panic!("l'appel c1 est parti sans réponse au moteur, qui refuserait ce fil : {fil:?}")
    });
    assert!(
        reponse.contains("inconnu"),
        "la réponse dit que l'effet est inconnu : {reponse}"
    );
}

#[test]
fn la_reprise_ne_rejoue_jamais_l_outil_dont_le_resultat_est_inconnu() {
    let (_, outil) = reprendre();
    assert_eq!(
        outil.effets.get(),
        0,
        "l'effet a peut-être eu lieu : jamais rejoué à l'aveugle"
    );
}

/// Restaure `fil` dans un harnais neuf (par la voie `de_confiance` ou la voie importée),
/// joue un message, et rend ce que le moteur a reçu et les événements émis.
fn reprendre_fil(fil: Vec<Message>, de_confiance: bool) -> (Vec<Message>, Vec<Event>) {
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = events.clone();
    let moteur = Rc::new(Espion::default());
    let harness = Harness::new(
        "consigne",
        Limits::default(),
        moteur.clone(),
        Rc::new(Ecrire {
            effets: Cell::new(0),
        }),
        move |e| sink.borrow_mut().push(e),
    );
    let mut sauve = harness.snapshot();
    sauve.transcript = fil;
    if de_confiance {
        harness.restore_trusted(sauve);
    } else {
        harness.restore(sauve);
    }
    futures::executor::block_on(harness.send("et ensuite ?"));
    let fil_envoye = moteur.fils.borrow()[0].clone();
    let evenements = events.borrow().clone();
    (fil_envoye, evenements)
}

fn nb_reponses_c1(fil: &[Message]) -> usize {
    fil.iter()
        .filter(|m| matches!(m, Message::Tool { id, .. } if id == "c1"))
        .count()
}

#[test]
fn la_reprise_dit_a_l_hote_qu_un_appel_a_ete_ferme_sans_preuve() {
    let (_, events) = reprendre_fil(fil_apres_crash(), true);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Hint { kind: HintKind::Interrupted, text } if text.contains("reprise") && text.contains("inconnu")
        )),
        "l'hôte doit savoir qu'un effet est inconnu : {events:?}"
    );
}

/// Témoin : un fil COMPLET (l'appel a sa réponse) n'est pas touché et ne fabrique aucun
/// signalement : la réparation ne vise que les appels réellement ouverts.
#[test]
fn un_fil_complet_n_est_ni_modifie_ni_signale() {
    let mut fil = fil_apres_crash();
    fil.push(Message::tool("c1", "écrit"));
    let (envoye, events) = reprendre_fil(fil, true);
    assert_eq!(nb_reponses_c1(&envoye), 1, "une seule réponse, la vraie");
    assert!(
        !envoye.iter().any(|m| m.text().contains("inconnu")),
        "rien d'inventé : {envoye:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::Hint { text, .. } if text.starts_with("reprise")
        )),
        "aucun faux signalement : {events:?}"
    );
}

/// Un snapshot IMPORTÉ (voie `restore`, non fiable) est réparé de la même façon : le fil
/// refusé par le moteur ne dépend pas de la confiance accordée à l'origine du snapshot.
#[test]
fn un_snapshot_importe_est_repare_comme_un_snapshot_de_confiance() {
    let (envoye, _) = reprendre_fil(fil_apres_crash(), false);
    assert_eq!(
        nb_reponses_c1(&envoye),
        1,
        "l'appel c1 est refermé : {envoye:?}"
    );
}

/// Plusieurs appels ouverts : chacun reçoit SA réponse, aucun n'est oublié.
#[test]
fn plusieurs_appels_ouverts_sont_tous_refermes() {
    let mut fil = fil_apres_crash();
    if let Some(Message::Assistant { calls, .. }) = fil.last_mut() {
        calls.push(ToolCall {
            id: "c2".into(),
            name: "ecrire".into(),
            arguments: json!({}),
        });
    }
    let (envoye, _) = reprendre_fil(fil, true);
    for id in ["c1", "c2"] {
        assert!(
            envoye
                .iter()
                .any(|m| matches!(m, Message::Tool { id: i, .. } if i == id)),
            "{id} doit avoir une réponse : {envoye:?}"
        );
    }
}
