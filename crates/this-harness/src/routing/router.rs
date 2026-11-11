//! Le routeur d'une conversation : il tient ce que la conversation a demandé
//! (son mode, son pool, la liste de la personne), la paire en vigueur, et rend
//! pour chaque tour la paire sur laquelle il part.
//!
//! Le routeur ne fait aucune entrée/sortie : ce qui dépend de l'hôte passe par
//! des ports ([`PoolSource`], [`Learning`], [`Seeds`]). Ni horloge, ni hasard qui
//! ne soit injecté.
//!
//! Portage de PO : `src/chat/agent_hooks.rs` (`TurnRouter`, `routing_of`,
//! `directive_for_turn`, `plan_provider_move`, `model_for_images`, `reroute`) et
//! `src/chat/provider/cognitive/auto_models.rs` (`restrict`). Ce qui diverge de
//! PO est dit là où ça diverge, et repris dans `docs/harnais-mode-auto.md`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use async_trait::async_trait;
use this_agent::Stop;

use super::candidate::{self, Candidate, Pick, Slot, is_auto_model, same_model, same_pair};
use super::decide::{DecideRequest, Decision, HYSTERESIS_MARGIN, decide};
use super::mode::{
    ConversationRoutingMode, LearningStage, PoolEntry, ProviderRoutingMode, RoutingSettings,
    conversation_stage,
};
use super::scorer::{ArmStats, PriorHints};
use super::signature::{ContextHints, Role, TurnSignature};

/// Code de motif d'une décision dont les candidats ont été restreints à la
/// liste de la personne.
pub const NARROWED_BY_PERSON: &str = "auto_models";
/// Code de motif d'une décision prise parmi tous les modèles parce que rien de
/// la liste de la personne ne pouvait servir : l'avis visible du repli.
pub const PERSON_LIST_UNAVAILABLE: &str = "auto_models_unavailable";
/// Suffixe de motif d'un tour de même classe juste après un changement.
pub const SAME_CLASS_NO_EXPLORE: &str = "same_class_no_explore";
/// Suffixe de motif du tour juste après un changement de fournisseur.
pub const AFTER_PROVIDER_MOVE: &str = "after_provider_move";
/// Suffixe de motif d'un tour dont le fournisseur est imposé.
pub const PROVIDER_IMPOSED: &str = "provider_imposed";

/// La paire sur laquelle un tour part, telle qu'elle sort vers un transport.
///
/// Le modèle est privé **exprès** : la seule façon d'en construire une passe
/// par [`Route::of`], qui écarte la sentinelle `auto`. « auto » ne sort donc
/// jamais vers un fournisseur — il n'y a pas d'autre porte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    provider_id: String,
    model: Option<String>,
}

impl Route {
    /// `model` vaut `None` quand le nom donné est la sentinelle (`auto`, vide) :
    /// le transport prend alors son propre défaut.
    pub fn of(provider_id: impl Into<String>, model: &str) -> Self {
        Self {
            provider_id: provider_id.into(),
            model: (!is_auto_model(model)).then(|| model.trim().to_owned()),
        }
    }

    /// L'instance de fournisseur.
    pub fn provider(&self) -> &str {
        &self.provider_id
    }

    /// Le modèle, jamais `auto` ; `None` : le défaut du transport.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

/// Les modèles parmi lesquels une décision choisit, avec leurs faits vivants
/// (santé, consentement, quotas déjà appliqués — voir
/// [`apply_quota`](super::candidate::apply_quota)). C'est l'hôte qui sait.
#[async_trait(?Send)]
pub trait PoolSource {
    /// Toutes les paires que la conversation pourrait utiliser, tous
    /// fournisseurs confondus. Vide : le routeur se replie sur la paire en vigueur.
    async fn pool(&self) -> Vec<Candidate>;
}

/// Le port par lequel l'apprentissage (coût × efficacité) se branchera.
///
/// Ce lot ne l'implémente pas : [`NoLearning`] ne sait rien et n'écrit rien, et
/// la note retombe sur l'a priori. Un hôte qui apprend rend ses bras ici, et
/// reçoit chaque décision et chaque fin de tour pour les nourrir.
#[async_trait(?Send)]
pub trait Learning {
    /// Les bras stockés de la classe `class` (clé de [`TurnSignature::arm_key`]).
    async fn arms(&self, _class: &str) -> Vec<ArmStats> {
        Vec::new()
    }

    /// Les alias déclarés, lus comme un a priori.
    fn hints(&self) -> PriorHints {
        PriorHints::new()
    }

    /// Une décision vient d'être prise, appliquée ou non. `decision.used` dit
    /// la paire sur laquelle le tour part **réellement** (`None` : inconnue —
    /// décision consultative, ou session encore sur la sentinelle).
    async fn record(&self, _decision: &Decision) {}

    /// Le tour décidé par `decision_id` s'est terminé sur `stop`. `code` est le
    /// code de panne du transport, s'il y en a un.
    ///
    /// `served` est la paire qui a **réellement** servi le tour : `None` quand le
    /// moteur n'a jamais été joint (connexion refusée, délai d'ouverture, annulation
    /// avant toute réponse) — aucun bras ne doit être nourri. Une décision
    /// consultative (transport qui n'honore pas la paire,
    /// [`Transport::honors_route`](crate::Transport::honors_route) faux) n'arrive
    /// jamais ici : elle n'ouvre aucun tour, donc aucune fin.
    ///
    /// # Contrat
    ///
    /// - `record` et `settle` sont des appels **au mieux** : l'un et l'autre sont
    ///   bornés dans le temps par le harnais ([`crate::ROUTER_TIMEOUT_MS`]) et peuvent
    ///   être abandonnés (future lâchée, page suspendue).
    /// - `settle` peut donc arriver pour un identifiant jamais `record`é : le
    ///   traiter comme un no-op, jamais comme une erreur.
    /// - une décision `record`ée appliquée dont la fin ne vient jamais est un
    ///   tour abandonné : à ne pas nourrir. Le routeur le dit lui-même au tour
    ///   suivant par un `settle` avec [`Stop::Interrupted`] (jamais une récompense).
    /// - un `settle` coupé (borne, future lâchée) laisse la décision ouverte : le
    ///   routeur la redit ensuite en [`Stop::Interrupted`], `served = None`, par une
    ///   réparation à part ([`TurnRouter::repair`]), jusqu'à [`REPAIR_ATTEMPTS`]
    ///   fois ; au-delà, elle est lâchée. L'hôte garde **la première** fin reçue
    ///   pour un identifiant et ignore les suivantes : c'est la seule façon de
    ///   recevoir deux fois la même décision.
    /// - `record` reçoit `applied` et `used` tels qu'ils sont : une décision
    ///   consultative (transport qui n'honore pas la paire) arrive `applied: false`,
    ///   `used: None`.
    async fn settle(
        &self,
        _decision_id: u128,
        _served: Option<&Pick>,
        _stop: &Stop,
        _code: Option<&str>,
    ) {
    }
}

/// Aucun apprentissage : pas de bras, rien d'écrit.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLearning;

#[async_trait(?Send)]
impl Learning for NoLearning {}

/// Le hasard, injecté : l'identifiant de chaque décision, donc sa graine. Une
/// page le tire de `crypto.getRandomValues`, un serveur d'un UUID, un test
/// d'une suite.
pub trait Seeds {
    fn next_id(&self) -> u128;
}

/// Une suite d'identifiants : `base`, `base + 1`, … Déterministe : c'est le
/// hasard des tests, et celui d'un rejeu.
#[derive(Debug, Default)]
pub struct SequenceSeeds(Cell<u128>);

impl SequenceSeeds {
    pub fn from(base: u128) -> Self {
        Self(Cell::new(base))
    }
}

impl Seeds for SequenceSeeds {
    fn next_id(&self) -> u128 {
        let id = self.0.get();
        self.0.set(id.wrapping_add(1));
        id
    }
}

/// Ce que le menu d'une conversation demande.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    /// Auto : le routeur choisit tout (`full`).
    Auto,
    /// Des modèles cochés : un seul = strict (`primary`), deux ou plus = `mixed`.
    Ticked(Vec<PoolEntry>),
}

/// Un menu sans aucun modèle coché et sans Auto : il ne dit rien.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyPool;

impl std::fmt::Display for EmptyPool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("le pool de routage est vide : coche un modèle, ou choisis Auto")
    }
}

impl std::error::Error for EmptyPool {}

/// Pourquoi un tour part sur la paire qu'il a.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RouteWhy {
    /// Le routeur a choisi une autre paire, et elle est appliquée.
    Routed,
    /// Le routeur a décidé, et la paire en vigueur reste.
    Kept,
    /// Un seul candidat éligible : on part dessus.
    SingleCandidate,
    /// Aucun candidat éligible : repli sur la paire en vigueur.
    NoCandidate,
    /// La décision est notée, pas appliquée (étape `shadow` / `advisory`).
    RecordedOnly,
    /// Le modèle est imposé (strict) : rien n'est décidé.
    Pinned,
    /// Le modèle a été changé à la main : le routeur laisse la conversation.
    Manual,
    /// La conversation n'a rien confié au routeur.
    NotRouted,
    /// Un tirage d'exploration ne change jamais de fournisseur.
    ExplorationNeverMoves,
    /// L'écart est sous la marge d'hystérésis : le fournisseur reste.
    UnderMargin,
    /// La personne a réglé la conversation pendant que la décision attendait un
    /// port : menu ([`TurnRouter::reroute`] réussi), retour aux réglages
    /// ([`TurnRouter::follow_settings`]), modèle à la main ([`TurnRouter::set_by_hand`])
    /// ou liste Auto ([`TurnRouter::set_auto_models`]). Son réglage gagne : la
    /// décision est jetée, le tour part sur la paire en vigueur après ce réglage, et
    /// le nouveau réglage décide dès le message suivant (le routeur ne redécide pas
    /// dans le même tour).
    Superseded,
    /// Un port de l'hôte (candidats, apprentissage) n'a pas répondu à temps :
    /// repli sur la paire en vigueur.
    TimedOut,
}

impl RouteWhy {
    /// Code stable, pour les journaux et l'interface.
    pub fn code(self) -> &'static str {
        match self {
            Self::Routed => "routed",
            Self::Kept => "kept",
            Self::SingleCandidate => "single_candidate",
            Self::NoCandidate => "no_candidate",
            Self::RecordedOnly => "recorded_only",
            Self::Pinned => "pinned",
            Self::Manual => "manual",
            Self::NotRouted => "not_routed",
            Self::ExplorationNeverMoves => "exploration_never_moves",
            Self::UnderMargin => "under_margin",
            Self::Superseded => "superseded",
            Self::TimedOut => "timed_out",
        }
    }
}

/// Ce que le routeur rend pour un tour.
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    /// La paire sur laquelle le tour part. Toujours là : jamais « aucun candidat ».
    pub route: Route,
    /// La paire a changé par rapport au tour précédent.
    pub changed: bool,
    pub why: RouteWhy,
    /// La décision prise, quand il y en a eu une (appliquée ou non).
    pub decision: Option<Decision>,
}

/// Ce qu'un tour apporte au routeur.
#[derive(Debug, Clone, Copy)]
pub struct TurnInput<'a> {
    /// Le texte du message qui ouvre le tour.
    pub text: &'a str,
    /// Le message porte des images.
    pub has_images: bool,
    /// Ce que l'appelant sait du contexte.
    pub hints: ContextHints,
    /// Le transport honore-t-il la paire décidée
    /// ([`Transport::honors_route`](crate::Transport::honors_route)) ? Faux : la
    /// décision est consultative — enregistrée `applied: false`, `used: None`.
    pub honored: bool,
}

/// Ce qui ouvre un [`TurnRouter`].
pub struct RouterSpec {
    pub pool: Rc<dyn PoolSource>,
    pub learning: Rc<dyn Learning>,
    pub seeds: Rc<dyn Seeds>,
    /// Les réglages (mode, étape, poids). Le mode et l'étape en vigueur pour la
    /// conversation en sont dérivés selon ce qu'elle a demandé.
    pub settings: RoutingSettings,
    /// L'instance de la session.
    pub provider_id: String,
    /// Le modèle de la session. Peut être la sentinelle `auto`.
    pub model: String,
    /// Ce que la conversation a demandé ; `None` : elle suit les réglages.
    pub mode: Option<ConversationRoutingMode>,
    /// Les modèles cochés (deux ou plus : le pool).
    pub routing_pool: Vec<PoolEntry>,
    /// Les modèles que la PERSONNE laisse à Auto ; vide : tous.
    pub auto_models: Vec<PoolEntry>,
    /// Le modèle est imposé (nommé par la demande) : jamais remplacé.
    pub explicit_model: bool,
    /// Le fournisseur est imposé : la conversation n'en change jamais toute seule.
    pub provider_imposed: bool,
    /// Les tours appellent des outils (PO : toujours vrai pour une conversation).
    pub needs_tools: bool,
    /// Qui parle.
    pub role: Role,
}

impl RouterSpec {
    /// Une conversation qui n'a rien demandé, sur la paire donnée, sans apprentissage.
    pub fn new(
        pool: Rc<dyn PoolSource>,
        seeds: Rc<dyn Seeds>,
        provider_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            pool,
            learning: Rc::new(NoLearning),
            seeds,
            settings: RoutingSettings::default(),
            provider_id: provider_id.into(),
            model: model.into(),
            mode: None,
            routing_pool: Vec::new(),
            auto_models: Vec::new(),
            explicit_model: false,
            provider_imposed: false,
            needs_tools: true,
            role: Role::Pilot,
        }
    }

    /// La conversation est en Auto (`full`).
    pub fn auto(mut self) -> Self {
        self.mode = Some(ConversationRoutingMode::Full);
        self.routing_pool.clear();
        self.explicit_model = false;
        self
    }

    /// La conversation coche ces modèles : un seul = strict, deux ou plus = `mixed`.
    pub fn ticked(mut self, pool: Vec<PoolEntry>) -> Self {
        match pool.as_slice() {
            [] => {}
            [only] => {
                self.mode = Some(ConversationRoutingMode::Primary);
                self.provider_id = only.provider.clone();
                self.model = only.model.clone();
                self.explicit_model = true;
                self.routing_pool.clear();
            }
            _ => {
                self.mode = Some(ConversationRoutingMode::Mixed);
                self.routing_pool = pool;
                self.explicit_model = false;
            }
        }
        self
    }

    pub fn settings(mut self, settings: RoutingSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn learning(mut self, learning: Rc<dyn Learning>) -> Self {
        self.learning = learning;
        self
    }

    pub fn auto_models(mut self, list: Vec<PoolEntry>) -> Self {
        self.auto_models = list;
        self
    }
}

/// Ce que la conversation a demandé de son routage, tel que le routeur le tient.
#[derive(Debug, Clone)]
struct Choice {
    /// Les réglages, avec le mode et l'étape EN VIGUEUR pour cette conversation.
    routing: RoutingSettings,
    /// L'étape des réglages : celle où retomber quand la personne reprend un modèle.
    settings_stage: LearningStage,
    /// Le mode des réglages : celui où retomber de même.
    settings_mode: ProviderRoutingMode,
    explicit_model: bool,
    provider_imposed: bool,
    /// Le pool de la conversation (deux modèles ou plus), sinon `None`.
    pool: Option<Vec<PoolEntry>>,
    /// La liste de la personne, sinon `None` (tous les modèles).
    auto_models: Option<Vec<PoolEntry>>,
    /// Ce que le menu a dit, pour le relire.
    asked: Option<ConversationRoutingMode>,
}

/// Une décision appliquée, ouverte jusqu'à la fin de son tour.
#[derive(Debug, Clone)]
struct Open {
    id: u128,
    pick: Pick,
    /// Ce que le tour a quitté, quand il a changé de paire : si le moteur n'est
    /// jamais joint, on y revient ([`TurnRouter::settle`]).
    before: Option<Before>,
    /// La génération des réglages de la personne au moment du changement : un
    /// réglage fait depuis (même sur la même paire) n'est jamais annulé par le retour.
    ///
    /// Tout réglage compte, y compris ceux qui ne touchent pas la paire
    /// ([`TurnRouter::set_auto_models`], [`TurnRouter::follow_settings`]) : une liste
    /// Auto modifiée pendant le tour laisse alors en vigueur une paire jamais
    /// atteinte. C'est voulu, et cohérent avec [`RouteWhy::Superseded`] dans
    /// [`TurnRouter::route`] : après un réglage de la personne, le routeur ne réécrit
    /// plus rien de lui-même, et le message suivant redécide sous ce réglage.
    generation: u64,
}

/// L'état servi d'avant un changement de paire.
#[derive(Debug, Clone)]
struct Before {
    provider: String,
    model: String,
    last_change_turn: Option<u32>,
    last_change_class: Option<String>,
    last_move_turn: Option<u32>,
}

/// Réparations tentées pour une décision abandonnée ([`TurnRouter::repair`])
/// avant qu'elle soit lâchée : un apprentissage qui pend durablement ne coûte
/// qu'autant de messages, jamais toute la conversation.
pub const REPAIR_ATTEMPTS: u32 = 3;
/// Décisions abandonnées retenues au plus, en attente de réparation.
const ABANDONED_CAP: usize = 8;

struct State {
    choice: Choice,
    provider: String,
    model: String,
    /// La personne a changé le modèle à la main : plus de changement automatique.
    manual: bool,
    next_turn: u32,
    last_change_turn: Option<u32>,
    /// Classe du tour dont la décision a fait le dernier changement : le tour
    /// suivant de la MÊME classe est décidé sans exploration.
    last_change_class: Option<String>,
    last_move_turn: Option<u32>,
    /// La dernière décision appliquée, et la paire qui a servi : ce que
    /// [`TurnRouter::settle`] rend à l'apprentissage.
    open: Option<Open>,
    /// Les décisions dont le tour n'a jamais reçu de fin, avec le nombre de
    /// réparations déjà tentées ([`TurnRouter::repair`]). Bornée : au-delà de
    /// [`ABANDONED_CAP`], la plus ancienne est lâchée.
    abandoned: VecDeque<(u128, u32)>,
    /// Compteur de réglages de la personne (menu, main, liste) : une décision
    /// qui a attendu un port ne réécrit l'état que si rien n'a changé entre-temps.
    generation: u64,
}

/// Le routeur d'UNE conversation.
pub struct TurnRouter {
    pool: Rc<dyn PoolSource>,
    learning: Rc<dyn Learning>,
    seeds: Rc<dyn Seeds>,
    needs_tools: bool,
    role: Role,
    state: RefCell<State>,
}

/// Ce que le routeur a le droit de faire sur les tours de cette conversation —
/// la SEULE lecture des règles de routage.
#[derive(Debug, Clone, Copy)]
struct Rights {
    /// Le routeur choisit la paire des tours.
    routes: bool,
    /// Il peut changer de fournisseur.
    provider: bool,
    /// L'étape en vigueur applique les décisions (`auto`) ; sinon elles sont notées.
    auto: bool,
}

fn rights_of(choice: &Choice, manual: bool) -> Rights {
    let free = !choice.explicit_model && !manual;
    let full = choice.routing.mode == ProviderRoutingMode::Full;
    let routes = free && (full || choice.pool.is_some());
    Rights {
        routes,
        provider: routes && !choice.provider_imposed,
        auto: choice.routing.stage == LearningStage::Auto,
    }
}

/// Les réglages avec lesquels une décision est demandée : l'étape tombe d'`auto`
/// à `shadow` quand le choix ne peut pas être appliqué, pour que la décision
/// soit notée comme ce qu'elle est.
fn settings_for(routing: &RoutingSettings, apply: bool) -> RoutingSettings {
    let mut settings = routing.clone();
    if !apply && settings.stage == LearningStage::Auto {
        settings.stage = LearningStage::Shadow;
    }
    settings
}

/// La paire en vigueur, si c'en est une : jamais la sentinelle (`auto`, vide).
fn real_pair(provider: &str, model: &str) -> Option<Pick> {
    (!is_auto_model(model)).then(|| Pick::new(provider, model))
}

/// La paire est-elle sur la liste ? Le modèle se compare au sens de
/// [`same_model`] : `m` coché retient `m[1m]` (relecture de #254, constat 7).
fn listed(list: &[PoolEntry], facts: &Candidate) -> bool {
    list.iter()
        .any(|e| e.provider == facts.provider_id && same_model(&e.model, &facts.model))
}

/// Restreint `pool` à la `list` de la personne pour une décision de
/// `signature`, et rend le code de motif à y inscrire. Quand aucun modèle listé
/// ne passe les contraintes dures (disparu, non consenti, en panne, incapable
/// de servir le tour), `pool` reste entier et le code est
/// [`PERSON_LIST_UNAVAILABLE`] : Auto ne bloque jamais une conversation à cause
/// de ce réglage.
pub fn restrict_to_person_list(
    pool: &mut Vec<Candidate>,
    list: &[PoolEntry],
    signature: &TurnSignature,
) -> &'static str {
    let kept: Vec<Candidate> = pool.iter().filter(|f| listed(list, f)).cloned().collect();
    let usable = candidate::filter(Slot::Automatic, signature, &kept)
        .is_some_and(|filtered| !filtered.eligible.is_empty());
    if usable {
        *pool = kept;
        NARROWED_BY_PERSON
    } else {
        PERSON_LIST_UNAVAILABLE
    }
}

impl TurnRouter {
    pub fn new(spec: RouterSpec) -> Self {
        let pool = (spec.routing_pool.len() > 1).then_some(spec.routing_pool);
        let routes = spec.mode == Some(ConversationRoutingMode::Full) || pool.is_some();
        let mut routing = spec.settings.clone();
        if let Some(mode) = spec.mode {
            routing.mode = mode.pilot_mode();
        }
        routing.stage = conversation_stage(spec.settings.stage, routes);
        Self {
            pool: spec.pool,
            learning: spec.learning,
            seeds: spec.seeds,
            needs_tools: spec.needs_tools,
            role: spec.role,
            state: RefCell::new(State {
                choice: Choice {
                    routing,
                    settings_stage: spec.settings.stage,
                    settings_mode: spec.settings.mode,
                    explicit_model: spec.explicit_model,
                    provider_imposed: spec.provider_imposed,
                    pool,
                    auto_models: (!spec.auto_models.is_empty()).then_some(spec.auto_models),
                    asked: spec.mode,
                },
                provider: spec.provider_id,
                model: spec.model,
                manual: false,
                next_turn: 0,
                last_change_turn: None,
                last_change_class: None,
                last_move_turn: None,
                open: None,
                abandoned: VecDeque::new(),
                generation: 0,
            }),
        }
    }

    /// Le menu de la conversation change **en cours de route** : Auto, un
    /// modèle (strict), ou plusieurs (`mixed`). Ce qui avait été réglé à la main
    /// est oublié — c'est le nouveau choix à la main — et un fournisseur imposé
    /// à l'ouverture est relâché. Vaut dès le tour suivant.
    pub fn reroute(&self, menu: Menu) -> Result<ConversationRoutingMode, EmptyPool> {
        let mut state = self.state.borrow_mut();
        let mode = match menu {
            Menu::Auto => {
                state.choice.pool = None;
                state.choice.explicit_model = false;
                ConversationRoutingMode::Full
            }
            Menu::Ticked(pool) => match pool.as_slice() {
                [] => return Err(EmptyPool),
                [only] => {
                    // Strict : le modèle coché est imposé, et c'est lui qui est en vigueur.
                    state.provider = only.provider.clone();
                    state.model = only.model.clone();
                    state.choice.pool = None;
                    state.choice.explicit_model = true;
                    state.last_change_turn = None;
                    state.last_change_class = None;
                    ConversationRoutingMode::Primary
                }
                _ => {
                    state.choice.pool = Some(pool);
                    state.choice.explicit_model = false;
                    ConversationRoutingMode::Mixed
                }
            },
        };
        state.choice.asked = Some(mode);
        state.choice.routing.mode = mode.pilot_mode();
        state.choice.provider_imposed = false;
        let routes = mode == ConversationRoutingMode::Full || state.choice.pool.is_some();
        state.choice.routing.stage = conversation_stage(state.choice.settings_stage, routes);
        state.manual = false;
        // Seulement un réglage qui a abouti : un menu vide ne change rien, et ne doit
        // pas jeter une décision en cours.
        state.generation += 1;
        Ok(mode)
    }

    /// La conversation rend la main aux réglages (elle ne demande plus rien).
    pub fn follow_settings(&self) {
        let mut state = self.state.borrow_mut();
        state.generation += 1;
        state.choice.asked = None;
        state.choice.pool = None;
        state.choice.explicit_model = false;
        state.choice.routing.mode = state.choice.settings_mode;
        state.choice.routing.stage = state.choice.settings_stage;
        state.manual = false;
    }

    /// La personne a changé le modèle à la main : c'est lui qui est en vigueur,
    /// et le routeur laisse la conversation tranquille jusqu'au prochain
    /// [`TurnRouter::reroute`].
    pub fn set_by_hand(&self, provider_id: impl Into<String>, model: impl Into<String>) {
        let mut state = self.state.borrow_mut();
        state.generation += 1;
        state.provider = provider_id.into();
        state.model = model.into();
        state.manual = true;
        state.last_change_turn = None;
        state.last_change_class = None;
    }

    /// Remplace la liste « modèles qu'Auto peut utiliser » de la personne ; vide : tous.
    pub fn set_auto_models(&self, list: Vec<PoolEntry>) {
        let mut state = self.state.borrow_mut();
        state.generation += 1;
        state.choice.auto_models = (!list.is_empty()).then_some(list);
    }

    /// La paire en vigueur.
    pub fn in_force(&self) -> Pick {
        let state = self.state.borrow();
        Pick::new(&state.provider, &state.model)
    }

    /// Ce que le menu de la conversation a demandé, s'il a demandé quelque chose.
    pub fn mode(&self) -> Option<ConversationRoutingMode> {
        self.state.borrow().choice.asked
    }

    /// La conversation est-elle routée (le routeur choisit ses tours) ?
    pub fn routes(&self) -> bool {
        let state = self.state.borrow();
        rights_of(&state.choice, state.manual).routes
    }

    fn stay(&self, why: RouteWhy, decision: Option<Decision>) -> Routed {
        let state = self.state.borrow();
        Routed {
            route: Route::of(&state.provider, &state.model),
            changed: false,
            why,
            decision,
        }
    }

    /// La paire du tour qui commence.
    ///
    /// - la conversation n'a rien confié au routeur, un modèle imposé, un
    ///   modèle changé à la main : rien n'est demandé, la paire en vigueur part ;
    /// - sinon une décision est prise parmi les candidats de l'hôte (le pool de
    ///   la conversation s'il y en a un, sinon la liste de la personne en Auto),
    ///   et appliquée quand l'étape le permet. Elle n'est PAS enregistrée ici :
    ///   l'appelant passe ensuite `routed.decision` à [`TurnRouter::record`] (le
    ///   harnais le fait, borné), pour qu'un `record` qui pend ne touche pas au tour ;
    /// - **un seul** éligible : on part dessus ; **zéro** : repli sur la paire
    ///   en vigueur. Jamais d'erreur « aucun candidat » ;
    /// - le réglage de la personne gagne : si elle règle la conversation
    ///   pendant que la décision attend un port, la décision est jetée
    ///   ([`RouteWhy::Superseded`]).
    ///
    /// Cette future peut être lâchée à tout moment (le harnais la borne dans le
    /// temps et la coupe sur annulation, voir [`TurnRouter::fallback`]) : l'état
    /// n'est réécrit qu'en un seul bloc, sans attente au milieu.
    pub async fn route(&self, turn: TurnInput<'_>) -> Routed {
        // Un tour appliqué dont la fin n'est jamais venue (future lâchée, page
        // suspendue, `settle` coupé) est abandonné : rangé pour être dit à
        // l'apprentissage par [`TurnRouter::repair`] — jamais ICI, où une réparation
        // qui pend mangerait la borne de la décision et éteindrait Auto pour toute la
        // conversation (relecture de #254, constat 1).
        {
            let mut state = self.state.borrow_mut();
            if let Some(open) = state.open.take() {
                if state.abandoned.len() >= ABANDONED_CAP {
                    state.abandoned.pop_front();
                }
                state.abandoned.push_back((open.id, 0));
            }
        }
        let (choice, manual, current, index, held_turn, held_class, last_move, generation) = {
            let mut state = self.state.borrow_mut();
            let index = state.next_turn;
            state.next_turn = state.next_turn.saturating_add(1);
            (
                state.choice.clone(),
                state.manual,
                Pick::new(&state.provider, &state.model),
                index,
                state.last_change_turn,
                state.last_change_class.clone(),
                state.last_move_turn,
                state.generation,
            )
        };
        let rights = rights_of(&choice, manual);
        if !rights.routes {
            let why = if manual {
                RouteWhy::Manual
            } else if choice.explicit_model {
                RouteWhy::Pinned
            } else {
                RouteWhy::NotRouted
            };
            return self.stay(why, None);
        }

        let mut signature = TurnSignature::from_chat(turn.text, turn.has_images, turn.hints);
        signature.needs_tools = self.needs_tools;
        signature.role = self.role;
        let class = signature.arm_key();

        let mut pool = self.pool.pool().await;
        if let Some(entries) = &choice.pool {
            pool.retain(|facts| listed(entries, facts));
        }
        // La liste de la personne ne vaut que pour une conversation Auto sans
        // pool à elle : les modèles cochés d'une conversation gagnent toujours.
        let person = match (&choice.pool, &choice.auto_models) {
            (None, Some(list)) if choice.routing.mode == ProviderRoutingMode::Full => {
                Some(restrict_to_person_list(&mut pool, list, &signature))
            }
            _ => None,
        };

        // La paire en vigueur, telle que le POOL la nomme : `m` et `m[1m]` sont le même
        // modèle. Sans ça, la garde d'exploration et l'hystérésis ne reconnaissent
        // pas la paire en vigueur dans les candidats, et sautent.
        let current = pool
            .iter()
            .find(|f| f.provider_id == current.provider_id && same_model(&f.model, &current.model))
            .map(Candidate::pick)
            .unwrap_or(current);

        // Une session ouverte sur la sentinelle (`auto`, vide) n'a PAS de paire à
        // quitter : le premier choix n'est ni un changement de fournisseur, ni
        // soumis aux gardes qui le protègent (PO : la décision d'ouverture est
        // hors de ces gardes, `agent_hooks.rs` 777-826).
        let has_pair = !is_auto_model(&current.model);

        // Le tour porte une image et le pool SAIT que la paire en vigueur ne la
        // lit pas : rester n'est pas une option, les gardes anti-oscillation
        // cèdent à la contrainte dure. Une paire que le pool ne liste pas n'est
        // PAS aveugle : rien ne dit qu'elle ne lit pas l'image.
        let blind = has_pair
            && turn.has_images
            && pool.iter().any(|f| {
                f.provider_id == current.provider_id
                    && same_model(&f.model, &current.model)
                    && !f.capabilities.images
            });
        // Jamais deux changements de FOURNISSEUR de suite : le tour juste après
        // un changement est décidé parmi les modèles du fournisseur d'arrivée.
        let just_moved = last_move.is_some_and(|t| t.checked_add(1) == Some(index));
        let may_move = rights.provider && (blind || !just_moved);
        // Juste après un changement, un tour de la même classe est stable.
        let held = held_turn.is_some_and(|t| t.checked_add(1) == Some(index))
            && held_class.as_deref() == Some(class.as_str());

        let mut settings = settings_for(&choice.routing, rights.auto);
        // Un pool est routé comme `full` à l'intérieur, quel que soit le mode des réglages.
        if choice.pool.is_some() {
            settings.mode = ProviderRoutingMode::Full;
        }
        let mut request = DecideRequest::new(signature, settings, pool);
        request.current = has_pair.then(|| current.clone());
        request.turn_index = Some(index);
        request.narrowed.extend(person);
        if held {
            request.settings.exploration_epsilon = 0.0;
            request.steady = true;
            request.narrowed.push(SAME_CLASS_NO_EXPLORE);
        }
        if !may_move {
            request.restrict_provider = Some(current.provider_id.clone());
            request.narrowed.push(if rights.provider {
                AFTER_PROVIDER_MOVE
            } else {
                PROVIDER_IMPOSED
            });
        }

        let arms = self.learning.arms(&class).await;
        let hints = self.learning.hints();
        let id = self.seeds.next_id();
        let mut decision = decide(id, &request, &arms, &hints);

        // Ce qui suit ne compte aucune attente : l'état est réécrit d'un bloc.
        let eligible = decision
            .alternatives
            .iter()
            .filter(|a| a.rejected.is_none())
            .count();
        let current_eligible = decision
            .alternatives
            .iter()
            .any(|a| a.rejected.is_none() && same_pair(&a.pick, &current));
        // Un tirage d'exploration ne doit jamais éloigner le tour d'une paire en
        // vigueur qui reste éligible. Mais si la paire en vigueur est ELLE-MÊME
        // hors du pool coché / de la liste de la personne, la garde n'a rien à
        // protéger : le tour part sur le meilleur candidat non exploratoire, jamais
        // sur un modèle qu'on n'a pas le droit de choisir.
        if decision.explored && decision.applied && has_pair && !current_eligible && eligible > 0 {
            request.settings.exploration_epsilon = 0.0;
            decision = decide(id, &request, &arms, &hints);
        }
        let current_eligible = decision
            .alternatives
            .iter()
            .any(|a| a.rejected.is_none() && same_pair(&a.pick, &current));

        let (why, apply) = self.verdict(
            &decision,
            &current,
            has_pair,
            current_eligible,
            blind,
            rights,
            eligible,
        );
        {
            let mut state = self.state.borrow_mut();
            if state.generation != generation {
                // La personne a réglé la conversation pendant l'attente (menu, main,
                // liste Auto) : son réglage gagne, la décision n'écrit rien.
                decision.applied = false;
                decision.used = if turn.honored {
                    real_pair(&state.provider, &state.model)
                } else {
                    None
                };
                Routed {
                    route: Route::of(&state.provider, &state.model),
                    changed: false,
                    why: RouteWhy::Superseded,
                    decision: Some(decision),
                }
            } else if !turn.honored {
                // Consultatif : le transport sert SON modèle. Le choix est rendu comme
                // conseil (la requête le porte, l'événement le dit `advisory`), mais
                // l'état SERVI ne bouge pas : ni paire en vigueur, ni garde « pas deux
                // changements de fournisseur de suite », ni décision ouverte — rien de
                // tout cela n'a eu lieu.
                decision.applied = false;
                decision.used = None;
                let advice = decision.chosen.clone().filter(|_| apply);
                let route = match &advice {
                    Some(pick) => Route::of(&pick.provider_id, &pick.model),
                    None => Route::of(&state.provider, &state.model),
                };
                Routed {
                    route,
                    changed: false,
                    why,
                    decision: Some(decision),
                }
            } else {
                let mut changed = false;
                if let (true, Some(pick)) = (apply, decision.chosen.clone()) {
                    let mut before = None;
                    if !same_pair(&pick, &current) {
                        let moves = has_pair && pick.provider_id != current.provider_id;
                        before = Some(Before {
                            provider: state.provider.clone(),
                            model: state.model.clone(),
                            last_change_turn: state.last_change_turn,
                            last_change_class: state.last_change_class.clone(),
                            last_move_turn: state.last_move_turn,
                        });
                        state.provider = pick.provider_id.clone();
                        state.model = pick.model.clone();
                        state.last_change_turn = Some(index);
                        state.last_change_class = Some(class);
                        if moves {
                            state.last_move_turn = Some(index);
                        }
                        changed = true;
                    }
                    state.open = Some(Open {
                        id: decision.id,
                        pick,
                        before,
                        generation: state.generation,
                    });
                }
                // Ce que le tour fait vraiment : la décision le dit, pour que
                // l'apprentissage ne prenne jamais pour servie une paire qui ne
                // l'a pas été (exploration refusée, écart sous la marge, étape
                // consultative).
                let used = real_pair(&state.provider, &state.model);
                decision.applied = apply
                    && matches!((&decision.chosen, &used), (Some(c), Some(u)) if same_pair(c, u));
                decision.used = used;
                Routed {
                    route: Route::of(&state.provider, &state.model),
                    changed,
                    why,
                    decision: Some(decision),
                }
            }
        }
    }

    /// Enregistre une décision auprès de l'apprentissage ([`Learning::record`]).
    ///
    /// Séparé de [`TurnRouter::route`] exprès : le harnais l'appelle **après** que
    /// l'état a été réécrit et l'événement émis, borné et annulable. Un `record` qui
    /// pend ne peut ainsi rien changer au tour — il est au pire perdu, ce que le
    /// contrat de [`Learning`] admet.
    pub async fn record(&self, decision: &Decision) {
        self.learning.record(decision).await;
    }

    /// Ce que la décision devient : pourquoi, et si elle est appliquée.
    #[allow(clippy::too_many_arguments)]
    fn verdict(
        &self,
        decision: &Decision,
        current: &Pick,
        has_pair: bool,
        current_eligible: bool,
        blind: bool,
        rights: Rights,
        eligible: usize,
    ) -> (RouteWhy, bool) {
        let Some(pick) = &decision.chosen else {
            return (RouteWhy::NoCandidate, false);
        };
        if !rights.auto || !decision.applied {
            return (RouteWhy::RecordedOnly, false);
        }
        if same_pair(pick, current) {
            let why = if eligible == 1 {
                RouteWhy::SingleCandidate
            } else {
                RouteWhy::Kept
            };
            return (why, true);
        }
        let moves = has_pair && pick.provider_id != current.provider_id;
        if moves && current_eligible {
            if decision.explored && !blind {
                return (RouteWhy::ExplorationNeverMoves, false);
            }
            let score_of = |wanted: &Pick| {
                decision
                    .alternatives
                    .iter()
                    .find(|a| same_pair(&a.pick, wanted) && a.rejected.is_none())
                    .and_then(|a| a.score)
            };
            if let (Some(best), Some(now)) = (decision.score, score_of(current))
                && best - now < HYSTERESIS_MARGIN
            {
                return (RouteWhy::UnderMargin, false);
            }
        }
        let why = if eligible == 1 {
            RouteWhy::SingleCandidate
        } else {
            RouteWhy::Routed
        };
        (why, true)
    }

    /// Le repli quand le routeur ne peut pas répondre (un port a pendu, ou la
    /// future de [`TurnRouter::route`] a été coupée) : la paire en vigueur, sans
    /// décision. Les gardes du tour restent comptées (`route` a déjà avancé le
    /// compteur de tours).
    pub fn fallback(&self, why: RouteWhy) -> Routed {
        self.stay(why, None)
    }

    /// Le tour s'est terminé : sa fin est rendue à l'apprentissage, pour la
    /// décision appliquée qui l'a servi. Sans décision appliquée, rien n'est dit
    /// (un bras jamais servi n'est jamais nourri).
    ///
    /// `honored` : le transport honore la paire ET le moteur a répondu. Faux avec une
    /// décision ouverte (ce qui n'arrive qu'avec un transport qui honore : en
    /// consultatif, rien n'est ouvert) : le moteur n'a jamais été joint, `served` est
    /// `None`, et la paire **décidée mais jamais atteinte** est oubliée — l'état servi
    /// revient à ce qu'il était avant le changement (l'équivalent de `forget_change`
    /// chez PO), pour que l'hystérésis ne favorise pas une paire qui n'a pas servi.
    ///
    /// La décision reste ouverte tant que `settle` n'a pas rendu : un `settle` coupé
    /// (borne de temps, future lâchée) la laisse ouverte, et le message suivant la
    /// range parmi les abandonnées ([`TurnRouter::repair`]) — jamais une décision
    /// perdue en silence.
    pub async fn settle(&self, stop: &Stop, code: Option<&str>, honored: bool) {
        // Seuls l'identifiant, la paire et la génération sont lus : `before` n'est
        // jamais copié hors de l'état (il y est consommé une fois, juste dessous).
        let (id, pick) = {
            let mut state = self.state.borrow_mut();
            let Some((id, pick, generation)) = state
                .open
                .as_ref()
                .map(|o| (o.id, o.pick.clone(), o.generation))
            else {
                return;
            };
            // La paire décidée n'a jamais été atteinte : l'état servi revient à l'avant
            // du changement, AVANT d'attendre l'apprentissage — cette restauration ne
            // dépend pas de sa réponse, et un `settle` coupé la perdrait pour toujours
            // (re-relecture de #254, constat A). Une seule fois (`before` est consommé),
            // et jamais par-dessus un réglage de la personne fait depuis (génération).
            if !honored
                && let Some(before) = state.open.as_mut().and_then(|o| o.before.take())
                && state.generation == generation
            {
                state.provider = before.provider;
                state.model = before.model;
                state.last_change_turn = before.last_change_turn;
                state.last_change_class = before.last_change_class;
                state.last_move_turn = before.last_move_turn;
            }
            (id, pick)
        };
        let served = honored.then_some(&pick);
        self.learning.settle(id, served, stop, code).await;
        let mut state = self.state.borrow_mut();
        if state.open.as_ref().is_some_and(|o| o.id == id) {
            state.open = None;
        }
    }

    /// Des décisions abandonnées attendent-elles une réparation ?
    pub fn needs_repair(&self) -> bool {
        !self.state.borrow().abandoned.is_empty()
    }

    /// Répare UNE décision abandonnée : la dit à l'apprentissage en
    /// [`Stop::Interrupted`], sans paire servie, puis l'oublie. Rend `true` s'il en
    /// reste d'autres.
    ///
    /// Appelée par le harnais **après** la décision du message, dans sa propre
    /// borne : une réparation qui pend ne retarde ni ne coupe le routage. Chaque
    /// appel compte une tentative ; une décision dont [`REPAIR_ATTEMPTS`] tentatives
    /// ont été coupées est lâchée **à l'appel suivant**, sans nouvel appel à
    /// l'apprentissage (donc au 4e message : il ne saura jamais sa fin — il est en
    /// panne, et Auto continue sans lui).
    pub async fn repair(&self) -> bool {
        let next = {
            let mut state = self.state.borrow_mut();
            loop {
                let Some(&(id, attempts)) = state.abandoned.front() else {
                    break None;
                };
                if attempts >= REPAIR_ATTEMPTS {
                    state.abandoned.pop_front();
                    continue;
                }
                if let Some(entry) = state.abandoned.front_mut() {
                    entry.1 += 1;
                }
                break Some(id);
            }
        };
        let Some(id) = next else {
            return false;
        };
        self.learning
            .settle(id, None, &Stop::Interrupted, None)
            .await;
        let mut state = self.state.borrow_mut();
        state.abandoned.retain(|(open, _)| *open != id);
        !state.abandoned.is_empty()
    }
}

/// Un pool en mémoire : ce que l'hôte sait, posé à la main.
#[derive(Debug, Default)]
pub struct MemoryPool(RefCell<Vec<Candidate>>);

impl MemoryPool {
    pub fn new(pool: Vec<Candidate>) -> Self {
        Self(RefCell::new(pool))
    }

    /// Remplace le pool : c'est ainsi qu'un test fait tomber un fournisseur.
    pub fn set(&self, pool: Vec<Candidate>) {
        *self.0.borrow_mut() = pool;
    }
}

#[async_trait(?Send)]
impl PoolSource for MemoryPool {
    async fn pool(&self) -> Vec<Candidate> {
        self.0.borrow().clone()
    }
}

/// Une fin de tour, telle que [`Learning::settle`] la reçoit.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Settled {
    /// La décision appliquée qui a servi le tour.
    pub decision_id: u128,
    /// La paire qui a servi ; `None` : décision consultative.
    pub served: Option<Pick>,
    /// Comment le tour s'est arrêté.
    pub stop: Stop,
    /// Le code de panne du transport, s'il y en a un.
    pub code: Option<String>,
}

/// Un apprentissage en mémoire : il rend les bras qu'on lui a posés et garde
/// ce qu'on lui dit. Il n'apprend rien — c'est le témoin des tests, et le
/// gabarit du lot suivant.
#[derive(Debug, Default)]
pub struct MemoryLearning {
    arms: RefCell<Vec<ArmStats>>,
    hints: RefCell<PriorHints>,
    decisions: RefCell<Vec<Decision>>,
    settled: RefCell<Vec<Settled>>,
    duplicates: Cell<u32>,
}

impl MemoryLearning {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_arms(&self, arms: Vec<ArmStats>) {
        *self.arms.borrow_mut() = arms;
    }

    pub fn set_hints(&self, hints: PriorHints) {
        *self.hints.borrow_mut() = hints;
    }

    /// Les décisions reçues, dans l'ordre.
    pub fn decisions(&self) -> Vec<Decision> {
        self.decisions.borrow().clone()
    }

    /// Les fins de tour reçues, dans l'ordre.
    pub fn settled(&self) -> Vec<Settled> {
        self.settled.borrow().clone()
    }

    /// Les fins reçues pour un identifiant qui en avait déjà une : ignorées.
    pub fn duplicates(&self) -> u32 {
        self.duplicates.get()
    }
}

#[async_trait(?Send)]
impl Learning for MemoryLearning {
    async fn arms(&self, class: &str) -> Vec<ArmStats> {
        self.arms
            .borrow()
            .iter()
            .filter(|arm| arm.key.class == class)
            .cloned()
            .collect()
    }

    fn hints(&self) -> PriorHints {
        self.hints.borrow().clone()
    }

    async fn record(&self, decision: &Decision) {
        self.decisions.borrow_mut().push(decision.clone());
    }

    async fn settle(
        &self,
        decision_id: u128,
        served: Option<&Pick>,
        stop: &Stop,
        code: Option<&str>,
    ) {
        // Le contrat de `Learning::settle` : la PREMIÈRE fin reçue pour un identifiant
        // fait foi, les suivantes sont ignorées (et comptées, pour que les tests voient
        // un doublon).
        let mut settled = self.settled.borrow_mut();
        if settled.iter().any(|s| s.decision_id == decision_id) {
            self.duplicates.set(self.duplicates.get() + 1);
            return;
        }
        settled.push(Settled {
            decision_id,
            served: served.cloned(),
            stop: stop.clone(),
            code: code.map(str::to_owned),
        });
    }
}
