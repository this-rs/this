//! # Le mode Auto : qui répond à ce tour ?
//!
//! Une conversation peut tourner en « Auto » : au lieu d'un modèle fixé, un
//! routeur choisit à chaque tour la paire (fournisseur, modèle) parmi des
//! candidats, selon la nature du tour.
//!
//! ```text
//! tour ──► signature ──► candidats ──filtres──► éligibles ──note──► décision ──► Route
//!          (classe,       (PoolSource,           (capacités,        (Thompson,     (jamais
//!           besoins)       l'hôte)                quota, santé…)     hystérésis)    « auto »)
//!                                                    │ 1 seul : on part dessus
//!                                                    │ 0 : repli sur la paire en vigueur
//! ```
//!
//! Trois modes, que la conversation choisit dans son menu et peut changer en
//! cours de route ([`TurnRouter::reroute`]) :
//!
//! | code | écran | ce que fait le routeur |
//! |---|---|---|
//! | `primary` | strict | rien : le modèle coché est imposé |
//! | `mixed` | mixed | choisit parmi les modèles cochés |
//! | `full` | auto | choisit tout, le fournisseur compris |
//!
//! Le cœur est **pur** : aucune entrée/sortie, aucune horloge, aucun hasard qui
//! ne soit injecté ([`Seeds`]). Ce qui dépend de l'hôte — les candidats vivants,
//! les quotas, l'apprentissage — passe par des ports ([`PoolSource`],
//! [`QuotaView`], [`Learning`]). Il compile donc en wasm32 comme le reste du crate.
//!
//! Portage du routage cognitif de Project Orchestrator ; l'inventaire, le
//! tableau d'écart et les divergences assumées sont dans
//! `docs/harnais-mode-auto.md`.

mod candidate;
mod decide;
mod mode;
mod router;
mod scorer;
mod signature;

pub use candidate::{
    Candidate, Capabilities, CostBasis, Filtered, MAX_PRICE_PER_MTOK, MemoryQuota, Pick, Price,
    QuotaExhaustion, QuotaView, RejectReason, Rejected, Slot, UnknownWindow, apply_quota, filter,
    is_auto_model,
};
pub use decide::{
    Alternative, DecideRequest, Decision, HYSTERESIS_MARGIN, HYSTERESIS_WARMUP, NO_CANDIDATE,
    confidence_from_scores, decide, is_applied, utility_of,
};
pub use mode::{
    ConversationRoutingMode, DEFAULT_COST_WEIGHT, DEFAULT_EXPLORATION_EPSILON,
    DEFAULT_LATENCY_WEIGHT, DEFAULT_QUOTA_WEIGHT, LearningStage, MAX_EXPLORATION_EPSILON,
    PoolEntry, ProviderRoutingMode, QUOTA_NEAR_LIMIT, RoutingSettings, conversation_routes,
    conversation_stage, pool_spans_providers,
};
pub use router::{
    AFTER_PROVIDER_MOVE, EmptyPool, Learning, MemoryLearning, MemoryPool, Menu, NARROWED_BY_PERSON,
    NoLearning, PERSON_LIST_UNAVAILABLE, PROVIDER_IMPOSED, PoolSource, REPAIR_ATTEMPTS, Route,
    RouteWhy, Routed, RouterSpec, SAME_CLASS_NO_EXPLORE, Seeds, SequenceSeeds, Settled, TurnInput,
    TurnRouter, restrict_to_person_list,
};
pub use scorer::{
    ALIAS_DEEP, ALIAS_FAST, ArmKey, ArmStats, PriorHints, Scored, arm_key_of, beta_draw, prior_for,
    quota_penalty, score, seed_of,
};
pub use signature::{
    ChatIntent, ContextHints, MAX_CONTEXT_TOKENS, MIN_CONTEXT_TOKENS, Role, TaskClass,
    TurnSignature, detect_intent,
};
