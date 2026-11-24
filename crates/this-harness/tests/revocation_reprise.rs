//! #287 : une RÉVOCATION survit à un redémarrage — et ne peut jamais être contournée par
//! une restauration.
//!
//! Scénario réel : l'hôte A retire un consentement (`set_grants`), puis s'arrête et sauvegarde
//! son `Snapshot`. Au redémarrage, l'hôte B réinstalle SA liste d'avant (qui autorise encore
//! l'outil), puis restaure le snapshot. Le consentement retiré ne doit pas revenir.
//!
//! Le `Snapshot` ne porte que des RÉVOCATIONS, jamais des octrois : une restauration importée
//! ne peut donc rien accorder (pas d'élévation de privilège).

use std::cell::Cell;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_agent::substrate::{Grant, Grants};
use this_harness::{
    Cancel, Declaration, Delta, Failure, Finish, Harness, Limits, Outcome, Reply, Request,
    ToolCall, ToolSpec, Toolbox, Transport,
};

const OUTIL: &str = "ecrire";

/// Demande l'outil une fois, puis conclut.
#[derive(Default)]
struct Demande {
    fait: Cell<bool>,
}

#[async_trait(?Send)]
impl Transport for Demande {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.fait.replace(true) {
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

/// Un outil qui écrit au monde : `effets` compte les exécutions réelles.
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

fn harnais(effets: Rc<Cell<u32>>, installees: Grants) -> Harness {
    Harness::new(
        "consigne",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        Rc::new(Demande::default()),
        Rc::new(Ecrire { effets }),
        |_| {},
    )
    .with_grants(installees)
}

fn autorise() -> Grants {
    let mut g = Grants::new();
    g.put(Grant::granted(OUTIL, "user"));
    g
}

/// Le consentement retiré : même outil, refusé avec sa cause.
fn retire() -> Grants {
    let mut g = Grants::new();
    g.put(Grant::refused(OUTIL, "user", "retrait du consentement"));
    g
}

/// Hôte A : installe `autorise`, puis (si `revoque`) retire le consentement, puis sauvegarde.
/// Hôte B : réinstalle SA liste d'avant (`autorise`), restaure le snapshot de A, relance un tour.
/// Rend le nombre d'exécutions réelles de l'outil chez B.
fn redemarrer(revoque: bool) -> u32 {
    let effets_a = Rc::new(Cell::new(0));
    let a = harnais(effets_a.clone(), autorise());
    if revoque {
        a.set_grants(retire());
    }
    let sauve = a.snapshot();

    let effets_b = Rc::new(Cell::new(0));
    let b = harnais(effets_b.clone(), autorise());
    b.restore_trusted(sauve);
    futures::executor::block_on(b.send("écris"));
    effets_b.get()
}

#[test]
fn une_revocation_ne_revient_pas_apres_redemarrage_et_restauration() {
    assert_eq!(
        redemarrer(true),
        0,
        "consentement retiré avant l'arrêt : l'outil ne doit PAS s'exécuter chez B"
    );
}

/// Témoin : sans révocation, le même scénario exécute l'outil. Le refus précédent vient donc
/// bien de la révocation, pas d'un défaut de la mise en scène.
#[test]
fn temoin_sans_revocation_l_outil_s_execute_apres_redemarrage() {
    assert_eq!(
        redemarrer(false),
        1,
        "consentement intact : l'outil s'exécute"
    );
}

/// Une restauration ne peut RIEN accorder : un hôte sans consentement n'en reçoit pas via un
/// snapshot. (Le snapshot ne porte que des révocations.)
#[test]
fn une_restauration_n_octroie_jamais() {
    let effets = Rc::new(Cell::new(0));
    let b = harnais(effets.clone(), Grants::new()); // aucun grant installé pour ecrire
    b.restore_trusted(Default::default());
    futures::executor::block_on(b.send("écris"));
    assert_eq!(effets.get(), 0, "rien n'est accordé par une restauration");
}
