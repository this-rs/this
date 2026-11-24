//! Le budget de contexte porte sur la REQUÊTE envoyée, pas sur le fil seul.
//!
//! #292 avait réservé la consigne système et le contexte éphémère, mais les
//! **schémas d'outils** (`Toolbox::specs`, envoyés à chaque tour, `wire.rs`) restaient
//! hors du compte : `fit` recevait un budget qui ne les voyait pas. Un hôte à la surface
//! d'outils large partait donc sous « budget respecté » avec une requête plus grosse que
//! la fenêtre du moteur — et l'échec se lisait comme une panne du modèle.
//!
//! Ces tests mesurent la requête **telle qu'elle est partie** (fil + schémas), pas une
//! estimation intermédiaire : c'est le seul chiffre que le moteur voit.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_harness::{
    Cancel, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome, Reply, Request,
    ToolCall, ToolSpec, Toolbox, Transport, TurnContext, estimate_request_tokens, estimate_tokens,
};

/// Le coût de la requête, mesuré ICI, indépendamment du code de production : le fil tel
/// que le moteur le lit (`Request::thread`, consigne comprise) **plus** les schémas
/// d'outils, qui partent à chaque tour. Même heuristique que le crate (≈ 4 caractères par
/// jeton ASCII), volontairement réécrite : un test qui appelle la fonction qu'il vérifie
/// ne prouve rien de son câblage.
fn cout_requete(request: &Request) -> usize {
    let schemas = request
        .tools()
        .iter()
        .map(|outil| {
            let corps = format!(
                "{}{}{}",
                outil.name,
                outil.description,
                serde_json::to_string(&outil.parameters).unwrap_or_default()
            );
            let ascii = corps.bytes().filter(|b| b.is_ascii()).count();
            let autres = corps.chars().count().saturating_sub(ascii);
            ascii.div_ceil(4) + autres + 4
        })
        .fold(0usize, usize::saturating_add);
    estimate_tokens(&request.thread()).saturating_add(schemas)
}

/// Un moteur qui garde la requête reçue et conclut aussitôt (un seul tour).
struct Vu(RefCell<Vec<Request>>);

#[async_trait(?Send)]
impl Transport for Vu {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        self.0.borrow_mut().push(request.clone());
        Ok(Reply {
            served: None,
            text: "fini".into(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }
}

/// Une surface d'outils LARGE : c'est elle qui doit être comptée.
struct OutilsLarges(usize);

#[async_trait(?Send)]
impl Toolbox for OutilsLarges {
    fn specs(&self) -> Vec<ToolSpec> {
        (0..self.0)
            .map(|i| ToolSpec {
                name: format!("outil_{i}"),
                description: "d".repeat(200),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chemin": {"type": "string", "description": "p".repeat(150)},
                        "profondeur": {"type": "integer", "description": "p".repeat(50)}
                    }
                }),
            })
            .collect()
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("rien")
    }
}

/// Un contexte éphémère constant : il doit être compté UNE fois.
struct Memoire;

#[async_trait(?Send)]
impl TurnContext for Memoire {
    async fn before_turn(&self, _: &[Message]) -> Option<String> {
        Some(format!("mémoire du tour : {}", "m".repeat(300)))
    }
}

fn journal() -> (Rc<RefCell<Vec<Event>>>, impl FnMut(Event) + 'static) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let garde = log.clone();
    (log, move |event| garde.borrow_mut().push(event))
}

/// Un fil qui oblige `fit` à couper : 20 messages de 400 caractères ≈ 2 000 jetons.
fn fil_long() -> Vec<Message> {
    let mut fil = Vec::new();
    for i in 0..20 {
        fil.push(Message::user(format!("tour {i} : {}", "q".repeat(400))));
        fil.push(Message::assistant("ok"));
    }
    fil
}

fn harnais(
    transport: Rc<Vu>,
    outils: Rc<OutilsLarges>,
    contexte: Option<Rc<Memoire>>,
    budget: usize,
    sink: impl FnMut(Event) + 'static,
) -> Harness {
    let h = Harness::new(
        format!("consigne système : {}", "s".repeat(300)),
        Limits {
            max_tours: 1,
            max_relances: 0,
            budget_ms: None,
            context_budget: Some(budget),
            ..Limits::default()
        },
        transport,
        outils,
        sink,
    );
    match contexte {
        Some(c) => h.with_turn_context(c),
        None => h,
    }
}

#[test]
fn le_budget_compte_les_schemas_d_outils() {
    const BUDGET: usize = 3_000;
    let vus = Rc::new(Vu(RefCell::new(Vec::new())));
    let outils = Rc::new(OutilsLarges(10));
    let (_log, sink) = journal();
    let h = harnais(vus.clone(), outils.clone(), None, BUDGET, sink);
    h.seed(fil_long());
    let _ = futures::executor::block_on(h.send("dernière question"));

    let vus = vus.0.borrow();
    let requete = vus.last().expect("au moins un tour");
    let total = cout_requete(requete);
    // La mesure du crate est ANCORÉE sur la mesure indépendante du test : deux
    // heuristiques qui divergent feraient dire au harnais un chiffre que personne ne
    // peut reproduire.
    assert_eq!(
        estimate_request_tokens(requete),
        total,
        "la mesure publique du crate diverge de la mesure indépendante du test"
    );
    assert!(
        total <= BUDGET,
        "la requête envoyée fait {total} jetons pour un budget de {BUDGET} : \
         les schémas d'outils n'ont pas été comptés (fil seul : {}, outils : {})",
        estimate_tokens(&requete.thread()),
        total - estimate_tokens(&requete.thread())
    );
}

#[test]
fn un_depassement_annonce_est_le_depassement_mesure() {
    const BUDGET: usize = 900;
    let vus = Rc::new(Vu(RefCell::new(Vec::new())));
    let outils = Rc::new(OutilsLarges(6));
    let (log, sink) = journal();
    let h = harnais(
        vus.clone(),
        outils.clone(),
        Some(Rc::new(Memoire)),
        BUDGET,
        sink,
    );
    h.seed(fil_long());
    // Une question épinglée qui dépasse à elle seule le budget : le dépassement est
    // inévitable, et c'est justement le cas où le chiffre annoncé doit être vrai.
    let _ = futures::executor::block_on(h.send(&"Q".repeat(8_000)));

    let vus = vus.0.borrow();
    let requete = vus.last().expect("au moins un tour");
    let total = cout_requete(requete);
    assert!(
        total > BUDGET,
        "le cas testé doit dépasser : {total} jetons pour {BUDGET}"
    );
    let annonce = log
        .borrow()
        .iter()
        .find_map(|event| match event {
            Event::Trimmed {
                over_budget: true,
                excess,
                ..
            } => Some(*excess),
            _ => None,
        })
        .expect("un dépassement inévitable doit être annoncé");
    assert_eq!(
        annonce,
        total - BUDGET,
        "le dépassement annoncé ({annonce}) doit être celui que le moteur voit ({} jetons envoyés, budget {BUDGET})",
        total
    );
}

#[test]
fn le_contexte_du_tour_et_la_consigne_ne_sont_injectes_qu_une_fois() {
    const BUDGET: usize = 3_000;
    let vus = Rc::new(Vu(RefCell::new(Vec::new())));
    let outils = Rc::new(OutilsLarges(4));
    let (_log, sink) = journal();
    let h = harnais(
        vus.clone(),
        outils.clone(),
        Some(Rc::new(Memoire)),
        BUDGET,
        sink,
    );
    h.seed(fil_long());
    let _ = futures::executor::block_on(h.send("question"));

    let vus = vus.0.borrow();
    let requete = vus.last().expect("au moins un tour");
    let fil = requete.thread();
    let contextes: Vec<&Message> = fil
        .iter()
        .filter(|m| m.text().starts_with("mémoire du tour"))
        .collect();
    assert_eq!(
        contextes.len(),
        1,
        "le contexte du tour doit être injecté exactement une fois : {fil:?}"
    );
    let consignes = fil
        .iter()
        .filter(|m| m.text().starts_with("consigne système"))
        .count();
    assert_eq!(
        consignes, 1,
        "la consigne système doit être en tête, une seule fois"
    );
    assert_eq!(
        fil[0],
        Message::system(requete.system()),
        "la consigne système est le PREMIER message : {fil:?}"
    );
    // Et le contexte du tour ne vit pas dans le transcript : il serait alors
    // réinjecté au tour suivant EN PLUS du contexte recalculé.
    let transcript = h.transcript();
    assert!(
        !transcript
            .iter()
            .any(|m| m.text().starts_with("mémoire du tour")),
        "le contexte éphémère s'est écrit dans le fil : {transcript:?}"
    );
}
