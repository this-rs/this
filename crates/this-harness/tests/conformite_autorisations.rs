//! Suite de conformité des autorisations (issue #287) : des TESTS DE REFUS.
//!
//! Les mécanismes existent déjà (`Permission`, `Toolbox::grants`, l'approbation #175,
//! les droits actualisés #276) ; ce banc ne les recrée pas. Il vérifie, cas par cas
//! du cahier, qu'un refus tient : l'outil refusé n'est jamais exécuté, le refus est
//! journalisé, et ni un changement de moteur ni un nouveau tour ne rétablissent un
//! consentement retiré.
//!
//! Chaque test compte les effets réels de la doublure : un refus qui « dit non » mais
//! exécute quand même serait pire que pas de refus.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use async_trait::async_trait;
use serde_json::json;

use this_agent::substrate::{Grant, Grants};
use this_harness::{
    Cancel, Declaration, Delta, Event, Failure, Finish, Harness, Limits, Outcome, Permission,
    Recorder, Reply, Request, Run, Step, Stop, ToolCall, ToolSpec, Toolbox, Toolboxes, Transport,
};

const OUTIL: &str = "ecrire";

/// Un outil qui écrit au monde. `declare` dit si `declarations()` le déclare encore :
/// c'est la seule entrée de la porte relue à CHAQUE appel (`gate` interroge la
/// toolbox au moment de l'appel). `effets` compte les écritures réelles ;
/// `appels_cancel` compte les appels par `call_with_cancel`, le chemin de la boucle.
struct Outil {
    declare: Cell<bool>,
    effets: Cell<u32>,
    appels_cancel: Cell<u32>,
}

impl Outil {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            declare: Cell::new(true),
            effets: Cell::new(0),
            appels_cancel: Cell::new(0),
        })
    }
}

#[async_trait(?Send)]
impl Toolbox for Outil {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: OUTIL.into(),
            description: "écrit".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    fn declarations(&self) -> Vec<Declaration> {
        if self.declare.get() {
            vec![Declaration::writes(OUTIL)]
        } else {
            vec![]
        }
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        self.effets.set(self.effets.get() + 1);
        Outcome::ok("écrit")
    }
    async fn call_with_cancel(&self, call: &ToolCall, _: &Cancel) -> Outcome {
        self.appels_cancel.set(self.appels_cancel.get() + 1);
        self.call(call).await
    }
}

/// Un journal de `Recorder` : garde les étapes et les arrêts poussés par la boucle.
#[derive(Default)]
struct Journal {
    steps: RefCell<Vec<Step>>,
}

#[async_trait(?Send)]
impl Recorder for Journal {
    async fn step(&self, _run_id: &str, _run: &Run, step: &Step) {
        self.steps.borrow_mut().push(step.clone());
    }
    async fn stopped(&self, _run_id: &str, _run: &Run, _stop: &Stop) {}
}

/// Un moteur scripté : chaque `stream` rend la réponse suivante ; à court de script,
/// il conclut. Il ne décide rien seul : c'est la boucle qui exécute ou refuse.
struct Script {
    replies: RefCell<VecDeque<Reply>>,
    /// Les noms d'outils offerts à chaque tour (l'annonce).
    offered: RefCell<Vec<Vec<String>>>,
}

impl Script {
    fn new(replies: Vec<Reply>) -> Rc<Self> {
        Rc::new(Self {
            replies: RefCell::new(replies.into()),
            offered: RefCell::new(Vec::new()),
        })
    }
}

fn demande() -> Reply {
    Reply {
        usage: None,
        served: None,
        text: String::new(),
        calls: vec![ToolCall {
            id: "c1".into(),
            name: OUTIL.into(),
            arguments: json!({}),
        }],
        finish: Finish::Tools,
    }
}

fn fini() -> Reply {
    Reply {
        usage: None,
        served: None,
        text: "fini".into(),
        calls: vec![],
        finish: Finish::Stop,
    }
}

#[async_trait(?Send)]
impl Transport for Script {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.offered
            .borrow_mut()
            .push(request.tools().iter().map(|s| s.name.clone()).collect());
        Ok(self.replies.borrow_mut().pop_front().unwrap_or_else(fini))
    }
}

fn refus_de_la_boucle(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::ToolResult {
            refused: true,
            text,
            ..
        } => Some(text.clone()),
        _ => None,
    })
}

// ---- Cas 1 : refus sur un outil exécuté par la boucle -----------------------

/// `Permission::allows` refuse (droit explicitement retiré, motif donné) : `call_with_cancel`
/// n'est jamais invoqué, la toolbox ne produit aucun effet, et le refus est journalisé.
#[test]
fn cas1_refus_de_la_boucle_n_appelle_jamais_l_outil() {
    let outil = Outil::new();
    let script = Script::new(vec![demande(), fini()]);
    let journal = Rc::new(Journal::default());
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();

    let mut grants = Grants::new();
    grants.put(Grant::refused(OUTIL, "user", "écriture non consentie"));
    let harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        script,
        outil.clone(),
        move |e| sink.borrow_mut().push(e),
    )
    .with_grants(grants)
    .with_recorder(journal.clone());

    futures::executor::block_on(harness.send("écris"));

    assert_eq!(
        outil.appels_cancel.get(),
        0,
        "call_with_cancel ne doit pas partir"
    );
    assert_eq!(outil.effets.get(), 0, "aucun effet dans la toolbox");
    let events = log.borrow().clone();
    let texte = refus_de_la_boucle(&events).expect("un ToolResult de refus est émis");
    assert!(texte.contains(OUTIL), "{texte}");
    let steps = journal.steps.borrow();
    assert_eq!(steps.len(), 1, "une étape journalisée pour l'appel refusé");
    assert!(steps[0].refused, "l'étape est marquée refusée");
    assert!(
        steps[0]
            .motif
            .as_deref()
            .unwrap_or("")
            .contains("écriture non consentie"),
        "motif : {:?}",
        steps[0].motif
    );
}

/// Témoin : le même outil, autorisé, est bien exécuté. Sans lui, le refus ci-dessus
/// pourrait venir d'un câblage cassé plutôt que de la porte.
#[test]
fn cas1_temoin_outil_autorise_est_execute() {
    let outil = Outil::new();
    let script = Script::new(vec![demande(), fini()]);
    let mut grants = Grants::new();
    grants.put(Grant::granted(OUTIL, "user"));
    let harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        script,
        outil.clone(),
        |_| {},
    )
    .with_grants(grants);
    futures::executor::block_on(harness.send("écris"));
    assert_eq!(outil.effets.get(), 1);
}

// ---- Cas 2 : refus sur un moteur à outils internes (famille A) ---------------

/// Un transport de la famille A : il exécute l'outil LUI-MÊME pendant `stream`. Il doit
/// consulter `Permission::allows` avant, et ne rien exécuter sur refus.
struct MoteurInterne {
    permission: RefCell<Option<Rc<dyn Permission>>>,
    outil: Rc<Outil>,
    /// Le motif renvoyé par la porte, s'il y en a eu un.
    motif: RefCell<Option<String>>,
    /// Le moteur ne demande l'outil qu'une fois, comme un vrai moteur de la famille A.
    asked: Cell<bool>,
}

#[async_trait(?Send)]
impl Transport for MoteurInterne {
    fn set_permission(&self, permission: Rc<dyn Permission>) {
        *self.permission.borrow_mut() = Some(permission);
    }

    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.asked.replace(true) {
            return Ok(fini());
        }
        let permission = self
            .permission
            .borrow()
            .clone()
            .expect("porte posée avant le flux");
        let call = ToolCall {
            id: "t1".into(),
            name: OUTIL.into(),
            arguments: json!({}),
        };
        match permission.allows(&call.name) {
            Ok(()) => {
                let outcome = self.outil.call(&call).await;
                permission.record(&call, 2, &outcome).await;
            }
            Err(motif) => {
                *self.motif.borrow_mut() = Some(motif.clone());
                let outcome =
                    Outcome::refused(format!("outil non autorisé : {}", call.name), motif);
                permission.record(&call, 0, &outcome).await;
            }
        }
        Ok(fini())
    }
}

#[test]
fn cas2_moteur_interne_refuse_n_execute_jamais_l_outil() {
    let outil = Outil::new();
    let moteur = Rc::new(MoteurInterne {
        permission: RefCell::new(None),
        outil: outil.clone(),
        motif: RefCell::new(None),
        asked: Cell::new(false),
    });
    let journal = Rc::new(Journal::default());
    let mut grants = Grants::new();
    grants.put(Grant::refused(OUTIL, "user", "écriture non consentie"));
    let harness = Harness::new(
        "sys",
        Limits::default(),
        moteur.clone(),
        outil.clone(),
        |_| {},
    )
    .with_grants(grants)
    .with_recorder(journal.clone());

    futures::executor::block_on(harness.send("écris"));

    assert_eq!(
        outil.effets.get(),
        0,
        "le moteur ne doit pas exécuter l'outil refusé"
    );
    let motif = moteur.motif.borrow().clone().unwrap_or_default();
    assert!(
        motif.contains("écriture non consentie"),
        "le transport a consulté la porte et reçu le motif : {motif}"
    );
    let steps = journal.steps.borrow();
    assert_eq!(steps.len(), 1);
    assert!(steps[0].refused, "le refus est journalisé hors-bande");
}

/// Témoin famille A : autorisé, le moteur exécute et l'effet a lieu.
#[test]
fn cas2_temoin_moteur_interne_autorise_execute() {
    let outil = Outil::new();
    let moteur = Rc::new(MoteurInterne {
        permission: RefCell::new(None),
        outil: outil.clone(),
        motif: RefCell::new(None),
        asked: Cell::new(false),
    });
    let mut grants = Grants::new();
    grants.put(Grant::granted(OUTIL, "user"));
    let harness =
        Harness::new("sys", Limits::default(), moteur, outil.clone(), |_| {}).with_grants(grants);
    futures::executor::block_on(harness.send("écris"));
    assert_eq!(outil.effets.get(), 1);
}

// ---- Cas 3 : droit révoqué entre l'annonce et l'appel -----------------------

/// Un moteur qui consulte l'annonce (`request.tools()`), puis retire la déclaration
/// d'effet de l'outil pendant son flux, puis réclame l'outil.
struct RevoqueApresAnnonce {
    outil: Rc<Outil>,
    /// `false` pour le témoin : même scénario, sans la révocation.
    revoque: bool,
    annonce: RefCell<Vec<String>>,
    demande_faite: Cell<bool>,
}

#[async_trait(?Send)]
impl Transport for RevoqueApresAnnonce {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.demande_faite.replace(true) {
            return Ok(fini());
        }
        *self.annonce.borrow_mut() = request.tools().iter().map(|s| s.name.clone()).collect();
        // La révocation tombe entre l'annonce (ci-dessus) et l'appel (ci-dessous).
        if self.revoque {
            self.outil.declare.set(false);
        }
        Ok(demande())
    }
}

#[test]
fn cas3_droit_revoque_entre_annonce_et_appel_est_refuse() {
    let outil = Outil::new();
    let moteur = Rc::new(RevoqueApresAnnonce {
        outil: outil.clone(),
        revoque: true,
        annonce: RefCell::new(Vec::new()),
        demande_faite: Cell::new(false),
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut grants = Grants::new();
    grants.put(Grant::granted(OUTIL, "user"));
    let harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        moteur.clone(),
        outil.clone(),
        move |e| sink.borrow_mut().push(e),
    )
    .with_grants(grants);

    futures::executor::block_on(harness.send("écris"));

    assert_eq!(
        *moteur.annonce.borrow(),
        vec![OUTIL.to_string()],
        "précondition : l'outil a bien été annoncé au tour"
    );
    assert_eq!(
        outil.effets.get(),
        0,
        "révoqué avant l'appel : jamais exécuté"
    );
    assert_eq!(outil.appels_cancel.get(), 0);
    assert!(
        refus_de_la_boucle(&log.borrow()).is_some(),
        "le refus est émis"
    );
}

/// Témoin : sans la révocation, le même scénario exécute l'outil. Le refus précédent
/// vient donc bien de la révocation et non d'un défaut de la mise en scène.
#[test]
fn cas3_temoin_sans_revocation_l_outil_est_execute() {
    let outil = Outil::new();
    let moteur = Rc::new(RevoqueApresAnnonce {
        outil: outil.clone(),
        revoque: false,
        annonce: RefCell::new(Vec::new()),
        demande_faite: Cell::new(false),
    });
    let mut grants = Grants::new();
    grants.put(Grant::granted(OUTIL, "user"));
    let harness =
        Harness::new("sys", Limits::default(), moteur, outil.clone(), |_| {}).with_grants(grants);
    futures::executor::block_on(harness.send("écris"));
    assert_eq!(
        outil.effets.get(),
        1,
        "sans révocation, l'outil annoncé s'exécute"
    );
}

// ---- Cas 4 : reprise / changement de moteur ne rétablit pas un consentement révoqué

/// Un consentement accordé dans un premier harnais, retiré ensuite (`put` refusé à la
/// même place), puis un NOUVEAU harnais avec un NOUVEAU moteur : l'outil ne repart pas.
/// Un nouveau tour sur le même harnais ne le relance pas non plus.
#[test]
fn cas4_consentement_revoque_ne_repart_ni_au_nouveau_tour_ni_au_nouveau_moteur() {
    let outil = Outil::new();

    // 1. Consentement accordé : l'outil s'exécute une fois.
    let mut accorde = Grants::new();
    accorde.put(Grant::granted(OUTIL, "user"));
    let premier = Harness::new(
        "sys",
        Limits::default(),
        Script::new(vec![demande(), fini()]),
        outil.clone(),
        |_| {},
    )
    .with_grants(accorde.clone());
    futures::executor::block_on(premier.send("écris"));
    assert_eq!(
        outil.effets.get(),
        1,
        "précondition : le consentement a joué une fois"
    );

    // 2. Révocation : même outil, même place, désormais refusé.
    let mut revoque = accorde;
    revoque.put(Grant::refused(OUTIL, "user", "consentement retiré"));

    // 3. Nouveau moteur, nouveau harnais, deux tours (le second est un nouvel objectif).
    let journal = Rc::new(Journal::default());
    let nouveau = Harness::new(
        "sys",
        Limits::default(),
        Script::new(vec![demande(), fini(), demande(), fini()]),
        outil.clone(),
        |_| {},
    )
    .with_grants(revoque)
    .with_recorder(journal.clone());
    futures::executor::block_on(nouveau.send("écris, tour 1"));
    futures::executor::block_on(nouveau.send("écris, tour 2"));

    assert_eq!(
        outil.effets.get(),
        1,
        "un nouveau moteur ni un nouveau tour ne relancent l'outil révoqué"
    );
    let steps = journal.steps.borrow();
    assert!(
        steps.iter().all(|s| s.refused),
        "chaque tentative est refusée : {steps:?}"
    );
    assert!(!steps.is_empty(), "les tentatives sont journalisées");
}

// ---- Cas 5 : enfant (sous-agent) -------------------------------------------
//
// Non couvert : le crate `this-harness` n'expose aucun sous-agent ni de mécanisme de
// délégation. Le seul mot « sous-agents » figure dans un commentaire de module
// (`session.rs`), pas dans du code. Un test pour un enfant serait fabriqué ; il n'est
// donc pas écrit.

// ---- Cas 3 bis : grant DYNAMIQUE retiré entre annonce et appel (#287, manque nommé) ----
//
// `cas3` joue le retrait d'une DÉCLARATION. Le retrait d'un GRANT passe par une autre
// source : `Toolbox::grants()`, relu à CHAQUE appel et prioritaire sur `with_grants`.
// Ce cas est le seul qui la fait varier pendant le tour ; sans lui, une porte qui
// ignorerait `Toolbox::grants()` passerait toute la suite au vert.

/// Outillage dont les grants dynamiques viennent d'un ensemble que le moteur peut
/// modifier pendant le tour. Les effets restent comptés par l'`Outil` enveloppé.
struct OutilGrantsDynamiques {
    outil: Rc<Outil>,
    grants: Rc<RefCell<Grants>>,
}

#[async_trait(?Send)]
impl Toolbox for OutilGrantsDynamiques {
    fn specs(&self) -> Vec<ToolSpec> {
        self.outil.specs()
    }
    fn declarations(&self) -> Vec<Declaration> {
        self.outil.declarations()
    }
    fn grants(&self) -> Option<Grants> {
        Some(self.grants.borrow().clone())
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        self.outil.call(call).await
    }
    async fn call_with_cancel(&self, call: &ToolCall, cancel: &Cancel) -> Outcome {
        self.outil.call_with_cancel(call, cancel).await
    }
}

/// Un moteur qui annonce l'outil, retire son grant dynamique, puis réclame l'outil.
struct RetireGrantApresAnnonce {
    grants: Rc<RefCell<Grants>>,
    /// `false` pour le témoin : même scénario, sans le retrait.
    retire: bool,
    demande_faite: Cell<bool>,
}

#[async_trait(?Send)]
impl Transport for RetireGrantApresAnnonce {
    async fn stream(&self, _: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.demande_faite.replace(true) {
            return Ok(fini());
        }
        if self.retire {
            self.grants
                .borrow_mut()
                .put(Grant::refused(OUTIL, "user", "consentement retiré"));
        }
        Ok(demande())
    }
}

#[test]
fn cas3_bis_grant_dynamique_retire_entre_annonce_et_appel_est_refuse() {
    let outil = Outil::new();
    let grants = Rc::new(RefCell::new({
        let mut g = Grants::new();
        g.put(Grant::granted(OUTIL, "user"));
        g
    }));
    let moteur = Rc::new(RetireGrantApresAnnonce {
        grants: grants.clone(),
        retire: true,
        demande_faite: Cell::new(false),
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let toolbox = Rc::new(OutilGrantsDynamiques {
        outil: outil.clone(),
        grants: grants.clone(),
    });
    let harness = Harness::new(
        "sys",
        Limits {
            max_relances: 0,
            ..Limits::default()
        },
        moteur,
        toolbox,
        move |e| sink.borrow_mut().push(e),
    );

    futures::executor::block_on(harness.send("écris"));

    assert_eq!(
        outil.effets.get(),
        0,
        "grant dynamique retiré avant l'appel : l'outil ne s'exécute pas"
    );
    assert!(
        refus_de_la_boucle(&log.borrow()).is_some(),
        "le refus est émis"
    );
}

/// Témoin : même montage, grant dynamique jamais retiré → l'outil s'exécute. Le refus
/// précédent vient donc du retrait, pas d'un défaut de la mise en scène.
#[test]
fn cas3_bis_temoin_grant_dynamique_intact_l_outil_est_execute() {
    let outil = Outil::new();
    let grants = Rc::new(RefCell::new({
        let mut g = Grants::new();
        g.put(Grant::granted(OUTIL, "user"));
        g
    }));
    let moteur = Rc::new(RetireGrantApresAnnonce {
        grants: grants.clone(),
        retire: false,
        demande_faite: Cell::new(false),
    });
    let toolbox = Rc::new(OutilGrantsDynamiques {
        outil: outil.clone(),
        grants,
    });
    let harness = Harness::new("sys", Limits::default(), moteur, toolbox, |_| {});
    futures::executor::block_on(harness.send("écris"));
    assert_eq!(outil.effets.get(), 1, "sans retrait, l'outil s'exécute");
}

// ---- Cas 3 ter : GRANT INSTALLÉ retiré entre l'annonce et l'appel -----------
//
// `cas3` retire une DÉCLARATION, `cas3_bis` un grant DYNAMIQUE (`Toolbox::grants`).
// Reste la troisième source d'autorité : les grants INSTALLÉS par l'hôte
// (`with_grants`), qu'une révocation atteint par `set_grants` — le chemin par lequel
// une autre partie prenante peut retirer un consentement PENDANT le tour.

/// Un moteur qui annonce l'outil, puis retire le grant installé par l'hôte
/// (`set_grants`), puis réclame l'outil. Le harnais lui est donné en `Weak` : le
/// moteur existe avant le harnais qui le possède (pas de cycle).
struct RetireGrantInstalleApresAnnonce {
    harnais: RefCell<Option<Weak<Harness>>>,
    /// `false` pour le témoin : même scénario, sans la révocation.
    retire: bool,
    annonce: RefCell<Vec<String>>,
    demande_faite: Cell<bool>,
}

impl RetireGrantInstalleApresAnnonce {
    fn new(retire: bool) -> Rc<Self> {
        Rc::new(Self {
            harnais: RefCell::new(None),
            retire,
            annonce: RefCell::new(Vec::new()),
            demande_faite: Cell::new(false),
        })
    }

    /// Le consentement retiré, poussé par l'hôte au harnais vivant.
    fn revoquer(&self) {
        let harnais = self
            .harnais
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)
            .expect("le harnais est posé avant le tour");
        let mut revoque = Grants::new();
        revoque.put(Grant::refused(OUTIL, "user", "consentement retiré"));
        harnais.set_grants(revoque);
    }
}

#[async_trait(?Send)]
impl Transport for RetireGrantInstalleApresAnnonce {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.demande_faite.replace(true) {
            return Ok(fini());
        }
        *self.annonce.borrow_mut() = request.tools().iter().map(|s| s.name.clone()).collect();
        // La révocation tombe entre l'annonce (ci-dessus) et l'appel (la boucle).
        if self.retire {
            self.revoquer();
        }
        Ok(demande())
    }
}

fn harnais_avec_moteur_qui_revoque(
    moteur: &Rc<RetireGrantInstalleApresAnnonce>,
    outil: &Rc<Outil>,
    log: Rc<RefCell<Vec<Event>>>,
) -> Rc<Harness> {
    let sink = log;
    let mut grants = Grants::new();
    grants.put(Grant::granted(OUTIL, "user"));
    let harnais = Rc::new(
        Harness::new(
            "sys",
            Limits {
                max_relances: 0,
                ..Limits::default()
            },
            moteur.clone(),
            outil.clone(),
            move |e| sink.borrow_mut().push(e),
        )
        .with_grants(grants),
    );
    *moteur.harnais.borrow_mut() = Some(Rc::downgrade(&harnais));
    harnais
}

#[test]
fn cas3_ter_grant_installe_retire_entre_annonce_et_appel_est_refuse() {
    let outil = Outil::new();
    let moteur = RetireGrantInstalleApresAnnonce::new(true);
    let log = Rc::new(RefCell::new(Vec::new()));
    let harnais = harnais_avec_moteur_qui_revoque(&moteur, &outil, log.clone());

    futures::executor::block_on(harnais.send("écris"));

    assert_eq!(
        *moteur.annonce.borrow(),
        vec![OUTIL.to_string()],
        "précondition : l'outil a bien été annoncé au tour"
    );
    assert_eq!(
        outil.effets.get(),
        0,
        "grant installé retiré avant l'appel : l'outil ne s'exécute pas"
    );
    assert!(
        refus_de_la_boucle(&log.borrow()).is_some(),
        "le refus est émis"
    );
}

/// Témoin : sans la révocation, le même montage exécute l'outil.
#[test]
fn cas3_ter_temoin_grant_installe_intact_l_outil_est_execute() {
    let outil = Outil::new();
    let moteur = RetireGrantInstalleApresAnnonce::new(false);
    let harnais =
        harnais_avec_moteur_qui_revoque(&moteur, &outil, Rc::new(RefCell::new(Vec::new())));
    futures::executor::block_on(harnais.send("écris"));
    assert_eq!(outil.effets.get(), 1, "sans révocation, l'outil s'exécute");
}

// ---- Cas 3 quater : famille A — la porte du run lit-elle les grants VIVANTS ? --
//
// La même révocation, mais l'outil est exécuté par le MOTEUR pendant `stream`. La
// porte qu'il reçoit (`Permission`) doit rendre le même refus que la boucle (case b
// du cahier) : une révocation faite pendant le flux doit l'atteindre, pas attendre
// le run suivant.

/// Un transport de la famille A qui annonce, révoque, PUIS interroge la porte.
struct MoteurInterneQuiRevoquePendantLeFlux {
    harnais: RefCell<Option<Weak<Harness>>>,
    outil: Rc<Outil>,
    retire: bool,
    permission: RefCell<Option<Rc<dyn Permission>>>,
    motif: RefCell<Option<String>>,
    annonce: RefCell<Vec<String>>,
    asked: Cell<bool>,
}

impl MoteurInterneQuiRevoquePendantLeFlux {
    fn new(retire: bool, outil: Rc<Outil>) -> Rc<Self> {
        Rc::new(Self {
            harnais: RefCell::new(None),
            outil,
            retire,
            permission: RefCell::new(None),
            motif: RefCell::new(None),
            annonce: RefCell::new(Vec::new()),
            asked: Cell::new(false),
        })
    }
}

#[async_trait(?Send)]
impl Transport for MoteurInterneQuiRevoquePendantLeFlux {
    fn set_permission(&self, permission: Rc<dyn Permission>) {
        *self.permission.borrow_mut() = Some(permission);
    }

    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        if self.asked.replace(true) {
            return Ok(fini());
        }
        *self.annonce.borrow_mut() = request.tools().iter().map(|s| s.name.clone()).collect();
        if self.retire {
            let harnais = self
                .harnais
                .borrow()
                .as_ref()
                .and_then(Weak::upgrade)
                .expect("le harnais est posé avant le tour");
            let mut revoque = Grants::new();
            revoque.put(Grant::refused(OUTIL, "user", "consentement retiré"));
            harnais.set_grants(revoque);
        }
        let permission = self
            .permission
            .borrow()
            .clone()
            .expect("porte posée avant le flux");
        let call = ToolCall {
            id: "t1".into(),
            name: OUTIL.into(),
            arguments: json!({}),
        };
        match permission.allows(&call.name) {
            Ok(()) => {
                let outcome = self.outil.call(&call).await;
                permission.record(&call, 2, &outcome).await;
            }
            Err(motif) => {
                *self.motif.borrow_mut() = Some(motif.clone());
                let outcome =
                    Outcome::refused(format!("outil non autorisé : {}", call.name), motif);
                permission.record(&call, 0, &outcome).await;
            }
        }
        Ok(fini())
    }
}

fn harnais_avec_moteur_interne(
    moteur: &Rc<MoteurInterneQuiRevoquePendantLeFlux>,
    outil: &Rc<Outil>,
    log: Rc<RefCell<Vec<Event>>>,
) -> Rc<Harness> {
    let mut grants = Grants::new();
    grants.put(Grant::granted(OUTIL, "user"));
    let harnais = Rc::new(
        Harness::new(
            "sys",
            Limits::default(),
            moteur.clone(),
            outil.clone(),
            move |e| log.borrow_mut().push(e),
        )
        .with_grants(grants),
    );
    *moteur.harnais.borrow_mut() = Some(Rc::downgrade(&harnais));
    harnais
}

#[test]
fn cas3_quater_famille_a_voit_une_revocation_faite_pendant_son_flux() {
    let outil = Outil::new();
    let moteur = MoteurInterneQuiRevoquePendantLeFlux::new(true, outil.clone());
    let log = Rc::new(RefCell::new(Vec::new()));
    let harnais = harnais_avec_moteur_interne(&moteur, &outil, log.clone());

    futures::executor::block_on(harnais.send("écris"));

    assert_eq!(
        *moteur.annonce.borrow(),
        vec![OUTIL.to_string()],
        "précondition : l'outil a bien été annoncé au tour"
    );
    assert_eq!(
        outil.effets.get(),
        0,
        "la porte du flux doit voir la révocation faite pendant le flux : \
         elle ne peut pas rendre un accord que la boucle refuse au même instant"
    );
    let motif = moteur.motif.borrow().clone().unwrap_or_default();
    assert!(
        motif.contains("consentement retiré"),
        "le refus rendu au moteur porte le motif de l'hôte : {motif}"
    );
}

/// Témoin : sans la révocation, le moteur exécute et l'effet a lieu.
#[test]
fn cas3_quater_temoin_famille_a_sans_revocation_execute() {
    let outil = Outil::new();
    let moteur = MoteurInterneQuiRevoquePendantLeFlux::new(false, outil.clone());
    let harnais = harnais_avec_moteur_interne(&moteur, &outil, Rc::new(RefCell::new(Vec::new())));
    futures::executor::block_on(harnais.send("écris"));
    assert_eq!(outil.effets.get(), 1);
}

// ---- Cas 5 : la COMPOSITION `Toolboxes` — les droits suivent la PROPRIÉTÉ de l'outil ----
//
// Mesuré le 24/11/2026 sur `787a988` : `impl Toolbox for Toolboxes` définissait `specs`,
// `declarations`, `vus`, `call`, `call_with_cancel` — mais PAS `grants()`. La composition
// rendait donc le défaut du trait (`None`), et les droits tenus par une part n'atteignaient
// JAMAIS la porte : un outil RÉVOQUÉ par sa part était refusé seul (effets 0) et exécuté
// composé (effets 1). Une révocation qui disparaît est un octroi.
//
// RÈGLE APPLIQUÉE (décision du 24/11/2026, `docs/harnais-autorisations.md` §4) : la
// composition porte les droits de chaque part POUR SES PROPRES OUTILS SEULEMENT, le
// propriétaire d'un nom étant la première part qui l'expose — la règle déjà en vigueur
// pour `specs`, les `declarations` et le routage de l'appel. Aucune part ne peut donc
// accorder ni révoquer un outil qu'une autre exécute. « La première gagne » serait un
// choix arbitraire (l'ordre de composition) là où la propriété de l'outil ne l'est pas.

fn outillage_qui_refuse(grants: Rc<RefCell<Grants>>) -> Rc<OutilGrantsDynamiques> {
    Rc::new(OutilGrantsDynamiques {
        outil: Outil::new(),
        grants,
    })
}

/// Une part qui n'expose AUCUN outil mais qui porte des droits : elle ne doit rien pouvoir
/// ressusciter, et c'est le cas qui distingue « filtrer par propriété » d'un simple `put`.
struct MuetteQuiAccorde {
    grants: Rc<RefCell<Grants>>,
}

#[async_trait(?Send)]
impl Toolbox for MuetteQuiAccorde {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }
    fn grants(&self) -> Option<Grants> {
        Some(self.grants.borrow().clone())
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::refused("muette", "cette part n'expose aucun outil")
    }
    async fn call_with_cancel(&self, call: &ToolCall, _: &Cancel) -> Outcome {
        self.call(call).await
    }
}

/// Une part qui expose l'outil et ne porte AUCUN droit (le défaut du trait) : elle ne doit
/// pas rendre « rien n'est vérifié » à une composition où une autre part accorde hors domaine.
struct OutilSansDroits {
    outil: Rc<Outil>,
}

#[async_trait(?Send)]
impl Toolbox for OutilSansDroits {
    fn specs(&self) -> Vec<ToolSpec> {
        self.outil.specs()
    }
    fn declarations(&self) -> Vec<Declaration> {
        self.outil.declarations()
    }
    async fn call(&self, call: &ToolCall) -> Outcome {
        self.outil.call(call).await
    }
    async fn call_with_cancel(&self, call: &ToolCall, cancel: &Cancel) -> Outcome {
        self.outil.call_with_cancel(call, cancel).await
    }
}

#[test]
fn cas5_un_outillage_seul_porte_ses_grants_dynamiques() {
    let grants = Rc::new(RefCell::new(Grants::new()));
    grants
        .borrow_mut()
        .put(Grant::refused(OUTIL, "user", "consentement retiré"));
    let outillage = outillage_qui_refuse(grants);
    let outil = outillage.outil.clone();
    let harnais = Harness::new(
        "sys",
        Limits::default(),
        Script::new(vec![demande(), fini()]),
        outillage,
        |_| {},
    );
    futures::executor::block_on(harnais.send("écris"));
    assert_eq!(
        outil.effets.get(),
        0,
        "témoin : seul, l'outillage fait foi et le refus tient"
    );
}

/// LE TROU MESURÉ, refermé : la révocation d'une part survit à la composition.
#[test]
fn cas5_compose_l_outillage_ne_perd_pas_sa_revocation() {
    let grants = Rc::new(RefCell::new(Grants::new()));
    grants
        .borrow_mut()
        .put(Grant::refused(OUTIL, "user", "consentement retiré"));
    let outillage = outillage_qui_refuse(grants);
    let outil = outillage.outil.clone();
    let composes = Rc::new(Toolboxes::new().with(outillage));
    let harnais = Harness::new(
        "sys",
        Limits::default(),
        Script::new(vec![demande(), fini()]),
        composes,
        |_| {},
    );
    futures::executor::block_on(harnais.send("écris"));
    assert_eq!(
        outil.effets.get(),
        0,
        "composé, l'outil reste refusé : la révocation portée par sa part doit atteindre \
         la porte. Effets 1 = le refus a disparu en entrant dans la composition"
    );
}

/// Une part ne décide que de SES outils : accorder celui d'une autre n'a aucun effet.
#[test]
fn cas5_une_part_ne_peut_pas_accorder_l_outil_d_une_autre() {
    // La première part expose et RÉVOQUE `ecrire` : elle en est propriétaire.
    let refus = Rc::new(RefCell::new(Grants::new()));
    refus
        .borrow_mut()
        .put(Grant::refused(OUTIL, "user", "consentement retiré"));
    let proprietaire = outillage_qui_refuse(refus);
    let outil = proprietaire.outil.clone();
    // La seconde porte un OCTROI du même nom : il ne doit rien ressusciter.
    let octroi = Rc::new(RefCell::new(Grants::new()));
    octroi.borrow_mut().put(Grant::granted(OUTIL, "user"));
    let usurpateur = outillage_qui_refuse(octroi);
    let composes = Rc::new(Toolboxes::new().with(proprietaire).with(usurpateur));
    let harnais = Harness::new(
        "sys",
        Limits::default(),
        Script::new(vec![demande(), fini()]),
        composes,
        |_| {},
    );
    futures::executor::block_on(harnais.send("écris"));
    assert_eq!(
        outil.effets.get(),
        0,
        "l'octroi d'une part non propriétaire de l'outil n'a aucun effet : sinon, \
         n'importe quel module ajouté à la composition pourrait rouvrir un outil révoqué"
    );
}

/// Même chose, sans même prétendre exposer l'outil : un `put` naïf le ressusciterait.
#[test]
fn cas5_une_part_qui_n_expose_rien_ne_ressuscite_aucun_droit() {
    let refus = Rc::new(RefCell::new(Grants::new()));
    refus
        .borrow_mut()
        .put(Grant::refused(OUTIL, "user", "consentement retiré"));
    let proprietaire = outillage_qui_refuse(refus);
    let outil = proprietaire.outil.clone();
    let muette = Rc::new(MuetteQuiAccorde {
        grants: Rc::new(RefCell::new({
            let mut g = Grants::new();
            g.put(Grant::granted(OUTIL, "user"));
            g
        })),
    });
    let composes = Rc::new(Toolboxes::new().with(proprietaire).with(muette));
    let harnais = Harness::new(
        "sys",
        Limits::default(),
        Script::new(vec![demande(), fini()]),
        composes,
        |_| {},
    );
    futures::executor::block_on(harnais.send("écris"));
    assert_eq!(
        outil.effets.get(),
        0,
        "une part qui n'expose aucun outil ne peut accorder aucun droit"
    );
}

/// Le défaut fermé : des droits qui ne visent que les outils d'une AUTRE part laissent la
/// composition fermée (`Some` vide), jamais « rien n'est vérifié » (`None`, qui ouvrirait tout).
#[test]
fn cas5_des_droits_hors_domaine_laissent_la_composition_fermee() {
    let sans_droits = Rc::new(OutilSansDroits {
        outil: Outil::new(),
    });
    let outil = sans_droits.outil.clone();
    let muette = Rc::new(MuetteQuiAccorde {
        grants: Rc::new(RefCell::new({
            let mut g = Grants::new();
            g.put(Grant::granted(OUTIL, "user"));
            g
        })),
    });
    let composes = Rc::new(
        Toolboxes::new()
            .with(sans_droits as Rc<dyn Toolbox>)
            .with(muette as Rc<dyn Toolbox>),
    );
    let harnais = Harness::new(
        "sys",
        Limits::default(),
        Script::new(vec![demande(), fini()]),
        composes,
        |_| {},
    );
    futures::executor::block_on(harnais.send("écris"));
    assert_eq!(
        outil.effets.get(),
        0,
        "un droit hors domaine ne doit pas se transformer en « aucun droit vérifié » : \
         la composition rend une liste VIDE (tout refusé), pas `None` (tout permis)"
    );
}
