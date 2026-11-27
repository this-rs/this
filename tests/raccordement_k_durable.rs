#![cfg(feature = "harness-seeds")]
//! Test CONSOMMATEUR du raccordement : le routeur réel (`TurnRouter`) alloue ses
//! `decision_id` (K) via [`this::harness_seeds::SeedsDurables`], pas via une suite
//! en mémoire (`SequenceSeeds`). On vérifie, à travers le chemin que le harnais
//! emprunte réellement (`route` → `Seeds::next_id` → `Decision.id`) :
//!
//! 1. les `decision_id` sont les K durables de [`AutoriteK`](this::autorite_k::AutoriteK)
//!    (1, 2, 3…) et le compteur durable est persisté AVANT publication ;
//! 2. après réouverture, le compteur CONTINUE — aucun K n'est réattribué ;
//! 3. deux autorités vivantes sur le MÊME fichier ne collisionnent pas.

use std::rc::Rc;

use this::harness_seeds::SeedsDurables;
use this_harness::routing::{
    Candidate, Capabilities, CostBasis, MemoryPool, RouterSpec, Seeds, TurnInput, TurnRouter,
};

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    futures::executor::block_on(f)
}

fn candidat(p: &str, m: &str) -> Candidate {
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

fn routeur(seeds: Rc<dyn Seeds>) -> TurnRouter {
    TurnRouter::new(
        RouterSpec::new(
            Rc::new(MemoryPool::new(vec![
                candidat("a", "m1"),
                candidat("b", "m2"),
            ])),
            seeds,
            "a",
            "m1",
        )
        .auto(),
    )
}

/// Les `decision.id` des `n` premiers tours du routeur. En mode `auto` sur un pool
/// valide, chaque tour route et consomme exactement un K.
fn ids(routeur: &TurnRouter, n: usize) -> Vec<u128> {
    (0..n)
        .map(|_| {
            let routed = run(routeur.route(TurnInput {
                text: "hello",
                has_images: false,
                hints: Default::default(),
                honored: true,
            }));
            routed
                .decision
                .unwrap_or_else(|| panic!("tour non routé : {:?}", routed.why))
                .id
        })
        .collect()
}

#[test]
fn decision_id_durable_et_survit_a_la_reouverture() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("autorite.k");

    let seeds1 = Rc::new(SeedsDurables::ouvrir(chemin.clone()).unwrap());
    let routeur1 = routeur(seeds1.clone());

    let ids1 = ids(&routeur1, 3);
    assert_eq!(
        ids1,
        vec![1, 2, 3],
        "K durables depuis une autorité fraîche"
    );
    // Les K rendus sont persistés AVANT publication : l'état durable pointe déjà
    // sur le prochain (4), jamais sur le dernier rendu.
    assert_eq!(
        seeds1.prochain_durable().unwrap(),
        4,
        "compteur durable = prochain K, avant même la réouverture"
    );

    drop(routeur1);
    drop(seeds1);

    // Réouverture sur le MÊME fichier : compteur restauré, jamais régénéré.
    let seeds2 = SeedsDurables::ouvrir(chemin.clone()).unwrap();
    assert_eq!(seeds2.prochain_durable().unwrap(), 4, "compteur restauré");
    let routeur2 = routeur(Rc::new(seeds2));
    let ids2 = ids(&routeur2, 2);
    assert_eq!(ids2, vec![4, 5], "jamais réattribué après réouverture");
}

#[test]
fn deux_instances_sur_le_meme_fichier_ne_collisionnent_pas() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("autorite.k");

    // Deux autorités VIVANTES sur le même fichier : une seule autorité logique,
    // sérialisée par le verrou du fichier — pas une seconde autorité.
    let routeur_a = routeur(Rc::new(SeedsDurables::ouvrir(chemin.clone()).unwrap()));
    let routeur_b = routeur(Rc::new(SeedsDurables::ouvrir(chemin.clone()).unwrap()));

    let ids_a = ids(&routeur_a, 2);
    let ids_b = ids(&routeur_b, 2);

    assert_eq!(ids_a, vec![1, 2]);
    assert_eq!(
        ids_b,
        vec![3, 4],
        "la seconde instance voit l'émission de la première"
    );
    assert!(ids_a.iter().all(|k| !ids_b.contains(k)), "aucun K partagé");
}
