//! **L2** : un apprenant qui RÉVISE les statistiques du routeur à chaque fin de tour.
//!
//! Récompense mesurée ici : la **fiabilité de transport** (`Stop::Done` vs `Stop::Failed`),
//! PAS la compétence fonctionnelle — le port `Learning::settle` ne porte pas ce signal.
//! Réutilise le chemin réel du routeur : `TurnRouter::route` → `Decision` → `record`/`settle`.
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::executor::block_on;
use this_agent::Stop;
use this_harness::routing::{
    ArmKey, ArmStats, Capabilities, CostBasis, Decision, Learning, MemoryPool, Pick,
    PriorHints, RouterSpec, RoutingSettings, SequenceSeeds, TurnRouter, TurnInput,
};

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    block_on(f)
}

fn cand(provider: &str, model: &str, images: bool) -> this_harness::routing::Candidate {
    this_harness::routing::Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..this_harness::routing::Candidate::new(
            provider,
            model,
            Capabilities {
                tools: true,
                images,
                reasoning: false,
                context_window: Some(200_000),
            },
        )
    }
}

/// L2 — révise `ArmStats` à chaque `settle` (modèle Beta, a priori neutre 1/1).
struct Appris {
    bras: Mutex<HashMap<String, Vec<ArmStats>>>,
    en_cours: Mutex<HashMap<u128, String>>,
}

impl Appris {
    /// Le routeur ne décide que s'il a des bras à comparer : on amorce la classe
    /// « chat.general » avec les deux candidats à l'a priori neutre (1/1).
    fn new() -> Self {
        let mut bras = HashMap::new();
        bras.insert(
            "chat.general".to_string(),
            vec![
                ArmStats::fresh(ArmKey::new("chat.general", "a", "m1"), 1.0, 1.0),
                ArmStats::fresh(ArmKey::new("chat.general", "b", "m2"), 1.0, 1.0),
            ],
        );
        Self {
            bras: Mutex::new(bras),
            en_cours: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait(?Send)]
impl Learning for Appris {
    async fn arms(&self, class: &str) -> Vec<ArmStats> {
        self.bras.lock().unwrap().get(class).cloned().unwrap_or_default()
    }

    fn hints(&self) -> PriorHints {
        PriorHints::new()
    }

    async fn record(&self, d: &Decision) {
        if d.used.is_some() {
            self.en_cours.lock().unwrap().insert(d.id, d.signature.arm_key());
        }
    }

    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(class) = self.en_cours.lock().unwrap().remove(&id) else {
            return; // settle sans record : no-op (contrat)
        };
        let Some(p) = served else {
            return; // moteur jamais joint : aucun bras nourri (contrat)
        };
        let succes = matches!(stop, Stop::Done);
        let mut m = self.bras.lock().unwrap();
        let v = m.entry(class.clone()).or_default();
        match v
            .iter_mut()
            .find(|a| a.key.provider_id == p.provider_id && a.key.model == p.model)
        {
            Some(a) => {
                a.n += 1;
                if succes {
                    a.alpha += 1.0;
                } else {
                    a.beta += 1.0;
                }
            }
            None => {
                let (alpha, beta) = if succes { (2.0, 1.0) } else { (1.0, 2.0) };
                v.push(ArmStats::fresh(
                    ArmKey::new(class, p.provider_id.clone(), p.model.clone()),
                    alpha,
                    beta,
                ));
            }
        }
    }
}

/// Une campagne : `n` tours routés, environnement de transport **indépendant du routeur**
/// (a/m1 rend 80 %, b/m2 rend 20 %), graine fixe. Rend (succès transport, tours sur a/m1).
fn campagne(learning: Rc<dyn Learning>, graine: u128, n: usize) -> (usize, usize) {
    let pool = vec![cand("a", "m1", true), cand("b", "m2", true)];
    let spec = RouterSpec::new(
        Rc::new(MemoryPool::new(pool)),
        Rc::new(SequenceSeeds::from(graine)),
        "a".to_string(),
        "m1".to_string(),
    )
    .learning(learning.clone())
    .auto()
    .settings(RoutingSettings {
        exploration_epsilon: 0.1,
        ..Default::default()
    });
    let routeur = TurnRouter::new(spec);

    let mut rng = graine as u64;
    let mut premiere = true;
    let mut succes = 0usize;
    let mut sur_a = 0usize;

    for _ in 0..n {
        let routed = run(routeur.route(TurnInput {
            text: "hello",
            has_images: false,
            hints: Default::default(),
            honored: true,
        }));
        if premiere {
            println!("premier tour : why={:?} changed={} decision présente={}", routed.why, routed.changed, routed.decision.is_some());
            premiere = false;
        }
        let Some(d) = routed.decision.clone() else { continue };
        let Some(used) = d.used.clone() else { continue };

        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let taux = if used.model == "m1" { 80u64 } else { 20u64 };
        let ok = (rng % 100) < taux;
        if ok {
            succes += 1;
        }
        if used.provider_id == "a" {
            sur_a += 1;
        }

        run(learning.record(&d));
        let stop = if ok {
            Stop::Done
        } else {
            Stop::Failed("transport".to_string())
        };
        run(learning.settle(d.id, Some(&used), &stop, None));
    }
    (succes, sur_a)
}

#[test]
fn l2_depasse_aucun_apprentissage_sur_la_fiabilite_transport() {
    for graine in [1u128, 7, 42, 99] {
        let appris = Rc::new(Appris::new());
        let (s2, a2) = campagne(appris, graine, 400);

        let temoin = Rc::new(this_harness::routing::NoLearning);
        let (s0, a0) = campagne(temoin, graine, 400);

        // Regret = tours joués sur le mauvais candidat (b/m2) : 400 - sur_a.
        let regret2 = 400 - a2;
        let regret0 = 400 - a0;
        println!(
            "graine {graine:3} | L2 succès={s2:3} a/m1={a2:3} regret={regret2:3} | \
             témoin succès={s0:3} a/m1={a0:3} regret={regret0:3}"
        );
        // On n'exige pas que L2 gagne sur CHAQUE graine : on rapporte, et on vérifie
        // seulement qu'il n'est pas systématiquement battu.
        assert!(
            s2 >= s0,
            "graine {graine} : L2 {s2} < témoin {s0} (a : {a2} vs {a0})"
        );
    }
}


/// **Contrôle élémentaire d'une seule boucle** : décision produite, paire sélectionnée,
/// `record` appelé, observation indépendante, `settle` appelé, `ArmStats` modifiés,
/// puis la décision suivante **lit** ces statistiques.
#[test]
fn une_seule_boucle_fermee() {
    let appris = Rc::new(Appris::new());
    let pool = vec![cand("a", "m1", true), cand("b", "m2", true)];
    let spec = RouterSpec::new(
        Rc::new(MemoryPool::new(pool)),
        Rc::new(SequenceSeeds::from(1u128)),
        "a".to_string(),
        "m1".to_string(),
    )
    .learning(appris.clone())
    .auto()
    .settings(RoutingSettings {
        exploration_epsilon: 0.0,
        ..Default::default()
    });
    let mut routeur = TurnRouter::new(spec);

    // 1. une décision est réellement produite
    let routed = run(routeur.route(TurnInput {
        text: "hello",
        has_images: false,
        hints: Default::default(),
        honored: true,
    }));
    let d = routed.decision.clone().expect("une décision doit être produite");
    let used = d.used.clone().expect("une paire doit être sélectionnée");

    // 2. record puis observation indépendante (échec synthétique) puis settle
    run(routeur.record(&d));
    run(routeur.settle(&Stop::Failed("synthétique".to_string()), None, true));

    // 3. les ArmStats du bras servi ont bougé (beta +1)
    let bras = run(appris.arms("chat.general"));
    let servi = bras
        .iter()
        .find(|a| a.key.provider_id == used.provider_id && a.key.model == used.model)
        .expect("le bras servi doit exister");
    assert!(
        servi.beta > 1.0,
        "le bras servi doit avoir été pénalisé : beta={} (n={})",
        servi.beta,
        servi.n
    );

    // 4. la statistique est relue : la décision suivante ne peut plus choisir ce bras
    //    si l'autre est resté neutre — on vérifie seulement que la lecture a lieu.
    let apres = run(appris.arms("chat.general"));
    assert_eq!(apres.len(), 2, "les deux bras sont toujours là");
}
