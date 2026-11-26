//! **Lot E — témoin : « aucune connaissance ⇒ aucune attribution » dans TOUTES
//! les branches de `TurnRouter::route`.**
//!
//! Contrat relu dans `aa65d685` (#387), deux énoncés qui doivent concorder :
//!
//! * `cognition.rs`, `Classification::class_key()` : « `None` : connaissance
//!   inconnue **ou** proposition non enregistrée — jamais une classe commune
//!   d'apprentissage. Le consommateur (routeur, mémoire) doit propager `None`
//!   sans le replier sur `repr.unknown` pour l'attribution. »
//! * `decision.rs`, `Decision::class_key()` : « `None` = aucune connaissance
//!   constituée : `record` ne doit RIEN attribuer. »
//!
//! `TurnRouter::route` réécrit `cognitive_class` dans la branche « tour servi »
//! (`router.rs:1064`), mais **retourne la décision telle que `decide` l'a
//! construite** dans deux autres branches : consultatif (`honored = false`) et
//! dépassé (`RouteWhy::Superseded`). Or `decide_free` y a déjà posé
//! `Some(arms_key)` (`decide.rs:432`), et `arms_key` vaut `repr.unknown` quand
//! aucune connaissance n'est constituée.
//!
//! Ce fichier sépare les deux cas : le positif (branche servie) doit rester vert,
//! le témoin (branche consultative) découvre l'écart.

use std::rc::Rc;

use this_harness::cognition::CLASSE_INCONNUE;
use this_harness::routing::{
    Candidate, Capabilities, CostBasis, MemoryLearning, MemoryPool, RouterSpec, RoutingSettings,
    SequenceSeeds, TurnInput, TurnRouter,
};

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    futures::executor::block_on(f)
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

/// Un routeur sans encodeur et sans prototypes : `Learning::classify` n'est pas
/// implémenté par `MemoryLearning`, donc la réponse est `Classification::inconnue()`
/// — exactement l'état d'amorçage décrit par `cognition.rs`.
fn routeur(mem: Rc<MemoryLearning>) -> TurnRouter {
    let pool = Rc::new(MemoryPool::new(vec![cand("a", "m1"), cand("b", "m2")]));
    TurnRouter::new(
        RouterSpec::new(
            pool,
            Rc::new(SequenceSeeds::from(7)),
            "a".to_string(),
            "m1".to_string(),
        )
        .learning(mem)
        .auto()
        .settings(RoutingSettings {
            exploration_epsilon: 0.0,
            ..RoutingSettings::default()
        }),
    )
}

fn tour(honored: bool) -> TurnInput<'static> {
    TurnInput {
        text: "bonjour",
        has_images: false,
        hints: Default::default(),
        honored,
    }
}

/// **Contrôle positif** : la branche « tour servi » propage bien `None`.
#[test]
fn controle_positif_un_tour_servi_sans_connaissance_n_attribue_aucune_classe() {
    let mem = Rc::new(MemoryLearning::new());
    let routeur = routeur(mem);
    let routed = run(routeur.route(tour(true)));
    let d = routed.decision.expect("une décision est rendue");
    assert!(
        d.cognitive_class.is_none(),
        "branche servie : aucune connaissance ne doit être attribuée, obtenu {:?}",
        d.cognitive_class
    );
    assert!(d.class_key().is_none(), "class_key() doit être None");
}

/// **Témoin discriminant** : la branche consultative (`honored = false`) rend la
/// décision SANS réécrire `cognitive_class`. Elle doit pourtant, elle aussi,
/// n'attribuer aucune classe : aucune connaissance n'est constituée.
#[test]
fn temoin_un_tour_consultatif_sans_connaissance_n_attribue_aucune_classe() {
    let mem = Rc::new(MemoryLearning::new());
    let routeur = routeur(mem);
    let routed = run(routeur.route(tour(false)));
    let d = routed.decision.expect("une décision est rendue (conseil)");
    assert!(
        d.cognitive_class.is_none(),
        "branche consultative : aucune connaissance ne doit être attribuée, obtenu {:?} \
         (la clé neutre {CLASSE_INCONNUE:?} est une clé de LECTURE des bras, pas une identité)",
        d.cognitive_class
    );
}
