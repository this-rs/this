//! **Contre-épreuve minimale du cycle de vie du routeur** — sans Obrain.
//!
//! Un `Learning` qui n'apprend rien, mais qui ENREGISTRE les identifiants reçus par
//! `record` et par `settle`. Pour chaque tour on conserve : `decision.id`, `applied`,
//! `chosen`, `used`, `routed.why`, et l'identifiant effectivement reçu par `settle`.
//!
//! But : identifier NOMMÉMENT les décisions jamais réglées et leur `RouteWhy` exact,
//! au lieu de déduire une cause d'une différence de compteurs.
use std::rc::Rc;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::executor::block_on;
use this_agent::Stop;
use this_harness::routing::{
    ArmStats, Candidate, Capabilities, CostBasis, Decision, Learning, MemoryPool, Pick, PriorHints,
    RouteWhy, RouterSpec, RoutingSettings, SequenceSeeds, TurnInput, TurnRouter,
};

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    block_on(f)
}

/// Témoin instrumenté : n'apprend rien, retient tout.
#[derive(Default)]
struct Enregistreur {
    recus_par_record: Mutex<Vec<u128>>,
    recus_par_settle: Mutex<Vec<u128>>,
}

#[async_trait(?Send)]
impl Learning for Enregistreur {
    async fn arms(&self, _class: &str) -> Vec<ArmStats> {
        Vec::new()
    }
    fn hints(&self) -> PriorHints {
        PriorHints::new()
    }
    async fn record(&self, d: &Decision) {
        self.recus_par_record.lock().unwrap().push(d.id);
    }
    async fn settle(&self, id: u128, _served: Option<&Pick>, _stop: &Stop, _code: Option<&str>) {
        self.recus_par_settle.lock().unwrap().push(id);
    }
}

fn cand(p: &str, m: &str) -> Candidate {
    Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..Candidate::new(
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

#[derive(Debug, Clone)]
struct Tour {
    id: u128,
    applied: bool,
    chosen: Option<(String, String)>,
    used: Option<(String, String)>,
    why: RouteWhy,
}

#[test]
fn identifie_nommement_les_decisions_jamais_reglees() {
    let temoin = Rc::new(Enregistreur::default());
    let spec = RouterSpec::new(
        Rc::new(MemoryPool::new(vec![cand("a", "m1"), cand("b", "m2")])),
        Rc::new(SequenceSeeds::from(3u128)),
        "a".to_string(),
        "m1".to_string(),
    )
    .learning(temoin.clone())
    .auto()
    .settings(RoutingSettings {
        exploration_epsilon: 0.1,
        ..Default::default()
    });
    let routeur = TurnRouter::new(spec);

    let mut tours: Vec<Tour> = Vec::new();
    for i in 0..40 {
        let routed = run(routeur.route(TurnInput {
            text: "hello",
            has_images: false,
            hints: Default::default(),
            honored: true,
        }));
        let Some(dec) = routed.decision.clone() else {
            println!("tour {i} : aucune décision ({:?})", routed.why);
            continue;
        };
        run(routeur.record(&dec));
        // Résultat synthétique : on exerce la clôture, sans prétendre mesurer quoi que
        // ce soit de fonctionnel (ce test ne mesure que le CYCLE DE VIE).
        run(routeur.settle(&Stop::Done, None, true));
        tours.push(Tour {
            id: dec.id,
            applied: dec.applied,
            chosen: dec
                .chosen
                .as_ref()
                .map(|p| (p.provider_id.clone(), p.model.clone())),
            used: dec
                .used
                .as_ref()
                .map(|p| (p.provider_id.clone(), p.model.clone())),
            why: routed.why,
        });
    }

    let records: std::collections::HashSet<u128> = temoin
        .recus_par_record
        .lock()
        .unwrap()
        .iter()
        .copied()
        .collect();
    let regles: std::collections::HashSet<u128> = temoin
        .recus_par_settle
        .lock()
        .unwrap()
        .iter()
        .copied()
        .collect();

    println!(
        "tours={} | record={} identifiants distincts | settle={} identifiants distincts",
        tours.len(),
        records.len(),
        regles.len()
    );

    // Ensembles : aucun doublon, tout réglé ⊆ enregistré, jamais-réglées calculées.
    let mut ids: Vec<u128> = tours.iter().map(|t| t.id).collect();
    ids.sort_unstable();
    let mut ids_uniques = ids.clone();
    ids_uniques.dedup();
    assert_eq!(ids, ids_uniques, "aucun identifiant de décision en double");
    assert_eq!(
        records.len(),
        tours.len(),
        "record doit avoir reçu chaque décision exactement une fois"
    );

    let mut jamais: Vec<&Tour> = tours.iter().filter(|t| !regles.contains(&t.id)).collect();
    println!("décisions JAMAIS réglées : {}", jamais.len());
    for t in &jamais {
        println!(
            "  id={} applied={} why={:?} chosen={:?} used={:?}",
            t.id, t.applied, t.why, t.chosen, t.used
        );
    }
    // Regroupement par (applied, why) : la cause doit être LISIBLE, pas devinée.
    let mut groupes: std::collections::BTreeMap<String, usize> = Default::default();
    for t in &jamais {
        *groupes
            .entry(format!("applied={} why={:?}", t.applied, t.why))
            .or_default() += 1;
    }
    for (k, n) in &groupes {
        println!("  groupe : {k} -> {n}");
    }

    // Tout identifiant réglé doit avoir été enregistré.
    assert!(
        regles.is_subset(&records),
        "settle a reçu un identifiant jamais enregistré"
    );
    // Les décisions jamais réglées doivent toutes être NON appliquées (hypothèse à
    // confirmer ou réfuter par ce test, pas à supposer).
    let appliquees_jamais = jamais.iter().filter(|t| t.applied).count();
    println!("parmi les jamais réglées : {appliquees_jamais} étaient APPLIQUÉES");
    // L'invariant qui compte : aucune décision APPLIQUÉE ne reste sans règlement.
    assert_eq!(
        appliquees_jamais, 0,
        "une décision appliquée doit avoir une issue (règlement ou réparation)"
    );
    // `used.is_some()` n'est PAS `applied` : les jamais-réglées portent bien `used`.
    assert!(
        jamais.iter().all(|t| t.used.is_some()),
        "les décisions non appliquées portent quand même une paire `used`"
    );
    jamais.clear();
}

/// `repair` : une décision laissée OUVERTE (deux décisions sans règlement de la
/// première) part en `abandoned`, puis `repair` la dit à l'apprentissage en
/// `Stop::Interrupted` SANS paire servie — donc sans récompense ni pénalité.
#[test]
fn repair_regle_une_decision_abandonnee_sans_recompense() {
    #[derive(Default)]
    struct T {
        settles: Mutex<Vec<(u128, bool, String)>>,
    } // (id, served_présent, stop)
    #[async_trait(?Send)]
    impl Learning for T {
        async fn arms(&self, _c: &str) -> Vec<ArmStats> {
            Vec::new()
        }
        fn hints(&self) -> PriorHints {
            PriorHints::new()
        }
        async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _c: Option<&str>) {
            self.settles
                .lock()
                .unwrap()
                .push((id, served.is_some(), format!("{stop:?}")));
        }
    }
    let t = Rc::new(T::default());
    let spec = RouterSpec::new(
        Rc::new(MemoryPool::new(vec![cand("a", "m1"), cand("b", "m2")])),
        Rc::new(SequenceSeeds::from(3u128)),
        "a".to_string(),
        "m1".to_string(),
    )
    .learning(t.clone())
    .auto()
    .settings(RoutingSettings {
        exploration_epsilon: 0.1,
        ..Default::default()
    });
    let routeur = TurnRouter::new(spec);

    // Deux décisions consécutives SANS régler la première => elle est abandonnée.
    let mut ouverte: Option<u128> = None;
    for _ in 0..2 {
        let routed = run(routeur.route(TurnInput {
            text: "hello",
            has_images: false,
            hints: Default::default(),
            honored: true,
        }));
        if let Some(d) = routed.decision.clone() {
            run(routeur.record(&d));
            if ouverte.is_none() {
                ouverte = Some(d.id);
            }
        }
    }
    println!("needs_repair = {}", routeur.needs_repair());
    assert!(
        routeur.needs_repair(),
        "une décision ouverte non réglée doit être abandonnée"
    );

    let reste = run(routeur.repair());
    let reglages = t.settles.lock().unwrap().clone();
    println!("réglages après repair : {reglages:?} ; reste = {reste}");
    assert_eq!(reglages.len(), 1, "repair règle UNE décision");
    let (id, servi, stop) = &reglages[0];
    assert_eq!(
        Some(*id),
        ouverte,
        "c'est bien la décision abandonnée qui est réglée"
    );
    assert!(!*servi, "repair ne fournit AUCUNE paire servie");
    assert!(
        stop.contains("Interrupted"),
        "repair dit Interrupted, pas une réussite"
    );
}
