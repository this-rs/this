//! La troisième borne, branchée dans la boucle : un fil qui grossit est ramené
//! sous le budget de contexte AVANT d'être envoyé, et le tour dit ce qu'il a
//! oublié. Sans ce lien, `context::fit` serait une jolie fonction que personne
//! n'appelle — et le transcript continuerait de grossir jusqu'au refus du moteur.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_harness::{
    Cancel, Delta, Event, Failure, Finish, Harness, Limits, Message, Outcome, Reply, Request,
    ToolCall, ToolSpec, Toolbox, Transport, estimate_tokens,
};

/// Un transport qui garde ce qu'on lui a envoyé, puis demande encore un outil :
/// le transcript grossit d'un groupe par tour.
struct Grossit(RefCell<Vec<Vec<Message>>>);

#[async_trait(?Send)]
impl Transport for Grossit {
    async fn stream(&self, request: &Request, _: &Cancel, _: Delta) -> Result<Reply, Failure> {
        let fil = request.thread();
        let id = format!("a{}", fil.len());
        self.0.borrow_mut().push(fil);
        Ok(Reply {
            text: String::new(),
            calls: vec![ToolCall {
                id,
                name: "gros".into(),
                arguments: json!({}),
            }],
            finish: Finish::Tools,
        })
    }
}

/// Un outil bavard : c'est lui qui fait grossir le fil.
struct GrosOutil;

#[async_trait(?Send)]
impl Toolbox for GrosOutil {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "gros".into(),
            description: "rend un long texte".into(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::ok("x".repeat(400))
    }
}

#[test]
fn la_borne_de_contexte_mord_et_le_dit() {
    let vus = Rc::new(Grossit(RefCell::new(Vec::new())));
    let log: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
    let vus_par_le_harnais = vus.clone();
    let journal = log.clone();
    let harness = Harness::new(
        "consigne",
        Limits {
            max_tours: 6,
            max_relances: 0,
            budget_ms: None,
            context_budget: Some(400),
            ..Limits::default()
        },
        vus_par_le_harnais.clone(),
        Rc::new(GrosOutil),
        move |event| journal.borrow_mut().push(event),
    );

    let _ = futures::executor::block_on(harness.send("premier"));

    // 1. Ce qui a été retiré a été DIT.
    let oublis: Vec<(usize, usize)> = log
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Trimmed {
                messages, tokens, ..
            } if *messages > 0 => Some((*messages, *tokens)),
            _ => None,
        })
        .collect();
    assert!(
        !oublis.is_empty(),
        "le fil a forcément dépassé 400 jetons : un Trimmed devait être émis"
    );

    // 2. Ce que le transport a reçu tient dans le budget.
    let vus = vus.0.borrow();
    let dernier = vus.last().expect("au moins un tour");
    assert!(
        estimate_tokens(dernier) <= 400,
        "la requête dépasse encore le budget : {} jetons",
        estimate_tokens(dernier)
    );

    // 3. Aucune réponse d'outil orpheline : le moteur refuserait la conversation
    //    (« tool_calls without tool messages »), et l'app paraîtrait cassée.
    for (index, message) in dernier.iter().enumerate() {
        if let Message::Tool { id, .. } = message {
            let appele = dernier[..index].iter().any(|precedent| {
                matches!(precedent, Message::Assistant { calls, .. }
                    if calls.iter().any(|call| &call.id == id))
            });
            assert!(
                appele,
                "réponse d'outil orpheline pour l'appel {id} : le moteur refuserait ce fil"
            );
        }
    }

    // 4. La question n'a pas été perdue : le dernier message utilisateur reste.
    assert!(
        dernier.iter().any(Message::is_user),
        "le fil gardé doit encore contenir la question"
    );
}

/// Un outil bavard qui ÉCHOUE : chaque échec fait pousser une relance au rôle
/// utilisateur par le harnais, ce qui est exactement le piège de l'épinglage.
struct GrosOutilEnEchec;

#[async_trait(?Send)]
impl Toolbox for GrosOutilEnEchec {
    fn specs(&self) -> Vec<ToolSpec> {
        GrosOutil.specs()
    }
    async fn call(&self, _: &ToolCall) -> Outcome {
        Outcome::failed("echec", "y".repeat(400))
    }
}

#[test]
fn la_question_d_origine_survit_a_la_borne_et_aux_relances() {
    let vus = Rc::new(Grossit(RefCell::new(Vec::new())));
    let harness = Harness::new(
        "consigne",
        Limits {
            max_tours: 8,
            max_relances: 8,
            budget_ms: None,
            context_budget: Some(400),
            ..Limits::default()
        },
        vus.clone(),
        Rc::new(GrosOutilEnEchec),
        |_| {},
    );
    let _ = futures::executor::block_on(harness.send("la vraie question"));
    let vus = vus.0.borrow();
    // Le test ne prouve rien si aucune relance n'est partie.
    assert!(
        vus.iter().any(|fil| fil.iter().any(Message::is_synthetic)),
        "aucune relance dans les fils envoyés : le test ne mord pas"
    );
    // Invariant : chaque fil envoyé au transport contient la question d'origine,
    // même quand le harnais a poussé des relances au rôle utilisateur.
    for fil in vus.iter() {
        assert!(
            fil.iter().any(
                |m| matches!(m, Message::User { content, synthetic: false, .. } if content == "la vraie question")
            ),
            "la question a été éjectée du fil envoyé : {fil:?}"
        );
    }
}
