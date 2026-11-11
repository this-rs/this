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
        honored: true,
    }
}

fn picture(message: &str) -> TurnInput<'_> {
    TurnInput {
        has_images: true,
        ..text(message)
    }
}

/// Ce que fait le harnais : décider, puis enregistrer la décision (`record` est
/// séparé de `route` depuis la relecture de #251, constat C1).
async fn routed_and_recorded(router: &TurnRouter, turn: TurnInput<'_>) -> Routed {
    let routed = router.route(turn).await;
    if let Some(decision) = &routed.decision {
        router.record(decision).await;
    }
    routed
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
    let routed = run(routed_and_recorded(&router, text("hello")));
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

    // Ce faux moteur lit la paire : il simule un transport qui honore le routeur.
    fn honors_route(&self) -> bool {
        true
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
        advisory,
    } = &events[0]
    else {
        unreachable!()
    };
    assert_eq!((provider.as_str(), model.as_deref()), ("b", Some("m2")));
    assert!(*changed);
    assert_eq!(why, "routed");
    assert!(reason.starts_with("b/m2: chat general turn"), "{reason}");
    assert_eq!(mode.as_deref(), Some("full"));
    assert!(!advisory, "ce transport honore la paire");

    // La fin de l'objectif est rendue au port d'apprentissage, pour la décision servie.
    let settled = world.learning.settled();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].decision_id, world.learning.decisions()[0].id);
    assert_eq!(settled[0].served, Some(Pick::new("b", "m2")));
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
    // `a/m1` gagne nettement parmi les cochés : une seule issue possible.
    world.prefer(("a", "m1"), &[("b", "m2"), ("c", "m3")]);
    run(harness.send("hello"));

    let seen = transport.seen.borrow();
    let pairs: Vec<_> = seen.iter().map(|r| (r.provider(), r.model())).collect();
    assert_eq!(pairs[0], (Some("b"), Some("m2")), "auto");
    assert_eq!(pairs[1], (Some("c"), Some("m3")), "strict");
    assert_eq!(pairs[2], (Some("a"), Some("m1")), "mixed");
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
        advisory: false,
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

// ── 7. Revue de la PR #249 : les constats, un test chacun ───────────────────

fn eager() -> RoutingSettings {
    RoutingSettings {
        exploration_epsilon: 0.25,
        ..RoutingSettings::default()
    }
}

/// Constat 3 : en `mixed`, pool `{b/m2, c/m3}`, paire en vigueur `a/m1` (non
/// cochée). Un tirage d'exploration ne doit pas laisser le tour sur `a/m1`.
#[test]
fn mixed_un_tirage_d_exploration_ne_laisse_jamais_le_tour_hors_du_pool_coche() {
    for seed in 0..400u128 {
        let world = World::new(three_providers());
        let router = TurnRouter::new(
            world
                .spec("a", "m1", seed)
                .settings(eager())
                .ticked(vec![entry("b", "m2"), entry("c", "m3")]),
        );
        let routed = run(router.route(text("hello")));
        assert_ne!(
            routed.route.provider(),
            "a",
            "graine {seed} : a/m1 n'est pas coché ({})",
            routed.why.code()
        );
    }
}

/// Constat 3 : en Auto, liste de la personne à plusieurs entrées chez d'autres
/// fournisseurs. PO `agent_hooks.rs::provider_move_tests::auto_never_chooses_outside_the_persons_list`.
#[test]
fn auto_ne_choisit_jamais_hors_de_la_liste_meme_sur_un_tirage_d_exploration() {
    for seed in 0..400u128 {
        let world = World::new(three_providers());
        let router = TurnRouter::new(
            world
                .spec("a", "m1", seed)
                .settings(eager())
                .auto()
                .auto_models(vec![entry("b", "m2"), entry("c", "m3")]),
        );
        for message in ["hello", "pourquoi ça plante ?", "hello"] {
            let routed = run(router.route(text(message)));
            assert_ne!(routed.route.provider(), "a", "graine {seed}");
        }
    }
}

/// Constat 4 : une session ouverte sur la sentinelle n'a pas de paire à
/// quitter. Le premier choix n'est pas un changement de fournisseur.
#[test]
fn l_ouverture_sur_la_sentinelle_n_est_pas_un_changement_de_fournisseur() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = TurnRouter::new(world.spec("claude-code", "auto", 1).auto());
    assert_eq!(pair(&run(router.route(text("hello")))), on("b", "m2"));
    // Tour 1 : `a` devient le préféré. Rien n'a « bougé » au tour 0.
    world.prefer(("a", "m1"), &[("b", "m2"), ("c", "m3")]);
    let next = run(router.route(text("pourquoi ça plante ?")));
    assert!(
        !reason(&next).contains(AFTER_PROVIDER_MOVE),
        "{}",
        reason(&next)
    );
    assert_eq!(
        looked_at(&next).len(),
        3,
        "aucune restriction de fournisseur"
    );
    assert_eq!(pair(&next), on("a", "m1"));

    // Même sur un tirage d'exploration, l'ouverture part sur une vraie paire.
    for seed in 0..300u128 {
        let world = World::new(three_providers());
        let router = TurnRouter::new(
            world
                .spec("claude-code", "auto", seed)
                .settings(eager())
                .auto(),
        );
        let routed = run(router.route(text("hello")));
        assert!(routed.route.model().is_some(), "graine {seed}");
        assert_ne!(routed.route.provider(), "claude-code");
    }
}

/// Un pool dont l'appel règle le modèle à la main PENDANT que le routeur l'attend.
struct Meddler {
    pool: Vec<Candidate>,
    router: RefCell<Option<Rc<TurnRouter>>>,
}

#[async_trait(?Send)]
impl this_harness::routing::PoolSource for Meddler {
    async fn pool(&self) -> Vec<Candidate> {
        if let Some(router) = self.router.borrow().as_ref() {
            router.set_by_hand("c", "m3");
        }
        self.pool.clone()
    }
}

/// Constat 6 : le réglage de la personne gagne toujours, même s'il arrive
/// pendant qu'une décision est en cours.
#[test]
fn un_reglage_a_la_main_pendant_la_decision_gagne() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let meddler = Rc::new(Meddler {
        pool: three_providers(),
        router: RefCell::new(None),
    });
    let mut spec = world.spec("a", "m1", 1).auto();
    spec.pool = meddler.clone();
    let router = Rc::new(TurnRouter::new(spec));
    *meddler.router.borrow_mut() = Some(router.clone());
    let routed = run(router.route(text("hello")));
    assert_eq!(
        router.in_force(),
        Pick::new("c", "m3"),
        "la personne a choisi c/m3"
    );
    assert_eq!(pair(&routed), on("c", "m3"));
    *meddler.router.borrow_mut() = None;
}

/// Constat 7 : une image déjà dans le fil part avec chaque requête. Un tour de
/// texte ne doit pas partir vers un modèle sans vision.
#[test]
fn une_image_deja_dans_le_fil_compte_pour_un_tour_de_texte() {
    use this_harness::{ImagePart, ImageSource, Message};
    let world = World::new(vec![cand("a", "seer", true), cand("b", "blind", false)]);
    world.prefer(("b", "blind"), &[("a", "seer")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "seer", 1).auto()));
    let transport = script(vec![]);
    let (harness, _journal) = harnais(transport.clone(), Some(router));
    harness.seed(vec![
        Message::user_with_images(
            "regarde",
            vec![ImagePart::new("image/png", ImageSource::Ref("abc".into()))],
        ),
        Message::assistant("vu"),
    ]);
    run(harness.send("et maintenant, résume"));
    let seen = transport.seen.borrow();
    assert!(seen[0].thread().iter().any(|m| !m.images().is_empty()));
    assert_eq!(
        (seen[0].provider(), seen[0].model()),
        (Some("a"), Some("seer")),
        "le fil porte une image : pas de modèle sans vision"
    );
}

/// Un moteur dont le premier appel tombe en panne, après avoir tenu un
/// second message : deux objectifs dans le même `pump`.
struct FailsOnce {
    handle: RefCell<Option<this_harness::Handle>>,
    calls: std::cell::Cell<u32>,
}

#[async_trait(?Send)]
impl Transport for FailsOnce {
    async fn stream(
        &self,
        _request: &Request,
        _cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        self.calls.set(self.calls.get() + 1);
        if self.calls.get() == 1 {
            if let Some(handle) = self.handle.borrow().as_ref() {
                handle.enqueue("et ça ?");
            }
            return Err(Failure::http(400));
        }
        delta("fini");
        Ok(Reply {
            text: "fini".to_string(),
            calls: vec![],
            finish: Finish::Stop,
        })
    }

    fn honors_route(&self) -> bool {
        true
    }
}

/// Constat 5b : le code de panne d'un objectif ne déborde pas sur le suivant.
#[test]
fn le_code_de_panne_d_un_objectif_ne_deborde_pas_sur_le_suivant() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let transport = Rc::new(FailsOnce {
        handle: RefCell::new(None),
        calls: std::cell::Cell::new(0),
    });
    let journal: Journal = Rc::new(RefCell::new(Vec::new()));
    let sink = journal.clone();
    let harness = Harness::new(
        "tu es l'atelier",
        Limits::default(),
        transport.clone(),
        Rc::new(Tools),
        move |event| sink.borrow_mut().push(event),
    )
    .with_router(router);
    *transport.handle.borrow_mut() = Some(harness.handle());
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));
    *transport.handle.borrow_mut() = None;

    let settled = world.learning.settled();
    assert_eq!(settled.len(), 2, "{settled:?}");
    assert!(
        settled[0].code.is_some(),
        "le premier objectif est en panne"
    );
    assert_eq!(settled[1].stop, Stop::Done);
    assert_eq!(settled[1].code, None, "le second a réussi : aucun code");
    let stopped = journal.borrow().iter().rev().find_map(|e| match e {
        Event::Stopped { stop, code, .. } => Some((stop.clone(), code.clone())),
        _ => None,
    });
    assert_eq!(stopped, Some((Stop::Done, None)));
}

// ── 8. Revue de la PR #249, suite : transport, ports lents, fins de tour ────

/// Un temps de test : `sleep` rend tout de suite et avance l'heure de ce qu'on
/// a demandé. Un port qui ne répond jamais rencontre donc sa borne sans attendre.
struct Fast(std::cell::Cell<f64>);

impl this_harness::Clock for Fast {
    fn now_ms(&self) -> f64 {
        self.0.get()
    }
    fn sleep(&self, ms: u64) -> Option<this_harness::Sleep> {
        self.0.set(self.0.get() + ms as f64);
        Some(Box::pin(async {}))
    }
}

/// Un pool qui ne répond jamais.
struct Hangs;

#[async_trait(?Send)]
impl this_harness::routing::PoolSource for Hangs {
    async fn pool(&self) -> Vec<Candidate> {
        std::future::pending().await
    }
}

/// Un apprentissage dont `settle` ne rend jamais la main (et `record` non plus si demandé).
struct StuckLearning {
    inner: Rc<this_harness::routing::MemoryLearning>,
    /// Combien de `settle` pendent encore avant que les suivants répondent.
    hang_settles: std::cell::Cell<u32>,
    /// `record` ne répond jamais.
    hang_record: bool,
}

impl StuckLearning {
    fn new(
        inner: Rc<this_harness::routing::MemoryLearning>,
        hang_settles: u32,
        hang_record: bool,
    ) -> Self {
        Self {
            inner,
            hang_settles: std::cell::Cell::new(hang_settles),
            hang_record,
        }
    }
}

#[async_trait(?Send)]
impl this_harness::routing::Learning for StuckLearning {
    async fn arms(&self, class: &str) -> Vec<ArmStats> {
        self.inner.arms(class).await
    }
    fn hints(&self) -> this_harness::routing::PriorHints {
        self.inner.hints()
    }
    async fn record(&self, decision: &this_harness::routing::Decision) {
        if self.hang_record {
            std::future::pending::<()>().await;
        }
        self.inner.record(decision).await;
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, code: Option<&str>) {
        if self.hang_settles.get() > 0 {
            self.hang_settles.set(self.hang_settles.get() - 1);
            std::future::pending::<()>().await;
        }
        self.inner.settle(id, served, stop, code).await;
    }
}

/// Un moteur qui NE déclare PAS honorer la paire (le défaut).
struct Deaf(Rc<Script>);

#[async_trait(?Send)]
impl Transport for Deaf {
    async fn stream(
        &self,
        request: &Request,
        cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure> {
        self.0.stream(request, cancel, delta).await
    }
}

fn build(
    transport: Rc<dyn Transport>,
    router: Rc<TurnRouter>,
    clock: Option<Rc<dyn this_harness::Clock>>,
) -> (Harness, Journal) {
    let journal: Journal = Rc::new(RefCell::new(Vec::new()));
    let sink = journal.clone();
    let mut harness = Harness::new(
        "tu es l'atelier",
        Limits::default(),
        transport,
        Rc::new(Tools),
        move |event| sink.borrow_mut().push(event),
    )
    .with_router(router);
    if let Some(clock) = clock {
        harness = harness.with_clock(clock);
    }
    (harness, journal)
}

/// Constat 1 : un transport qui ne déclare pas honorer la paire ne la sert pas.
/// L'événement dit « consultatif », et l'apprentissage ne reçoit AUCUNE paire servie.
#[test]
fn un_transport_qui_n_honore_pas_la_paire_rend_la_decision_consultative() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let (harness, journal) = build(Rc::new(Deaf(script(vec![]))), router, None);
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));

    let events = routed_events(&journal);
    let Event::Routed {
        provider,
        changed,
        advisory,
        ..
    } = &events[0]
    else {
        unreachable!()
    };
    assert_eq!(provider, "b", "ce que le routeur a décidé");
    assert!(*changed);
    assert!(*advisory, "mais personne ne l'honore");
    let settled = world.learning.settled();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].served, None, "aucun bras nourri");
    assert_eq!(settled[0].stop, Stop::Done);
}

/// Constat 1 (forme sur le fil) : `advisory` s'écrit seulement quand il est vrai.
#[test]
fn advisory_s_ecrit_seulement_quand_il_est_vrai() {
    let event = Event::Routed {
        provider: "b".into(),
        model: None,
        changed: false,
        why: "kept".into(),
        reason: String::new(),
        mode: None,
        advisory: true,
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap()["advisory"],
        json!(true)
    );
    let old: Event = serde_json::from_value(
        json!({"type": "routed", "provider": "a", "changed": false, "why": "kept"}),
    )
    .unwrap();
    assert!(matches!(
        old,
        Event::Routed {
            advisory: false,
            ..
        }
    ));
}

/// Constat 2 : un pool qui ne répond jamais ne retient pas le tour. Le tour
/// part sur la paire en vigueur, et l'événement dit `timed_out`.
#[test]
fn un_pool_qui_pend_ne_bloque_pas_le_tour() {
    let world = World::new(three_providers());
    let mut spec = world.spec("a", "m1", 1).auto();
    spec.pool = Rc::new(Hangs);
    let router = Rc::new(TurnRouter::new(spec));
    let transport = script(vec![]);
    let (harness, journal) = build(
        transport.clone(),
        router,
        Some(Rc::new(Fast(std::cell::Cell::new(0.0)))),
    );
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));
    assert!(!harness.handle().busy());
    let seen = transport.seen.borrow();
    assert_eq!(
        (seen[0].provider(), seen[0].model()),
        (Some("a"), Some("m1"))
    );
    let events = routed_events(&journal);
    let Event::Routed { why, changed, .. } = &events[0] else {
        unreachable!()
    };
    assert_eq!(why, "timed_out");
    assert!(!changed);
}

/// L'horloge d'une page sans minuteur : `sleep` rend `None` (c'est ce que fait
/// `SystemClock` en wasm32). Aucune borne de temps n'existe.
struct NoTimer;

impl this_harness::Clock for NoTimer {
    fn now_ms(&self) -> f64 {
        0.0
    }
    fn sleep(&self, _ms: u64) -> Option<this_harness::Sleep> {
        None
    }
}

/// Rend la main `n` fois à l'exécuteur, en se réveillant à chaque fois.
fn yields(n: u32) -> impl std::future::Future<Output = ()> {
    let mut left = n;
    std::future::poll_fn(move |cx| {
        if left == 0 {
            std::task::Poll::Ready(())
        } else {
            left -= 1;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
}

/// Revue de #251, B1/B2 : SANS minuteur, un pool qui ne répond jamais, et
/// `interrupt()` venu d'une AUTRE tâche, APRÈS que le run s'est suspendu sur le
/// pool. L'annulation doit réveiller l'attente : le run se termine `Cancelled`,
/// sans appel au moteur, et n'est plus occupé. Avant le correctif, ce test ne
/// rendait jamais (l'attente était suspendue sur `pending()` sans réveil).
#[test]
fn l_annulation_coupe_un_routeur_qui_pend() {
    let world = World::new(three_providers());
    let mut spec = world.spec("a", "m1", 1).auto();
    spec.pool = Rc::new(Hangs);
    let router = Rc::new(TurnRouter::new(spec));
    let transport = script(vec![]);
    let (harness, journal) = build(transport.clone(), router, Some(Rc::new(NoTimer)));
    let handle = harness.handle();
    let polled_first = Rc::new(std::cell::Cell::new(false));
    let seen_busy = polled_first.clone();
    let (ran, ()) = run(futures::future::join(harness.send("hello"), async move {
        // `join` sonde d'abord le run : il est déjà suspendu sur le pool ici.
        yields(3).await;
        seen_busy.set(handle.busy());
        handle.interrupt();
    }));
    assert!(
        polled_first.get(),
        "l'interruption est arrivée pendant le run"
    );
    assert_eq!(ran, Running::Ran(Stop::Cancelled));
    assert!(!harness.handle().busy());
    assert!(transport.seen.borrow().is_empty(), "aucun appel au moteur");
    assert!(names(&journal).contains(&"stopped"));
}

/// Constat 2 : un apprentissage dont `settle` pend ne retient ni `Stopped`, ni `busy`.
#[test]
fn un_settle_qui_pend_ne_retient_ni_stopped_ni_busy() {
    let world = World::new(three_providers());
    let learning = Rc::new(StuckLearning::new(world.learning.clone(), u32::MAX, false));
    let router = Rc::new(TurnRouter::new(
        world.spec("a", "m1", 1).auto().learning(learning),
    ));
    let (harness, journal) = build(
        script(vec![]),
        router,
        Some(Rc::new(Fast(std::cell::Cell::new(0.0)))),
    );
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));
    assert!(!harness.handle().busy());
    assert!(names(&journal).contains(&"stopped"));
    assert!(
        world.learning.settled().is_empty(),
        "le settle n'est jamais venu"
    );
}

/// Constat 5a : une future lâchée avant la clôture laisse une décision appliquée
/// sans `settle` ; le tour suivant la déclare abandonnée (`Interrupted`, aucune
/// paire servie), jamais une récompense.
#[test]
fn un_tour_abandonne_est_declare_au_tour_suivant() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let first = run(routed_and_recorded(&router, text("hello")));
    assert!(first.changed);
    // Le tour est abandonné ici : aucun `settle`. Le tour suivant le dit.
    let _ = run(router.route(text("hello")));
    let settled = world.learning.settled();
    assert_eq!(settled.len(), 1, "{settled:?}");
    assert_eq!(settled[0].decision_id, world.learning.decisions()[0].id);
    assert_eq!(settled[0].stop, Stop::Interrupted);
    assert_eq!(settled[0].served, None);
}

/// Constat 5a/9 : `settle` sur un échec et sur une annulation. Un échec rend le
/// code du transport ; une annulation rend `Cancelled`. Dans les deux cas la
/// paire servie est rendue (le transport honore).
#[test]
fn settle_sur_echec_et_sur_annulation() {
    // Échec : le moteur répond 400.
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let transport = Rc::new(FailsOnce {
        handle: RefCell::new(None),
        calls: std::cell::Cell::new(0),
    });
    let (harness, _) = build(transport, router, None);
    run(harness.send("hello"));
    let settled = world.learning.settled();
    assert!(
        matches!(settled[0].stop, Stop::Failed(_)),
        "{:?}",
        settled[0]
    );
    assert_eq!(settled[0].code.as_deref(), Some("http"));
    assert_eq!(settled[0].served, Some(Pick::new("b", "m2")));

    // Annulation : le moteur interrompt le run pendant le flux.
    struct Cancels(RefCell<Option<this_harness::Handle>>);
    #[async_trait(?Send)]
    impl Transport for Cancels {
        async fn stream(&self, _r: &Request, _c: &Cancel, _d: Delta) -> Result<Reply, Failure> {
            if let Some(handle) = self.0.borrow().as_ref() {
                handle.interrupt();
            }
            Err(Failure::aborted())
        }
        fn honors_route(&self) -> bool {
            true
        }
    }
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let transport = Rc::new(Cancels(RefCell::new(None)));
    let (harness, _) = build(transport.clone(), router, None);
    *transport.0.borrow_mut() = Some(harness.handle());
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Cancelled));
    *transport.0.borrow_mut() = None;
    let settled = world.learning.settled();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].stop, Stop::Cancelled);
}

/// Constat 5c : sur `exploration_never_moves`, la décision enregistrée dit
/// `applied: false` et `used` = la paire en vigueur ; jamais une paire qui
/// n'a pas servi.
#[test]
fn une_exploration_refusee_n_est_pas_notee_comme_appliquee() {
    let mut seen = 0;
    for seed in 0..600u128 {
        let world = World::new(vec![cand("a", "m1", false), cand("b", "m2", false)]);
        world.prefer(("a", "m1"), &[("b", "m2")]);
        let router = TurnRouter::new(world.spec("a", "m1", seed).settings(eager()).auto());
        let routed = run(routed_and_recorded(&router, text("hello")));
        let d = routed.decision.as_ref().unwrap();
        if routed.why == RouteWhy::ExplorationNeverMoves {
            seen += 1;
            assert!(!d.applied, "graine {seed}");
            assert_eq!(d.used, Some(Pick::new("a", "m1")));
            assert_eq!(d.chosen, Some(Pick::new("b", "m2")));
            assert_eq!(world.learning.decisions()[0].used, d.used);
        } else {
            assert_eq!(d.used.as_ref(), Some(&routed_pair(&routed)));
        }
    }
    assert!(seen > 20, "{seen}");
}

fn routed_pair(routed: &Routed) -> Pick {
    Pick::new(
        routed.route.provider(),
        routed.route.model().unwrap_or("auto"),
    )
}

/// Constat 5c : un exécuteur en `full` + `advisory` n'applique rien ; la
/// décision dit `used` = la paire en vigueur.
#[test]
fn un_executeur_en_full_advisory_n_est_pas_note_comme_applique() {
    use this_harness::routing::Role;
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let settings = RoutingSettings {
        mode: ProviderRoutingMode::Full,
        stage: LearningStage::Advisory,
        ..quiet()
    };
    let mut spec = world.spec("a", "m1", 1).settings(settings);
    spec.role = Role::Executor;
    let router = TurnRouter::new(spec);
    let routed = run(router.route(text("hello")));
    let d = routed.decision.as_ref().unwrap();
    assert!(!d.applied);
    assert_eq!(d.used, Some(Pick::new("a", "m1")));
    assert_eq!(routed.why, RouteWhy::RecordedOnly);
}

/// Constat 9 : `under_margin`. Tour stable (même classe juste après un
/// changement de modèle) : la décision compare les moyennes, mais le garde de
/// fournisseur lit les notes tirées — un challenger meilleur en moyenne dont le
/// tirage ne passe pas la marge ne fait pas changer de fournisseur.
#[test]
fn un_ecart_sous_la_marge_ne_change_pas_de_fournisseur() {
    let arm_of = |provider: &str, model: &str, alpha: f64, beta: f64, n: u64| ArmStats {
        n,
        ..ArmStats::fresh(ArmKey::new(GENERAL, provider, model), alpha, beta)
    };
    let mut held = 0;
    for seed in 0..800u128 {
        let world = World::new(vec![cand("a", "m1", false), cand("a", "m2", false)]);
        world.learning.set_arms(vec![
            arm_of("a", "m1", 1.0, 30.0, 30),
            arm_of("a", "m2", 8.0, 12.0, 5),
            arm_of("b", "m3", 12.0, 8.0, 5),
        ]);
        let router = TurnRouter::new(world.spec("a", "m1", seed * 2).auto());
        let first = run(router.route(text("hello")));
        if pair(&first) != on("a", "m2") {
            continue; // tirage rare : a/m2 sous la marge de a/m1, pas le cas visé
        }
        world.pool.set(vec![
            cand("a", "m1", false),
            cand("a", "m2", false),
            cand("b", "m3", false),
        ]);
        let second = run(router.route(text("hello again")));
        if second.why == RouteWhy::UnderMargin {
            held += 1;
            assert_eq!(pair(&second), on("a", "m2"), "le fournisseur ne change pas");
            let d = second.decision.as_ref().unwrap();
            assert_eq!(d.chosen, Some(Pick::new("b", "m3")));
            assert!(!d.applied);
            assert_eq!(d.used, Some(Pick::new("a", "m2")));
        }
    }
    eprintln!("under_margin : held = {held} sur 800 graines");
    assert!(held > 5, "le cas existe : {held}");
}

/// Constat 8 : des faits d'hôte invalides n'empoisonnent jamais une décision
/// de bout en bout : le candidat est écarté avec un motif nommé.
#[test]
fn des_faits_invalides_sont_ecartes_avec_un_motif_nomme() {
    let mut bad_price = cand("a", "m1", false);
    bad_price.price = Some(this_harness::routing::Price {
        input_per_mtok: f64::NAN,
        output_per_mtok: 1.0,
    });
    bad_price.cost_basis = CostBasis::Priced;
    let world = World::new(vec![bad_price, cand("b", "m2", false)]);
    world.learning.set_arms(vec![ArmStats::fresh(
        ArmKey::new(GENERAL, "b", "m2"),
        f64::INFINITY,
        1.0,
    )]);
    // Le seul candidat sain a un bras infini : personne d'éligible, repli.
    let router = TurnRouter::new(world.spec("a", "m1", 1).auto());
    let routed = run(router.route(text("hello")));
    assert_eq!(routed.why, RouteWhy::NoCandidate);
    let codes: Vec<_> = routed
        .decision
        .as_ref()
        .unwrap()
        .alternatives
        .iter()
        .map(|a| a.rejected.as_ref().unwrap().code())
        .collect();
    assert_eq!(codes, vec!["invalid_price", "invalid_arm"]);
}

// ── 9. Relecture de #251 : les constats, un test chacun ─────────────────────

/// C1 : un `record` qui pend ne peut plus rien changer au tour. L'état est
/// réécrit et l'événement émis AVANT `record` : la requête part sur la paire
/// décidée, et l'événement le dit (`changed: true`, `why: routed`, motif).
#[test]
fn un_record_qui_pend_ne_desaccorde_pas_l_evenement_et_la_requete() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let learning = Rc::new(StuckLearning::new(world.learning.clone(), 0, true));
    let router = Rc::new(TurnRouter::new(
        world.spec("a", "m1", 1).auto().learning(learning),
    ));
    let transport = script(vec![]);
    let (harness, journal) = build(
        transport.clone(),
        router,
        Some(Rc::new(Fast(std::cell::Cell::new(0.0)))),
    );
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));
    let seen = transport.seen.borrow();
    assert_eq!(
        (seen[0].provider(), seen[0].model()),
        (Some("b"), Some("m2"))
    );
    let events = routed_events(&journal);
    let Event::Routed {
        provider,
        changed,
        why,
        reason,
        ..
    } = &events[0]
    else {
        unreachable!()
    };
    assert_eq!(provider, "b");
    assert!(*changed);
    assert_eq!(why, "routed");
    assert!(!reason.is_empty());
    // Le tour est parti : sa fin est rendue, servie par la paire décidée.
    let settled = world.learning.settled();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].served, Some(Pick::new("b", "m2")));
}

/// C1 : un objectif annulé pendant `record`, sans minuteur. Aucune requête
/// n'est partie : la fin rendue ne déclare AUCUNE paire servie.
#[test]
fn un_objectif_annule_avant_toute_requete_ne_declare_aucune_paire_servie() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let learning = Rc::new(StuckLearning::new(world.learning.clone(), 0, true));
    let router = Rc::new(TurnRouter::new(
        world.spec("a", "m1", 1).auto().learning(learning),
    ));
    let transport = script(vec![]);
    let (harness, _journal) = build(transport.clone(), router, Some(Rc::new(NoTimer)));
    let handle = harness.handle();
    let (ran, ()) = run(futures::future::join(harness.send("hello"), async move {
        yields(3).await;
        handle.interrupt();
    }));
    assert_eq!(ran, Running::Ran(Stop::Cancelled));
    assert!(transport.seen.borrow().is_empty());
    let settled = world.learning.settled();
    assert_eq!(settled.len(), 1, "{settled:?}");
    assert_eq!(settled[0].stop, Stop::Cancelled);
    assert_eq!(settled[0].served, None, "aucune requête n'est partie");
}

/// C3 : une décision consultative (transport qui n'honore pas la paire) est
/// enregistrée `applied: false`, `used: None`.
#[test]
fn une_decision_consultative_est_enregistree_non_appliquee() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let router = Rc::new(TurnRouter::new(world.spec("a", "m1", 1).auto()));
    let (harness, _) = build(Rc::new(Deaf(script(vec![]))), router, None);
    run(harness.send("hello"));
    let recorded = world.learning.decisions();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].chosen, Some(Pick::new("b", "m2")));
    assert!(!recorded[0].applied);
    assert_eq!(recorded[0].used, None);
}

/// C4 : la sentinelle ne fuit jamais dans `Decision.used`.
#[test]
fn la_sentinelle_ne_fuit_jamais_dans_used() {
    // Aucun candidat, session ouverte sur `auto`.
    let world = World::new(vec![]);
    let router = TurnRouter::new(world.spec("claude-code", "auto", 1).auto());
    let routed = run(router.route(text("hello")));
    assert_eq!(routed.why, RouteWhy::NoCandidate);
    assert_eq!(routed.decision.as_ref().unwrap().used, None);
    // Décision notée seulement (`full` + `shadow`), session sur `auto`.
    let world = World::new(three_providers());
    let settings = RoutingSettings {
        mode: ProviderRoutingMode::Full,
        stage: LearningStage::Shadow,
        ..quiet()
    };
    let router = TurnRouter::new(world.spec("claude-code", "AUTO", 1).settings(settings));
    let routed = run(router.route(text("hello")));
    assert_eq!(routed.why, RouteWhy::RecordedOnly);
    let decision = routed.decision.as_ref().unwrap();
    assert_eq!(decision.used, None);
    assert!(
        world
            .learning
            .decisions()
            .iter()
            .all(|d| d.used.as_ref().is_none_or(|u| !is_auto_model(&u.model)))
    );
}

/// C6 : la paire en vigueur `m` est la même que `m[1m]` du pool. Exploration
/// forte : la garde et l'hystérésis la reconnaissent, le tour ne bouge jamais.
#[test]
fn la_paire_en_vigueur_se_reconnait_sous_sa_variante_de_fenetre() {
    for seed in 0..400u128 {
        let world = World::new(vec![cand("a", "m[1m]", false), cand("b", "x", false)]);
        world.prefer(("a", "m[1m]"), &[("b", "x")]);
        let router = TurnRouter::new(world.spec("a", "m", seed).settings(eager()).auto());
        let routed = run(router.route(text("hello")));
        assert!(!routed.changed, "graine {seed} : {}", routed.why.code());
        assert_eq!(pair(&routed), on("a", "m"), "graine {seed}");
        let d = routed.decision.as_ref().unwrap();
        assert_eq!(d.used, Some(Pick::new("a", "m")));
    }
}

/// Un pool qui, pendant que le routeur l'attend, coche un menu VIDE.
struct EmptyMenu(RefCell<Option<Rc<TurnRouter>>>);

#[async_trait(?Send)]
impl this_harness::routing::PoolSource for EmptyMenu {
    async fn pool(&self) -> Vec<Candidate> {
        if let Some(router) = self.0.borrow().as_ref() {
            assert_eq!(router.reroute(Menu::Ticked(vec![])), Err(EmptyPool));
        }
        three_providers()
    }
}

/// M1 : un menu vide est refusé et ne jette pas la décision en cours.
#[test]
fn un_menu_vide_ne_jette_pas_la_decision_en_cours() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let pool = Rc::new(EmptyMenu(RefCell::new(None)));
    let mut spec = world.spec("a", "m1", 1).auto();
    spec.pool = pool.clone();
    let router = Rc::new(TurnRouter::new(spec));
    *pool.0.borrow_mut() = Some(router.clone());
    let routed = run(router.route(text("hello")));
    *pool.0.borrow_mut() = None;
    assert_eq!(routed.why, RouteWhy::Routed);
    assert_eq!(pair(&routed), on("b", "m2"));
}

/// M5 : un `settle` coupé par sa borne laisse la décision ouverte ; le message
/// suivant la déclare `Interrupted`, sans paire servie.
#[test]
fn un_settle_coupe_est_declare_interrompu_au_message_suivant() {
    let world = World::new(three_providers());
    world.prefer(("b", "m2"), &[("a", "m1"), ("c", "m3")]);
    let learning = Rc::new(StuckLearning::new(world.learning.clone(), 1, false));
    let router = Rc::new(TurnRouter::new(
        world.spec("a", "m1", 1).auto().learning(learning),
    ));
    let (harness, _) = build(
        script(vec![]),
        router,
        Some(Rc::new(Fast(std::cell::Cell::new(0.0)))),
    );
    assert_eq!(run(harness.send("hello")), Running::Ran(Stop::Done));
    assert!(
        world.learning.settled().is_empty(),
        "le premier settle a pendu"
    );
    assert_eq!(run(harness.send("encore")), Running::Ran(Stop::Done));
    let settled = world.learning.settled();
    let first = world.learning.decisions()[0].id;
    assert_eq!(settled[0].decision_id, first);
    assert_eq!(settled[0].stop, Stop::Interrupted);
    assert_eq!(settled[0].served, None);
    assert_eq!(
        settled.len(),
        2,
        "puis la fin du second objectif : {settled:?}"
    );
    assert_eq!(settled[1].stop, Stop::Done);
}
