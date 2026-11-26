//! Contre-épreuve d'attribution : une observation suit la CONNAISSANCE de la
//! décision (`Decision::class_key()`), jamais la dernière classe consultée par
//! `arms`. Deux décisions de connaissances distinctes, réglées dans l'ordre
//! inverse, avec des consultations intermédiaires — l'ancien canal latéral
//! (thread-local « dernière classe vue ») aurait tout attribué à la mauvaise.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

use async_trait::async_trait;
use this_agent::Stop;
use this_harness::cognition::{
    Classification, LearnedClass, SemanticEncoder, TaskRepr, Uncertainty,
};
use this_harness::routing::{
    ArmKey, ArmStats, Capabilities, CostBasis, Decision, Learning, MemoryPool, Pick, RouterSpec,
    RoutingSettings, SequenceSeeds, TurnInput, TurnRouter,
};

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    futures::executor::block_on(f)
}

fn cand(p: &str, m: &str) -> this_harness::routing::Candidate {
    this_harness::routing::Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..this_harness::routing::Candidate::new(
            p,
            m,
            Capabilities {
                tools: true,
                images: false,
                reasoning: false,
                context_window: Some(200_000),
            },
        )
    }
}

struct Enc;

impl SemanticEncoder for Enc {
    fn encode(&self, text: &str) -> Option<Vec<f32>> {
        match text {
            "A" => Some(vec![1.0]),
            "B" => Some(vec![-1.0]),
            _ => None,
        }
    }
}

/// Deux connaissances K1 (texte A) et K2 (texte B). `record` mémorise
/// `decision_id → class_key()`, `settle` écrit dans la classe de LA décision.
struct MemoireDeux {
    en_cours: Mutex<HashMap<u128, String>>,
    ecrites: Mutex<Vec<(String, String, bool)>>, // (classe, provider, favorable)
    bras_consultees: Mutex<Vec<String>>,
}

impl MemoireDeux {
    fn new() -> Self {
        Self {
            en_cours: Mutex::new(HashMap::new()),
            ecrites: Mutex::new(Vec::new()),
            bras_consultees: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait(?Send)]
impl Learning for MemoireDeux {
    async fn classify(&self, repr: &TaskRepr) -> Classification {
        let v = repr.embedding.as_ref().map(|e| e[0]).unwrap_or(0.0);
        let classe = if v >= 0.0 { "repr.K1" } else { "repr.K2" };
        Classification {
            class: Some(LearnedClass(classe.to_string())),
            formation: None,
            uncertainty: Uncertainty::Established(1.0),
            nearest: Some(LearnedClass(classe.to_string())),
            similarity: Some(1.0),
        }
    }
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        self.bras_consultees
            .lock()
            .unwrap()
            .push(classe.to_string());
        vec![
            ArmStats::fresh(ArmKey::new(classe, "a", "m1"), 1.0, 1.0),
            ArmStats::fresh(ArmKey::new(classe, "b", "m2"), 1.0, 1.0),
        ]
    }
    async fn record(&self, d: &Decision) {
        if d.used.is_some()
            && let Some(c) = d.class_key()
        {
            self.en_cours.lock().unwrap().insert(d.id, c);
        }
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let classe = self.en_cours.lock().unwrap().remove(&id);
        if let (Some(c), Some(p)) = (classe, served) {
            self.ecrites.lock().unwrap().push((
                c,
                p.provider_id.clone(),
                matches!(stop, Stop::Done),
            ));
        }
    }
}

fn routeur_pour(mem: Rc<MemoireDeux>) -> TurnRouter {
    let pool = Rc::new(MemoryPool::new(vec![cand("a", "m1"), cand("b", "m2")]));
    TurnRouter::new(
        RouterSpec::new(
            pool,
            Rc::new(SequenceSeeds::from(1)),
            "a".to_string(),
            "m1".to_string(),
        )
        .learning(mem.clone())
        .encoder(Rc::new(Enc))
        .auto()
        .settings(RoutingSettings {
            exploration_epsilon: 0.0,
            ..Default::default()
        }),
    )
}

fn tour(texte: &str) -> TurnInput<'_> {
    TurnInput {
        text: texte,
        has_images: false,
        hints: Default::default(),
        honored: true,
    }
}

#[test]
fn l_attribution_suit_la_decision_pas_la_derniere_classe_consultee() {
    let mem = Rc::new(MemoireDeux::new());
    let router = routeur_pour(mem.clone());

    // Deux décisions de connaissances distinctes, produites par le cycle natif.
    let d1 = run(router.route(tour("A"))).decision.clone().unwrap();
    let d2 = run(router.route(tour("B"))).decision.clone().unwrap();
    assert_eq!(d1.cognitive_class.as_deref(), Some("repr.K1"));
    assert_eq!(d2.cognitive_class.as_deref(), Some("repr.K2"));

    // Enregistrées D1 puis D2.
    run(mem.record(&d1));
    run(mem.record(&d2));

    // Consultations `arms` intermédiaires : la dernière classe consultée devient K1
    // (le piège de l'ancien thread-local CLASSE_COURANTE).
    run(mem.arms("repr.K2"));
    run(mem.arms("repr.K1"));

    // Réglées dans l'ordre INVERSE : D2 puis D1.
    run(mem.settle(d2.id, Some(&Pick::new("a", "m1")), &Stop::Done, None));
    run(mem.settle(d1.id, Some(&Pick::new("a", "m1")), &Stop::Done, None));

    let ecrites = mem.ecrites.lock().unwrap().clone();
    assert_eq!(
        ecrites.iter().filter(|(c, _, _)| c == "repr.K1").count(),
        1,
        "D1 doit être attribuée à K1 : {ecrites:?}"
    );
    assert_eq!(
        ecrites.iter().filter(|(c, _, _)| c == "repr.K2").count(),
        1,
        "D2 doit être attribuée à K2 : {ecrites:?}"
    );
    assert_eq!(ecrites.len(), 2, "{ecrites:?}");
}

#[test]
fn un_rejeu_du_meme_identifiant_n_ecrit_pas_deux_fois() {
    let mem = Rc::new(MemoireDeux::new());
    let router = routeur_pour(mem.clone());
    let d = run(router.route(tour("A"))).decision.clone().unwrap();
    run(mem.record(&d));
    run(mem.settle(d.id, Some(&Pick::new("a", "m1")), &Stop::Done, None));
    // Rejeu : `en_cours` ne contient plus l'identifiant → no-op.
    run(mem.settle(d.id, Some(&Pick::new("a", "m1")), &Stop::Done, None));
    let ecrites = mem.ecrites.lock().unwrap().clone();
    assert_eq!(
        ecrites.len(),
        1,
        "rejeu ne doit pas nourrir deux fois : {ecrites:?}"
    );
}
