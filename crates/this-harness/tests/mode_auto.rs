//! Le mode Auto, branché sur le vrai chemin d'un tour.
//!
//! Deux étages :
//! 1. le **routeur** d'une conversation (`routing::TurnRouter`) — les scénarios
//!    sont repris des tests du routeur par tour de Project Orchestrator
//!    (`src/chat/agent_hooks.rs`, modules `provider_move_tests` et
//!    `image_routing_tests`) ; chaque test cite son test PO d'origine. PO les
//!    écrit contre un décideur factice et une session liée à un fournisseur ; ici
//!    le vrai décideur tourne, rendu déterministe par des bras posés à la main ;
//! 2. le **harnais** : un tour en Auto obtient son modèle par le routeur, un tour
//!    hors Auto ne porte rien de plus qu'avant, et « auto » ne sort jamais.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use this_harness::routing::{
    AFTER_PROVIDER_MOVE, ArmKey, ArmStats, Candidate, Capabilities, ConversationRoutingMode,
    CostBasis, EmptyPool, LearningStage, MemoryLearning, MemoryPool, Menu, NARROWED_BY_PERSON,
    NO_CANDIDATE, PERSON_LIST_UNAVAILABLE, PROVIDER_IMPOSED, Pick, PoolEntry, ProviderRoutingMode,
    RejectReason, RouteWhy, Routed, RouterSpec, RoutingSettings, SAME_CLASS_NO_EXPLORE,
    SequenceSeeds, TurnInput, TurnRouter, is_auto_model,
};
use this_harness::{
    Cancel, Delta, Event, Failure, Finish, Harness, Limits, Outcome, Reply, Request, Running, Stop,
    ToolCall, ToolSpec, Toolbox, Transport,
};

fn run<F: std::future::Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

// ── Le monde d'un test ──────────────────────────────────────────────────────

/// Une paire saine, autorisée, à outils, sans prix (abonnement).
fn cand(provider: &str, model: &str, images: bool) -> Candidate {
    Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..Candidate::new(
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

fn entry(provider: &str, model: &str) -> PoolEntry {
    PoolEntry::new(provider, model)
}

/// Un bras si observé que son tirage ne laisse aucun doute : la paire gagne
/// (`wins`) ou perd, quelle que soit la graine.
fn arm(class: &str, provider: &str, model: &str, wins: bool) -> ArmStats {
    let (alpha, beta) = if wins { (2_000.0, 1.0) } else { (1.0, 2_000.0) };
    ArmStats {
        n: 2_000,
        ..ArmStats::fresh(ArmKey::new(class, provider, model), alpha, beta)
    }
}

/// La classe de « hello » et celle d'une question de débogage.
const GENERAL: &str = "chat.general";
const DEBUG: &str = "chat.debug";

struct World {
    pool: Rc<MemoryPool>,
    learning: Rc<MemoryLearning>,
}

impl World {
    fn new(pool: Vec<Candidate>) -> Self {
        Self {
            pool: Rc::new(MemoryPool::new(pool)),
            learning: Rc::new(MemoryLearning::new()),
        }
    }

    /// Une conversation sur `provider`/`model`, graine de départ `seed`.
    fn spec(&self, provider: &str, model: &str, seed: u128) -> RouterSpec {
        RouterSpec::new(
            self.pool.clone(),
            Rc::new(SequenceSeeds::from(seed)),
            provider,
            model,
        )
        .learning(self.learning.clone())
        // Pas d'exploration par défaut : un test qui en veut la demande.
        .settings(quiet())
    }

    /// `winner` gagne nettement, pour les classes « general » et « debug ».
    fn prefer(&self, winner: (&str, &str), losers: &[(&str, &str)]) {
        let mut arms = Vec::new();
        for class in [GENERAL, DEBUG] {
            arms.push(arm(class, winner.0, winner.1, true));
            for (provider, model) in losers {
                arms.push(arm(class, provider, model, false));
            }
        }
        self.learning.set_arms(arms);
    }
}

/// Les réglages par défaut, sans tirage d'exploration.
fn quiet() -> RoutingSettings {
    RoutingSettings {
        exploration_epsilon: 0.0,
        ..RoutingSettings::default()
    }
}

fn text(message: &str) -> TurnInput<'_> {
    TurnInput {
        text: message,
        has_images: false,
        hints: Default::default(),
    }
}

fn picture(message: &str) -> TurnInput<'_> {
    TurnInput {
        has_images: true,
        ..text(message)
    }
}

fn pair(routed: &Routed) -> (String, Option<String>) {
    (
        routed.route.provider().to_string(),
        routed.route.model().map(str::to_string),
    )
}

fn on(provider: &str, model: &str) -> (String, Option<String>) {
    (provider.to_string(), Some(model.to_string()))
}

fn reason(routed: &Routed) -> &str {
    routed.decision.as_ref().map_or("", |d| d.reason.as_str())
}

fn looked_at(routed: &Routed) -> Vec<Pick> {
    let mut picks: Vec<Pick> = routed
        .decision
        .as_ref()
        .map(|d| d.alternatives.iter().map(|a| a.pick.clone()).collect())
        .unwrap_or_default();
    picks.sort_by(|a, b| (&a.provider_id, &a.model).cmp(&(&b.provider_id, &b.model)));
    picks
}

fn three_providers() -> Vec<Candidate> {
    vec![
        cand("a", "m1", false),
        cand("b", "m2", false),
        cand("c", "m3", false),
    ]
}

// ── 1. Le routeur : la liste de la personne ─────────────────────────────────

/// PO `agent_hooks.rs::provider_move_tests::auto_never_chooses_outside_the_persons_list`
/// et `the_model_of_a_turn_stays_within_the_persons_list`.
#[test]
fn auto_ne_choisit_jamais_hors_de_la_liste_de_la_personne() {
    for seed in 0..60u128 {
        let world = World::new(three_providers());
        let router = TurnRouter::new(
            world
                .spec("a", "m1", seed)
                .auto()
                .auto_models(vec![entry("b", "m2")]),
        );
        let routed = run(router.route(text("hello")));
        assert_eq!(pair(&routed), on("b", "m2"), "graine {seed}");
        assert!(
            reason(&routed).ends_with(&format!("; narrowed: {NARROWED_BY_PERSON}")),
            "{}",
            reason(&routed)
        );
        assert_eq!(looked_at(&routed), vec![Pick::new("b", "m2")]);
    }
}

/// PO `agent_hooks.rs::provider_move_tests::auto_without_a_list_sees_every_available_model`.
#[test]
fn auto_sans_liste_voit_tous_les_modeles_disponibles() {
    let world = World::new(three_providers());
    let router = TurnRouter::new(world.spec("a", "m1", 1).auto());
    let routed = run(router.route(text("hello")));
    assert_eq!(looked_at(&routed).len(), 3);
    assert!(!reason(&routed).contains("narrowed"), "{}", reason(&routed));
}

/// PO `agent_hooks.rs::provider_move_tests::a_list_with_nothing_usable_falls_back_to_every_model_and_says_so`
/// et `auto_models.rs::nothing_usable_on_the_list_falls_back_to_every_model_and_says_so`
/// (mêmes entrées : supprimé, consentement retiré, en panne).
#[test]
fn une_liste_sans_rien_d_utilisable_retombe_sur_tous_les_modeles_et_le_dit() {
    let mut pool = three_providers();
    pool[1].allowed = false;
    pool[2].healthy = Some(false);
    let world = World::new(pool);
    let router = TurnRouter::new(world.spec("a", "m1", 3).auto().auto_models(vec![
        entry("gone", "x"),
        entry("b", "m2"),
        entry("c", "m3"),
    ]));
    let routed = run(router.route(text("hello")));
    assert_eq!(pair(&routed), on("a", "m1"));
    assert_eq!(
        looked_at(&routed).len(),
        3,
        "tous redeviennent des candidats"
    );
    assert!(
        reason(&routed).ends_with(&format!("; narrowed: {PERSON_LIST_UNAVAILABLE}")),
        "{}",
        reason(&routed)
    );
}

/// PO `auto_models.rs::a_list_narrows_the_pool_to_its_models` et
/// `a_model_that_disappeared_leaves_the_pool_silently` (mêmes entrées).
#[test]
fn une_liste_restreint_le_pool_et_un_modele_disparu_en_sort_sans_bruit() {
    use this_harness::routing::{ContextHints, TurnSignature, restrict_to_person_list};
    let signature = TurnSignature::from_chat("hello", false, ContextHints::default());
    let mut pool = vec![
        cand("a", "m1", false),
        cand("b", "m2", false),
        cand("b", "m3", false),
    ];
    let code = restrict_to_person_list(&mut pool, &[entry("b", "m2")], &signature);
    assert_eq!(code, NARROWED_BY_PERSON);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].model, "m2");

    let mut pool = vec![cand("a", "m1", false), cand("b", "m2", false)];
    let code = restrict_to_person_list(
        &mut pool,
        &[entry("gone", "x"), entry("a", "m1")],
        &signature,
    );
    assert_eq!(code, NARROWED_BY_PERSON);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].provider_id, "a");
}

/// PO `agent_hooks.rs::provider_move_tests::a_conversations_pool_wins_over_the_persons_list`.
#[test]
fn le_pool_de_la_conversation_gagne_sur_la_liste_de_la_personne() {
    let world = World::new(vec![
        cand("a", "m1", false),
        cand("a", "m2", false),
        cand("b", "m9", false),
    ]);
    let router = TurnRouter::new(
        world
            .spec("a", "m1", 5)
            .ticked(vec![entry("a", "m1"), entry("a", "m2")])
            .auto_models(vec![entry("b", "m9")]),
    );
    assert_eq!(router.mode(), Some(ConversationRoutingMode::Mixed));
    let routed = run(router.route(text("hello")));
    assert_eq!(
        looked_at(&routed),
        vec![Pick::new("a", "m1"), Pick::new("a", "m2")]
    );
    assert!(
        !reason(&routed).contains("auto_models"),
        "{}",
        reason(&routed)
    );
}

// ── 2. Le routeur : les trois modes et l'étape ──────────────────────────────

/// PO `agent_hooks.rs::provider_move_tests::the_conversations_auto_is_decided_at_the_auto_stage_whatever_the_settings`.
#[test]
fn l_auto_de_la_conversation_se_decide_a_l_etape_auto_quels_que_soient_les_reglages() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    // Réglages par défaut : `primary` + `shadow`.
    let router = TurnRouter::new(world.spec("a", "m1", 1).auto());
    let routed = run(router.route(text("hello")));
    let decision = routed.decision.as_ref().unwrap();
    assert_eq!(decision.mode, ProviderRoutingMode::Full);
    assert_eq!(decision.stage, LearningStage::Auto);
    assert!(decision.applied);
    assert_eq!(pair(&routed), on("b", "m2"));
    assert_eq!(routed.why, RouteWhy::Routed);
    assert!(routed.changed);
}

/// PO `agent_hooks.rs::provider_move_tests::a_pool_chosen_on_the_conversation_is_decided_at_the_auto_stage`.
#[test]
fn un_pool_choisi_sur_la_conversation_se_decide_a_l_etape_auto() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = TurnRouter::new(
        world
            .spec("a", "m1", 1)
            .ticked(vec![entry("a", "m1"), entry("b", "m2")]),
    );
    let routed = run(router.route(text("hello")));
    let decision = routed.decision.as_ref().unwrap();
    // Un pool est routé comme `full` à l'intérieur.
    assert_eq!(decision.mode, ProviderRoutingMode::Full);
    assert_eq!(decision.stage, LearningStage::Auto);
    assert!(decision.applied);
    assert_eq!(pair(&routed), on("b", "m2"));
    // `c/m3` n'est pas coché : jamais regardé.
    assert_eq!(
        looked_at(&routed),
        vec![Pick::new("a", "m1"), Pick::new("b", "m2")]
    );
}

/// PO `agent_hooks.rs::provider_move_tests::the_shadow_stage_records_the_provider_decision_and_moves_nothing`.
#[test]
fn l_etape_shadow_note_la_decision_et_ne_change_rien() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    // La conversation n'a rien demandé : elle suit les réglages (`full` + `shadow`).
    let settings = RoutingSettings {
        mode: ProviderRoutingMode::Full,
        stage: LearningStage::Shadow,
        ..quiet()
    };
    let router = TurnRouter::new(world.spec("a", "m1", 1).settings(settings));
    let routed = run(router.route(text("hello")));
    assert_eq!(pair(&routed), on("a", "m1"));
    assert_eq!(routed.why, RouteWhy::RecordedOnly);
    assert!(!routed.changed);
    let recorded = world.learning.decisions();
    assert_eq!(recorded.len(), 1, "la décision est notée");
    assert_eq!(recorded[0].chosen, Some(Pick::new("b", "m2")));
    assert!(!recorded[0].applied);
}

/// Une conversation qui n'a rien demandé, sous les réglages par défaut
/// (`primary`) : le routeur ne décide rien. PO `routing_modes_e2e_tests.rs`
/// (monde `primary`) et `decider.rs::applied_follows_the_mode_stage_and_role_table`.
#[test]
fn sans_demande_et_en_primary_rien_n_est_decide() {
    let world = World::new(three_providers());
    let router = TurnRouter::new(world.spec("a", "m1", 1));
    assert!(!router.routes());
    let routed = run(router.route(text("hello")));
    assert_eq!(pair(&routed), on("a", "m1"));
    assert_eq!(routed.why, RouteWhy::NotRouted);
    assert!(routed.decision.is_none());
    assert!(world.learning.decisions().is_empty());
}

/// PO `agent_hooks.rs::provider_move_tests::an_imposed_model_or_provider_is_never_moved_until_auto_is_chosen_again`.
#[test]
fn un_modele_ou_un_fournisseur_impose_ne_bouge_pas_jusqu_a_ce_qu_auto_soit_rechoisi() {
    // Modèle imposé : rien n'est demandé.
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let mut spec = world.spec("a", "m1", 1).auto();
    spec.explicit_model = true;
    let router = TurnRouter::new(spec);
    let routed = run(router.route(text("hello")));
    assert_eq!(pair(&routed), on("a", "m1"));
    assert_eq!(routed.why, RouteWhy::Pinned);
    assert!(world.learning.decisions().is_empty());
    // Auto rechoisi : le routeur reprend la main, dès le tour suivant.
    assert_eq!(
        router.reroute(Menu::Auto),
        Ok(ConversationRoutingMode::Full)
    );
    assert_eq!(pair(&run(router.route(text("hello")))), on("b", "m2"));

    // Fournisseur imposé : le modèle se route, le fournisseur ne bouge pas.
    let world = World::new(vec![
        cand("a", "m1", false),
        cand("a", "m2", false),
        cand("b", "m9", false),
    ]);
    world.prefer(("b", "m9"), &[("a", "m1"), ("a", "m2")]);
    let mut spec = world.spec("a", "m1", 1).auto();
    spec.provider_imposed = true;
    let router = TurnRouter::new(spec);
    let routed = run(router.route(text("hello")));
    assert_eq!(routed.route.provider(), "a");
    assert!(
        reason(&routed).ends_with(&format!("; narrowed: {PROVIDER_IMPOSED}")),
        "{}",
        reason(&routed)
    );
    assert_eq!(router.reroute(Menu::Auto).unwrap().as_str(), "full");
    assert_eq!(pair(&run(router.route(text("hello")))), on("b", "m9"));
}

/// Le changement de mode en pleine conversation : auto → strict → mixed → auto,
/// puis à la main. PO `agent_hooks.rs::TurnRouter::reroute` et
/// `provider_move_tests::re_ticking_a_pool_across_providers_opens_a_strict_conversation_to_a_move`.
#[test]
fn le_mode_change_en_pleine_conversation() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = TurnRouter::new(world.spec("a", "m1", 1).auto());
    assert_eq!(pair(&run(router.route(text("hello")))), on("b", "m2"));

    // Strict : un modèle coché, imposé, jamais remplacé — même s'il perd.
    assert_eq!(
        router.reroute(Menu::Ticked(vec![entry("c", "m3")])),
        Ok(ConversationRoutingMode::Primary)
    );
    let before = world.learning.decisions().len();
    for _ in 0..5 {
        let routed = run(router.route(text("hello")));
        assert_eq!(pair(&routed), on("c", "m3"));
        assert_eq!(routed.why, RouteWhy::Pinned);
    }
    assert_eq!(
        world.learning.decisions().len(),
        before,
        "rien n'est décidé"
    );

    // Mixed : deux modèles cochés, le routeur choisit parmi eux seulement.
    assert_eq!(
        router.reroute(Menu::Ticked(vec![entry("a", "m1"), entry("c", "m3")])),
        Ok(ConversationRoutingMode::Mixed)
    );
    let routed = run(router.route(text("hello")));
    assert_eq!(
        looked_at(&routed),
        vec![Pick::new("a", "m1"), Pick::new("c", "m3")]
    );
    assert_ne!(routed.route.provider(), "b", "b n'est pas coché");

    // Auto de nouveau : tout redevient candidat.
    router.reroute(Menu::Auto).unwrap();
    assert_eq!(looked_at(&run(router.route(text("hello")))).len(), 3);

    // À la main : le routeur laisse la conversation, jusqu'au prochain menu.
    router.set_by_hand("a", "m1");
    let routed = run(router.route(text("hello")));
    assert_eq!(pair(&routed), on("a", "m1"));
    assert_eq!(routed.why, RouteWhy::Manual);
    router.reroute(Menu::Auto).unwrap();
    assert!(router.routes());

    // Un menu qui ne coche rien ne dit rien.
    assert_eq!(router.reroute(Menu::Ticked(vec![])), Err(EmptyPool));
    assert_eq!(
        router.mode(),
        Some(ConversationRoutingMode::Full),
        "inchangé"
    );

    // La conversation rend la main aux réglages (`primary` par défaut).
    router.follow_settings();
    assert_eq!(router.mode(), None);
    assert!(!router.routes());
}

// ── 3. Le routeur : un seul candidat, aucun candidat ────────────────────────

/// Contrainte produit de PO : un seul moteur est un cas de PREMIER ORDRE. Un
/// seul candidat éligible → on part dessus, à chaque tour, sans hésiter.
/// PO `mode.rs::take_over_subagent_tool` (« Mono is a first-order case, not a
/// degraded multi ») et `decider.rs::the_candidates_can_be_restricted_to_one_instance`.
#[test]
fn un_seul_candidat_on_part_dessus() {
    for seed in 0..40u128 {
        // Le seul moteur est celui de la session.
        let world = World::new(vec![cand("local", "llama", false)]);
        let router = TurnRouter::new(world.spec("local", "llama", seed).auto());
        for message in ["hello", "pourquoi ça plante ?", "implémente ça"] {
            let routed = run(router.route(text(message)));
            assert_eq!(pair(&routed), on("local", "llama"));
            assert_eq!(routed.why, RouteWhy::SingleCandidate);
            assert!(!routed.changed);
        }

        // La session est ouverte sur la sentinelle : le seul moteur la remplace.
        let world = World::new(vec![cand("local", "llama", false)]);
        let router = TurnRouter::new(world.spec("local", "auto", seed).auto());
        let routed = run(router.route(text("hello")));
        assert_eq!(pair(&routed), on("local", "llama"));
        assert_eq!(routed.why, RouteWhy::SingleCandidate);
        assert!(routed.changed);

        // Trois moteurs, un seul éligible (les autres : en panne, sans consentement).
        let mut pool = three_providers();
        pool[0].healthy = Some(false);
        pool[2].allowed = false;
        let world = World::new(pool);
        let router = TurnRouter::new(world.spec("a", "m1", seed).auto());
        let routed = run(router.route(text("hello")));
        assert_eq!(pair(&routed), on("b", "m2"));
        assert_eq!(routed.why, RouteWhy::SingleCandidate);
    }
}

/// Zéro candidat → repli sur le modèle de la session ; jamais « aucun candidat »
/// comme une erreur. PO `decider.rs::nothing_eligible_is_no_candidate_with_the_rejections_listed`
/// (la décision) et `agent_hooks.rs::directive_for_turn` (le repli : `TurnDirective::none()`).
#[test]
fn aucun_candidat_repli_sur_le_modele_de_la_session() {
    // L'hôte ne connaît aucun modèle.
    let world = World::new(vec![]);
    let router = TurnRouter::new(world.spec("a", "m1", 1).auto());
    let routed = run(router.route(text("hello")));
    assert_eq!(pair(&routed), on("a", "m1"));
    assert_eq!(routed.why, RouteWhy::NoCandidate);
    assert!(!routed.changed);

    // Tous rejetés : la décision le dit, avec chaque motif, et la session tient.
    let mut pool = three_providers();
    pool[0].capabilities.tools = false;
    pool[1].allowed = false;
    pool[2].healthy = Some(false);
    let world = World::new(pool);
    let router = TurnRouter::new(world.spec("a", "m1", 1).auto());
    let routed = run(router.route(text("hello")));
    assert_eq!(pair(&routed), on("a", "m1"));
    assert_eq!(routed.why, RouteWhy::NoCandidate);
    let decision = routed.decision.as_ref().unwrap();
    assert_eq!(decision.reason, NO_CANDIDATE);
    let rejected: Vec<_> = decision
        .alternatives
        .iter()
        .map(|a| a.rejected.clone().unwrap())
        .collect();
    assert_eq!(
        rejected,
        vec![
            RejectReason::NoTools,
            RejectReason::NotAllowed,
            RejectReason::Unhealthy
        ]
    );

    // Le fournisseur revient : le tour suivant est routé, sans rien faire.
    world.pool.set(three_providers());
    assert_ne!(
        run(router.route(text("hello"))).why,
        RouteWhy::NoCandidate,
        "le repli n'est pas un état"
    );
}

// ── 4. Le routeur : les images ──────────────────────────────────────────────

fn vision_world() -> World {
    let world = World::new(vec![cand("a", "blind", false), cand("a", "seer", true)]);
    // Le modèle sans vision est le préféré : seule la contrainte dure l'écarte.
    world.prefer(("a", "blind"), &[("a", "seer")]);
    world
}

/// PO `agent_hooks.rs::image_routing_tests::an_image_turn_is_routed_to_a_vision_candidate_and_the_reason_says_so`
/// et `a_text_turn_keeps_the_model_in_force`.
#[test]
fn un_tour_a_image_part_sur_un_modele_qui_la_lit_et_le_motif_le_dit() {
    for seed in 0..40u128 {
        let world = vision_world();
        let router = TurnRouter::new(world.spec("a", "blind", seed).auto());
        // Un tour de texte garde le modèle en vigueur.
        let routed = run(router.route(text("hello")));
        assert_eq!(pair(&routed), on("a", "blind"));
        assert_eq!(routed.why, RouteWhy::Kept);
        // Le tour à image le quitte.
        let routed = run(router.route(picture("hello")));
        assert_eq!(pair(&routed), on("a", "seer"), "graine {seed}");
        assert!(
            reason(&routed).contains("reads the attached images (models without vision excluded)"),
            "{}",
            reason(&routed)
        );
        let decision = routed.decision.as_ref().unwrap();
        assert!(
            decision
                .alternatives
                .iter()
                .any(|a| a.rejected == Some(RejectReason::NoImages))
        );
    }
}

/// PO `agent_hooks.rs::image_routing_tests::the_hook_routes_an_image_turn_even_right_after_a_change`.
#[test]
fn un_tour_a_image_est_route_meme_juste_apres_un_changement() {
    let world = World::new(vec![
        cand("a", "blind", false),
        cand("a", "other", false),
        cand("a", "seer", true),
    ]);
    world.prefer(("a", "blind"), &[("a", "other"), ("a", "seer")]);
    let router = TurnRouter::new(world.spec("a", "other", 1).auto());
    let first = run(router.route(text("hello")));
    assert_eq!(pair(&first), on("a", "blind"));
    assert!(first.changed);
    // Le tour suivant, de la même classe, porte une image.
    let second = run(router.route(picture("hello")));
    assert_eq!(pair(&second), on("a", "seer"));
    assert!(reason(&second).contains(SAME_CLASS_NO_EXPLORE));
}

/// PO `agent_hooks.rs::image_routing_tests::a_pool_without_any_vision_model_still_says_images_will_not_pass` :
/// un pool sans vision n'est jamais caché — la session tient, et le moteur refusera honnêtement.
#[test]
fn un_pool_sans_vision_laisse_la_session_et_le_dit() {
    let world = World::new(vec![cand("a", "blind", false), cand("b", "blind2", false)]);
    let router = TurnRouter::new(world.spec("a", "blind", 1).auto());
    let routed = run(router.route(picture("regarde")));
    assert_eq!(pair(&routed), on("a", "blind"));
    assert_eq!(routed.why, RouteWhy::NoCandidate);
    let decision = routed.decision.as_ref().unwrap();
    assert!(
        decision
            .alternatives
            .iter()
            .all(|a| a.rejected == Some(RejectReason::NoImages))
    );
}

// ── 5. Le routeur : les gardes anti-oscillation ─────────────────────────────

/// PO `agent_hooks.rs::provider_move_tests::right_after_a_change_a_turn_of_the_same_class_is_decided_without_exploration`
/// et `right_after_a_change_a_turn_of_another_class_is_routed_on_the_turn`.
#[test]
fn juste_apres_un_changement_un_tour_de_meme_classe_est_stable_et_un_autre_non() {
    let build = || {
        let world = World::new(vec![cand("a", "m1", false), cand("a", "m2", false)]);
        world.prefer(("a", "m2"), &[("a", "m1")]);
        let settings = RoutingSettings {
            exploration_epsilon: 0.25,
            ..RoutingSettings::default()
        };
        let router = TurnRouter::new(world.spec("a", "m1", 7).settings(settings).auto());
        let first = run(router.route(text("hello")));
        assert!(first.changed, "le tour 0 change de modèle");
        router
    };

    // Même classe au tour suivant : pas d'exploration, et le motif le dit.
    let same = run(build().route(text("hello again")));
    let decision = same.decision.as_ref().unwrap();
    assert_eq!(decision.exploration_epsilon, 0.0);
    assert!(!decision.explored);
    assert!(
        decision
            .reason
            .ends_with(&format!("; narrowed: {SAME_CLASS_NO_EXPLORE}"))
    );

    // Autre classe : routé comme n'importe quel tour.
    let other = run(build().route(text("pourquoi ça plante ?")));
    let decision = other.decision.as_ref().unwrap();
    assert_eq!(decision.signature.arm_key(), DEBUG);
    assert_eq!(decision.exploration_epsilon, 0.25);
    assert!(!decision.reason.contains("narrowed"), "{}", decision.reason);
}

/// PO `agent_hooks.rs::provider_move_tests::the_turn_after_a_move_may_change_model_but_not_provider_then_the_router_may_move_again`
/// et `the_turn_after_a_move_changes_its_model_on_the_provider_it_moved_to`.
#[test]
fn le_tour_apres_un_changement_de_fournisseur_peut_changer_de_modele_pas_de_fournisseur() {
    let world = World::new(vec![
        cand("a", "m1", false),
        cand("b", "m2", false),
        cand("b", "m3", false),
    ]);
    world.prefer(("b", "m2"), &[("a", "m1"), ("b", "m3")]);
    let router = TurnRouter::new(world.spec("a", "m1", 1).auto());
    // Tour 0 : la conversation passe de `a` à `b`.
    let moved = run(router.route(text("hello")));
    assert_eq!(pair(&moved), on("b", "m2"));

    // Tour 1 : `a` devient le préféré, mais on vient de bouger.
    world.prefer(("a", "m1"), &[("b", "m2"), ("b", "m3")]);
    let after = run(router.route(text("pourquoi ça plante ?")));
    assert_eq!(
        after.route.provider(),
        "b",
        "pas deux changements de fournisseur de suite"
    );
    assert!(
        reason(&after).ends_with(&format!("; narrowed: {AFTER_PROVIDER_MOVE}")),
        "{}",
        reason(&after)
    );
    assert_eq!(
        looked_at(&after),
        vec![Pick::new("b", "m2"), Pick::new("b", "m3")],
        "seuls les modèles du fournisseur d'arrivée sont regardés"
    );

    // Tour 2 : le routeur peut bouger de nouveau.
    let again = run(router.route(text("hello")));
    assert_eq!(pair(&again), on("a", "m1"));
}

/// PO `agent_hooks.rs::provider_move_tests::a_gap_under_the_hysteresis_margin_or_an_exploration_draw_never_moves`.
#[test]
fn un_tirage_d_exploration_ne_change_jamais_de_fournisseur() {
    let mut refused = 0;
    for seed in 0..600u128 {
        let world = World::new(vec![cand("a", "m1", false), cand("b", "m2", false)]);
        // `a` gagne nettement : seul un tirage d'exploration peut nommer `b`.
        world.prefer(("a", "m1"), &[("b", "m2")]);
        let settings = RoutingSettings {
            exploration_epsilon: 0.25,
            ..RoutingSettings::default()
        };
        let router = TurnRouter::new(world.spec("a", "m1", seed).settings(settings).auto());
        let routed = run(router.route(text("hello")));
        assert_eq!(pair(&routed), on("a", "m1"), "graine {seed}");
        let decision = routed.decision.as_ref().unwrap();
        if decision.explored && decision.chosen == Some(Pick::new("b", "m2")) {
            assert_eq!(routed.why, RouteWhy::ExplorationNeverMoves);
            refused += 1;
        }
    }
    assert!(
        refused > 20,
        "l'exploration tombe (~12 % ici) et ne déplace rien : {refused}"
    );
}

// ── 6. Le harnais : le vrai chemin d'un tour ────────────────────────────────

/// Un moteur qui récite un script et garde chaque requête reçue.
struct Script {
    replies: RefCell<VecDeque<Reply>>,
    seen: RefCell<Vec<Request>>,
}

fn script(replies: Vec<Reply>) -> Rc<Script> {
    Rc::new(Script {
        replies: RefCell::new(replies.into()),
        seen: RefCell::new(Vec::new()),
    })
}

#[async_trait(?Send)]
impl Transport for Script {
    async fn stream(
        &self,
        request: &Request,
        _cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        self.seen.borrow_mut().push(request.clone());
        let reply = self.replies.borrow_mut().pop_front().unwrap_or(Reply {
            text: "fini".to_string(),
            calls: vec![],
            finish: Finish::Stop,
        });
        delta(&reply.text);
        Ok(reply)
    }
}

struct Tools;

#[async_trait(?Send)]
impl Toolbox for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lire".to_string(),
            description: "lit".to_string(),
            parameters: json!({"type": "object"}),
        }]
    }
    async fn call(&self, _call: &ToolCall) -> Outcome {
        Outcome::ok("lu")
    }
}

type Journal = Rc<RefCell<Vec<Event>>>;

fn harnais(transport: Rc<Script>, router: Option<Rc<TurnRouter>>) -> (Harness, Journal) {
    let journal: Journal = Rc::new(RefCell::new(Vec::new()));
    let sink = journal.clone();
    let harness = Harness::new(
        "tu es l'atelier",
        Limits::default(),
        transport,
        Rc::new(Tools),
        move |event| sink.borrow_mut().push(event),
    );
    let harness = match router {
        Some(router) => harness.with_router(router),
        None => harness,
    };
    (harness, journal)
}

fn asks_a_tool() -> Reply {
    Reply {
        text: String::new(),
        calls: vec![ToolCall {
            id: "a1".to_string(),
            name: "lire".to_string(),
            arguments: json!({}),
        }],
        finish: Finish::Tools,
    }
}

fn names(journal: &Journal) -> Vec<&'static str> {
    journal.borrow().iter().map(Event::name).collect()
}

fn routed_events(journal: &Journal) -> Vec<Event> {
    journal
        .borrow()
        .iter()
        .filter(|e| matches!(e, Event::Routed { .. }))
        .cloned()
        .collect()
}

/// Hors Auto, un tour se comporte exactement comme avant : la requête ne porte
/// aucune paire, et le flux d'événements ne contient rien de plus. (La preuve
/// large est ailleurs : toute la suite existante du crate passe inchangée.)
#[test]
fn sans_routeur_la_requete_ne_porte_rien_et_aucun_evenement_de_plus_ne_sort() {
    let play = |router: Option<Rc<TurnRouter>>| {
        let transport = script(vec![asks_a_tool()]);
        let (harness, journal) = harnais(transport.clone(), router);
        assert_eq!(
            run(harness.send("ajoute une alerte")),
            Running::Ran(Stop::Done)
        );
        (transport, journal, harness.transcript())
    };
    let (bare, bare_journal, bare_transcript) = play(None);
    assert!(
        bare.seen
            .borrow()
            .iter()
            .all(|r| r.model().is_none() && r.provider().is_none())
    );
    assert!(!names(&bare_journal).contains(&"routed"));

    // Le même run avec un routeur ne diffère QUE par la paire portée et par
    // l'événement `routed` : même fil, mêmes outils, mêmes autres événements.
    let world = World::new(three_providers());
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let (auto, auto_journal, auto_transcript) = play(Some(router));
    let threads = |t: &Rc<Script>| -> Vec<_> {
        t.seen
            .borrow()
            .iter()
            .map(|r| (r.thread(), r.tools().to_vec()))
            .collect()
    };
    assert_eq!(threads(&bare), threads(&auto));
    assert_eq!(bare_transcript, auto_transcript);
    let without_routed: Vec<_> = names(&auto_journal)
        .into_iter()
        .filter(|n| *n != "routed")
        .collect();
    assert_eq!(names(&bare_journal), without_routed);
}

/// Un tour en mode Auto obtient son modèle par le routeur : la paire décidée
/// est celle que le transport reçoit, pour TOUS les tours de l'objectif, et un
/// événement dit laquelle et pourquoi, avant le premier tour.
#[test]
fn un_tour_en_auto_obtient_son_modele_par_le_routeur() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let transport = script(vec![asks_a_tool()]);
    let (harness, journal) = harnais(transport.clone(), Some(router.clone()));
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));

    let seen = transport.seen.borrow();
    assert_eq!(seen.len(), 2, "un appel d'outil, puis la conclusion");
    for request in seen.iter() {
        assert_eq!(request.provider(), Some("b"));
        assert_eq!(request.model(), Some("m2"));
    }
    // Décidé une fois par message, pas par appel au modèle.
    assert_eq!(world.learning.decisions().len(), 1);
    assert_eq!(router.in_force(), Pick::new("b", "m2"));

    let order = names(&journal);
    let routed = order.iter().position(|n| *n == "routed").unwrap();
    let turn = order.iter().position(|n| *n == "turn").unwrap();
    assert!(routed < turn, "{order:?}");
    let events = routed_events(&journal);
    assert_eq!(events.len(), 1);
    let Event::Routed {
        provider,
        model,
        changed,
        why,
        reason,
        mode,
    } = &events[0]
    else {
        unreachable!()
    };
    assert_eq!((provider.as_str(), model.as_deref()), ("b", Some("m2")));
    assert!(*changed);
    assert_eq!(why, "routed");
    assert!(reason.starts_with("b/m2: chat general turn"), "{reason}");
    assert_eq!(mode.as_deref(), Some("full"));

    // La fin de l'objectif est rendue au port d'apprentissage, pour la décision servie.
    let settled = world.learning.settled();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].decision_id, world.learning.decisions()[0].id);
    assert_eq!(settled[0].served, Pick::new("b", "m2"));
    assert_eq!(settled[0].stop, Stop::Done);
}

/// Le mode change entre deux messages de la MÊME conversation, et le tour
/// suivant le suit.
#[test]
fn le_harnais_suit_un_changement_de_mode_en_cours_de_conversation() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let transport = script(vec![]);
    let (harness, journal) = harnais(transport.clone(), Some(router.clone()));

    run(harness.send("hello"));
    router
        .reroute(Menu::Ticked(vec![entry("c", "m3")]))
        .unwrap();
    run(harness.send("hello"));
    router
        .reroute(Menu::Ticked(vec![entry("a", "m1"), entry("c", "m3")]))
        .unwrap();
    run(harness.send("hello"));

    let seen = transport.seen.borrow();
    let pairs: Vec<_> = seen.iter().map(|r| (r.provider(), r.model())).collect();
    assert_eq!(pairs[0], (Some("b"), Some("m2")), "auto");
    assert_eq!(pairs[1], (Some("c"), Some("m3")), "strict");
    assert!(
        pairs[2] == (Some("a"), Some("m1")) || pairs[2] == (Some("c"), Some("m3")),
        "mixed : parmi les cochés, {:?}",
        pairs[2]
    );
    let modes: Vec<_> = routed_events(&journal)
        .into_iter()
        .map(|e| match e {
            Event::Routed { mode, why, .. } => (mode.unwrap(), why),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(modes[0], ("full".to_string(), "routed".to_string()));
    assert_eq!(modes[1], ("primary".to_string(), "pinned".to_string()));
    assert_eq!(modes[2].0, "mixed");
}

/// « auto » ne sort JAMAIS vers un fournisseur. `auto` est une sentinelle, pas
/// un nom de modèle : transmis tel quel, le moteur le rejette (PO
/// `manager.rs::is_auto_model` — « passed verbatim it becomes `--model auto`,
/// which the Claude Code CLI rejects »).
///
/// Balayage : chaque mode × chaque façon dont la sentinelle peut entrer (le
/// modèle de la session, un candidat de l'hôte, un modèle coché, un modèle posé
/// à la main) × pool vide / plein. Aucune requête reçue par le transport ne
/// porte la sentinelle.
#[test]
fn auto_ne_sort_jamais_vers_un_fournisseur() {
    let sentinels = ["auto", "AUTO", " Auto ", ""];
    let mut requests = 0;
    for sentinel in sentinels {
        let pools: Vec<Vec<Candidate>> = vec![
            vec![],
            vec![cand("a", sentinel, false)],
            vec![cand("a", sentinel, false), cand("b", "m2", false)],
            three_providers(),
        ];
        for pool in pools {
            for seed in 0..6u128 {
                let menus: Vec<Option<Menu>> = vec![
                    None,
                    Some(Menu::Auto),
                    Some(Menu::Ticked(vec![entry("a", sentinel)])),
                    Some(Menu::Ticked(vec![entry("a", sentinel), entry("b", "m2")])),
                ];
                for menu in menus {
                    let world = World::new(pool.clone());
                    let router = Rc::new(TurnRouter::new(world.spec("a", sentinel, seed)));
                    if let Some(menu) = menu {
                        router.reroute(menu).unwrap();
                    }
                    let transport = script(vec![asks_a_tool()]);
                    let (harness, journal) = harnais(transport.clone(), Some(router.clone()));
                    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));
                    // Et encore après un réglage à la main sur la sentinelle.
                    router.set_by_hand("a", sentinel);
                    assert_eq!(run(harness.send("encore")), Running::Ran(Stop::Done));

                    for request in transport.seen.borrow().iter() {
                        requests += 1;
                        assert!(request.provider().is_some());
                        if let Some(model) = request.model() {
                            assert!(
                                !is_auto_model(model),
                                "la sentinelle {model:?} est sortie ({sentinel:?})"
                            );
                        }
                    }
                    for event in routed_events(&journal) {
                        let Event::Routed { model, .. } = event else {
                            unreachable!()
                        };
                        assert!(model.is_none_or(|m| !is_auto_model(&m)));
                    }
                }
            }
        }
    }
    assert!(requests > 1_000, "le balayage a bien tourné : {requests}");
}

/// Zéro candidat sur une session ouverte sur la sentinelle : le tour part
/// quand même, sans modèle imposé — le transport prend son défaut. Ni erreur,
/// ni « auto » envoyé.
#[test]
fn aucun_candidat_et_session_en_auto_le_tour_part_sans_modele_impose() {
    let world = World::new(vec![]);
    let router = Rc::new(TurnRouter::new(world.spec("claude-code", "auto", 1).auto()));
    let transport = script(vec![]);
    let (harness, journal) = harnais(transport.clone(), Some(router));
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));
    let seen = transport.seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].provider(), Some("claude-code"));
    assert_eq!(seen[0].model(), None);
    let events = routed_events(&journal);
    let Event::Routed { why, model, .. } = &events[0] else {
        unreachable!()
    };
    assert_eq!(why, "no_candidate");
    assert_eq!(*model, None);
}

/// La forme de `routed` sur le fil : additive, et un modèle absent ne s'écrit pas.
#[test]
fn l_evenement_routed_a_une_forme_stable_sur_le_fil() {
    let event = Event::Routed {
        provider: "b".into(),
        model: Some("m2".into()),
        changed: true,
        why: "routed".into(),
        reason: "b/m2: chat general turn".into(),
        mode: Some("full".into()),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({
            "type": "routed", "provider": "b", "model": "m2", "changed": true,
            "why": "routed", "reason": "b/m2: chat general turn", "mode": "full"
        })
    );
    let bare: Event = serde_json::from_value(
        json!({"type": "routed", "provider": "a", "changed": false, "why": "pinned"}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&bare).unwrap(),
        json!({"type": "routed", "provider": "a", "changed": false, "why": "pinned"})
    );
    assert_eq!(event.name(), "routed");
}
