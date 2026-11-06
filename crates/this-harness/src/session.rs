//! La boucle elle-même : un run, des tours, une file, et une raison d'arrêt.
//!
//! ## Ce que ce crate reprend de Project Orchestrator
//!
//! PO tient une session d'agent derrière une poignée (`AgentSessionHandle`) :
//! `send_message`, `queue_message`, `interrupt`, `answer_permission`, et un
//! flux d'événements typés que plusieurs clients peuvent lire en même temps.
//! Deux idées en sont reprises telles quelles, parce qu'elles manquaient ici et
//! que c'est précisément ce qui rendait l'Atelier inutilisable :
//!
//! 1. **Un message tenu n'est pas un message perdu.** Pendant qu'un tour
//!    tourne, l'utilisateur peut écrire : le message entre dans la file et part
//!    à la frontière du tour suivant. La discussion est bidirectionnelle.
//! 2. **Un tour qui ne finit pas s'abandonne.** `interrupt` arrête le run en
//!    cours et le dit (`Stop::Cancelled`) au lieu de laisser l'interface
//!    attendre une promesse qui ne se résoudra jamais.
//!
//! Ce qui n'est **pas** repris : la base, le graphe, les fournisseurs, les
//! permissions, les sous-agents. Ce crate ne fait aucune entrée/sortie.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::Poll;

use async_trait::async_trait;

use crate::event::{Event, HintKind, Outcome, Pending, Snapshot};
use crate::ports::{Cancel, Delta, Toolbox, Transport};
use crate::wire::{Code, Failure, Finish, Message, Reply, Request, ToolCall};
use this_agent::evidence::Verdict;
use this_agent::{Decision, DefaultPolicy, Goal, Policy, Run, Step, Stop};

/// L'état partagé. `Rc<RefCell<_>>` sans condition : la boucle est sur un seul
/// fil, et un `Arc<Mutex<_>>` ne rendrait pas le crate plus utilisable sur
/// wasm — il rendrait seulement le code plus lourd partout.
pub type Shared<T> = Rc<RefCell<T>>;

type Sink = Box<dyn FnMut(Event)>;

/// Combien de temps un transport annulé a pour rendre la main avant que le
/// tour soit abandonné (la grâce de PO, `INTERRUPT_GRACE_DEFAULT`).
pub const INTERRUPT_GRACE_MS: u64 = 10_000;

/// Le silence toléré par défaut sur le flux d'un tour.
pub const SILENCE_MS: u64 = 120_000;

/// Relances accordées à un tour qui a échoué sur une panne passagère.
pub const MAX_RETRIES: u32 = 3;

/// Première attente avant une relance ; elle double à chaque tentative.
pub const RETRY_BACKOFF_MS: u64 = 1_000;

/// Le pas du chien de garde : le drapeau d'annulation est un simple `Cell`,
/// il se lit, il ne prévient pas.
const WATCH_TICK_MS: u64 = 1_000;

/// Une attente rendue par une [`Clock`].
pub type Sleep = Pin<Box<dyn Future<Output = ()>>>;

/// L'heure et les minuteurs de la boucle.
///
/// Le crate n'apporte aucun exécuteur : attendre « 10 s » demande un minuteur
/// que seul l'hôte connaît (`setTimeout` dans une page, un fil ou tokio dans
/// un binaire). Injectable pour les tests, qui font avancer le temps à la main.
pub trait Clock {
    /// Maintenant, en millisecondes.
    fn now_ms(&self) -> f64;
    /// Une attente de `ms`, ou `None` si ce target ne sait pas attendre : les
    /// chiens de garde sont alors éteints et les relances partent sans délai.
    fn sleep(&self, ms: u64) -> Option<Sleep>;
}

/// Un contexte recalculé avant **chaque** appel au modèle, jamais conservé.
///
/// Le brief est figé au montage du harnais ; une mémoire qui dépend de ce que
/// l'utilisateur vient de demander ne peut pas y vivre. Ce hook rend un message
/// système éphémère : la boucle l'insère dans la requête du tour, juste avant
/// le dernier message utilisateur, et ne l'écrit **jamais** dans le transcript
/// (il serait sinon renvoyé, périmé, à tous les tours suivants, et sauvegardé).
///
/// `?Send`, comme [`Transport`] : l'hôte qui le lit (une page wasm) n'est pas
/// `Send`. `None` (ou un texte vide) : la requête part telle quelle.
#[async_trait(?Send)]
pub trait TurnContext {
    /// Le contexte de ce tour, d'après la conversation telle qu'elle est.
    async fn before_turn(&self, transcript: &[Message]) -> Option<String>;
}

/// L'horloge du target. En natif, elle attend sur un fil ; en wasm, le crate
/// n'a pas de minuteur (pas de `wasm-bindgen-futures`) : l'hôte en injecte un
/// avec [`Harness::with_clock`] pour armer les chiens de garde.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> f64 {
        crate::now_ms()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn sleep(&self, ms: u64) -> Option<Sleep> {
        Some(Box::pin(ThreadSleep { ms, shared: None }))
    }

    #[cfg(target_arch = "wasm32")]
    fn sleep(&self, _ms: u64) -> Option<Sleep> {
        None
    }
}

/// Une attente sans exécuteur : un fil dort, puis réveille la tâche.
#[cfg(not(target_arch = "wasm32"))]
type Wake = std::sync::Arc<std::sync::Mutex<(bool, Option<std::task::Waker>)>>;

#[cfg(not(target_arch = "wasm32"))]
struct ThreadSleep {
    ms: u64,
    shared: Option<Wake>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Future for ThreadSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<()> {
        let ms = self.ms;
        let shared = self
            .shared
            .get_or_insert_with(|| {
                let shared: Wake = Default::default();
                let alarm = shared.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    let mut state = alarm.lock().unwrap_or_else(|e| e.into_inner());
                    state.0 = true;
                    if let Some(waker) = state.1.take() {
                        waker.wake();
                    }
                });
                shared
            })
            .clone();
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        if state.0 {
            Poll::Ready(())
        } else {
            state.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// Pourquoi le chien de garde a coupé le flux.
enum Watch {
    /// Aucun événement pendant ce nombre de millisecondes.
    Silent(u64),
    /// Annulé, et le transport n'a pas rendu la main dans la grâce.
    Abandoned(u64),
}

/// Veille sur un flux : rend dès que le silence dépasse `silence_ms`, ou que
/// l'annulation dure plus que `grace_ms` sans que le transport ait rendu.
async fn watch(
    clock: Rc<dyn Clock>,
    activity: Rc<Cell<f64>>,
    cancel: Cancel,
    silence_ms: Option<u64>,
    grace_ms: u64,
) -> Watch {
    let mut cancelled_at: Option<f64> = None;
    loop {
        let now = clock.now_ms();
        let left = if cancel.is_cancelled() {
            let since = *cancelled_at.get_or_insert(now);
            let waited = (now - since).max(0.0) as u64;
            if waited >= grace_ms {
                return Watch::Abandoned(grace_ms);
            }
            grace_ms - waited
        } else if let Some(limit) = silence_ms {
            let idle = (now - activity.get()).max(0.0) as u64;
            if idle >= limit {
                return Watch::Silent(limit);
            }
            limit - idle
        } else {
            WATCH_TICK_MS
        };
        match clock.sleep(left.min(WATCH_TICK_MS)) {
            Some(sleep) => sleep.await,
            // Pas de minuteur : la garde ne peut rien affirmer, elle se tait.
            None => return std::future::pending().await,
        }
    }
}

/// Le premier des deux qui rend. Le flux est sondé d'abord : un transport qui
/// répond tout de suite n'arme jamais la garde.
async fn race<T>(
    work: impl Future<Output = T>,
    guard: impl Future<Output = Watch>,
) -> Result<T, Watch> {
    let mut work = std::pin::pin!(work);
    let mut guard = std::pin::pin!(guard);
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(value) = work.as_mut().poll(cx) {
            return Poll::Ready(Ok(value));
        }
        if let Poll::Ready(why) = guard.as_mut().poll(cx) {
            return Poll::Ready(Err(why));
        }
        Poll::Pending
    })
    .await
}

/// Une panne passagère : la limite de débit (429) ou un délai dépassé.
fn transient(failure: &Failure) -> bool {
    failure.code == Code::Timeout || failure.status == Some(429)
}

/// Les bornes d'un run : ce qui arrête une boucle qui n'a pas de fin naturelle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Nombre maximal de tours du modèle pour un même objectif.
    pub max_tours: u32,
    /// Nombre de relances accordées après un refus ou un silence.
    pub max_relances: u32,
    /// Budget de temps du run, en millisecondes (`None` : pas de borne).
    pub budget_ms: Option<u64>,
    /// Silence toléré sur le flux d'un tour, en millisecondes : passé ce délai
    /// sans un seul événement, le tour est interrompu et le dit (`None` : pas
    /// de chien de garde).
    pub silence_ms: Option<u64>,
}

impl Limits {
    /// Les bornes d'une composition lue dans le graphe.
    ///
    /// Une composition porte ses bornes : changer `max_tours` est un acte de
    /// composition (versionné, empreinté), pas un réglage d'interface.
    pub fn from_doc(doc: &this_agent::graph::HarnessDoc) -> Self {
        Self {
            max_tours: doc.max_tours,
            max_relances: doc.max_relances,
            budget_ms: doc.budget_ms,
            silence_ms: Some(SILENCE_MS),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // 8 tours suffisaient à un modèle discipliné ; un module complet
            // (lire → écrire → compiler → vérifier → corriger) en demande plus,
            // et un budget atteint se lit comme une coupure de l'application.
            max_tours: 12,
            max_relances: 3,
            budget_ms: Some(15 * 60 * 1000),
            silence_ms: Some(SILENCE_MS),
        }
    }
}

/// Ce qu'un `send` a fait.
#[derive(Clone, Debug, PartialEq)]
pub enum Running {
    /// Le message a fait tourner l'agent jusqu'à son arrêt.
    Ran(Stop),
    /// Un run tournait déjà : le message est tenu pour le tour suivant.
    Queued,
    /// Rien à faire (message vide, file déjà vidée) : aucun tour n'a eu lieu,
    /// et aucun « terminé » n'est annoncé.
    Idle,
}

struct State {
    system: String,
    limits: Limits,
    transcript: Vec<Message>,
    pending: Vec<Pending>,
    next_id: u64,
    busy: bool,
    generation: u64,
    cancel: Cancel,
}

/// La sortie d'événements, dans sa propre cellule.
///
/// Séparée de l'état exprès : une interface qui, depuis le rappel d'événement,
/// relit la file d'attente ne doit pas se heurter à un emprunt déjà pris.
#[derive(Clone)]
struct Out(Rc<RefCell<Sink>>);

impl Out {
    fn emit(&self, event: Event) {
        (self.0.borrow_mut())(event);
    }
}

/// Le harnais. Clonable : toutes ses poignées se partagent le même état, donc
/// l'interface peut en garder une pendant qu'un run tourne.
#[derive(Clone)]
pub struct Harness {
    transport: Rc<dyn Transport>,
    toolbox: Rc<dyn Toolbox>,
    /// La politique du run. `DefaultPolicy` par défaut : les règles du crate ne
    /// changent pas parce qu'un hôte en branche une autre.
    policy: Rc<dyn Policy>,
    /// L'heure et les minuteurs : `SystemClock` par défaut.
    clock: Rc<dyn Clock>,
    /// Le contexte éphémère de chaque tour : aucun par défaut.
    turn_context: Option<Rc<dyn TurnContext>>,
    state: Shared<State>,
    out: Out,
}

impl Harness {
    pub fn new(
        system: impl Into<String>,
        limits: Limits,
        transport: Rc<dyn Transport>,
        toolbox: Rc<dyn Toolbox>,
        sink: impl FnMut(Event) + 'static,
    ) -> Self {
        Self {
            transport,
            toolbox,
            policy: Rc::new(DefaultPolicy),
            clock: Rc::new(SystemClock),
            turn_context: None,
            state: Rc::new(RefCell::new(State {
                system: system.into(),
                limits,
                transcript: Vec::new(),
                pending: Vec::new(),
                next_id: 1,
                busy: false,
                generation: 0,
                cancel: Cancel::new(),
            })),
            out: Out(Rc::new(RefCell::new(Box::new(sink)))),
        }
    }

    /// Compose un harness depuis sa définition dans le graphe.
    ///
    /// Le brief, les bornes et la politique viennent du document : la boucle ne
    /// change pas, c'est la **donnée** qui décide. Les emplacements `Tools` et
    /// `Guard` sont tenus par qui les a chargés — le harnais ne va pas chercher
    /// un module lui-même (il ne fait aucune entrée/sortie).
    pub fn from_doc(
        doc: &this_agent::graph::HarnessDoc,
        transport: Rc<dyn Transport>,
        toolbox: Rc<dyn Toolbox>,
        policy: Rc<dyn Policy>,
        sink: impl FnMut(Event) + 'static,
    ) -> Self {
        Self::new(
            doc.brief.clone(),
            Limits::from_doc(doc),
            transport,
            toolbox,
            sink,
        )
        .with_policy(policy)
    }

    /// Remplace la politique de décision du run.
    ///
    /// C'est le point d'entrée de la composition : une politique fournie par un
    /// module remplace « relance deux fois puis abandonne » par ses propres
    /// règles, sans toucher à la boucle, aux transports ni aux outils.
    ///
    /// ```ignore
    /// let harness = Harness::new(brief, limits, transport, toolbox, sink)
    ///     .with_policy(Rc::new(ModulePolicy::new(installed)));
    /// ```
    pub fn with_policy(mut self, policy: Rc<dyn Policy>) -> Self {
        self.policy = policy;
        self
    }

    /// Remplace l'horloge : un hôte wasm y branche son minuteur (`setTimeout`),
    /// un test y branche un temps qu'il fait avancer lui-même.
    pub fn with_clock(mut self, clock: Rc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Branche le contexte éphémère de chaque tour (voir [`TurnContext`]).
    ///
    /// Le hook est attendu avant chaque appel au modèle ; ce qu'il rend part
    /// dans la requête de ce tour seulement, jamais dans le transcript.
    pub fn with_turn_context(mut self, context: Rc<dyn TurnContext>) -> Self {
        self.turn_context = Some(context);
        self
    }

    /// La poignée de commande : c'est tout ce que l'interface doit garder.
    pub fn handle(&self) -> Handle {
        Handle {
            state: self.state.clone(),
            out: self.out.clone(),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.borrow_mut())
    }

    /// La conversation telle que le modèle la verra (pour la sauvegarder).
    pub fn transcript(&self) -> Vec<Message> {
        self.with(|s| s.transcript.clone())
    }

    /// Reprend une conversation déjà commencée.
    pub fn seed(&self, transcript: Vec<Message>) {
        self.with(|s| s.transcript = transcript);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.handle().snapshot()
    }

    /// Remet un état sauvegardé en place — la file d'attente comprise.
    pub fn restore(&self, snapshot: Snapshot) {
        self.handle().restore(snapshot);
    }

    /// Le message entre dans la conversation et l'agent travaille jusqu'à
    /// s'arrêter. Si un run tourne déjà, le message est tenu et la méthode
    /// rend tout de suite : c'est ce qui permet à l'interface d'écrire
    /// pendant que l'agent répond.
    pub async fn send(&self, text: impl Into<String>) -> Running {
        self.handle().enqueue(text);
        if !self.claim() {
            return Running::Queued;
        }
        match self.pump().await {
            Some(stop) => Running::Ran(stop),
            None => Running::Idle,
        }
    }

    /// Reprend les messages tenus d'une session précédente. Rend `None` si
    /// rien n'attend ou si un run tourne déjà.
    pub async fn resume(&self) -> Option<Stop> {
        if self.with(|s| s.pending.is_empty()) {
            return None;
        }
        if !self.claim() {
            return None;
        }
        self.pump().await
    }

    /// Prend le tour pour soi, ou dit que quelqu'un d'autre l'a pris.
    ///
    /// C'est le verrou qui empêche deux `send` concurrents de faire tourner
    /// deux boucles sur la même conversation — le défaut classique d'une
    /// interface qui n'attend pas la fin du tour précédent.
    fn claim(&self) -> bool {
        self.with(|s| {
            if s.busy {
                return false;
            }
            s.busy = true;
            s.generation += 1;
            true
        })
    }

    /// Fait tourner la file jusqu'à l'arrêt. `None` : aucun objectif n'a été
    /// ouvert (file vide) — rien n'a tourné, donc rien n'est « terminé ».
    async fn pump(&self) -> Option<Stop> {
        let clock = self.clock.clone();
        let started = clock.now_ms();
        let cancel = self.with(|s| s.cancel.clone());
        cancel.reset();
        let transport = self.transport.clone();
        let toolbox = self.toolbox.clone();

        let mut turns = 0u32;
        let mut stop: Option<Stop> = None;
        let mut said = String::new();
        let mut code: Option<String> = None;
        // Le verdict du run qui a posé `stop` : lu à la sortie de ses tours, avant
        // qu'un objectif suivant n'ouvre un autre run.
        let mut verdict: Option<Verdict> = None;

        'goals: loop {
            // 1. Le prochain message tenu ouvre un objectif. Sans message, il
            //    n'y a plus rien à faire : le run s'arrête là où il était.
            let Some(first) = self.take_pending() else {
                break 'goals;
            };
            self.push(Message::user(first.text.clone()));
            self.out.emit(Event::User {
                text: first.text.clone(),
            });

            if cancel.is_cancelled() {
                stop = Some(Stop::Cancelled);
                // Aucun run n'a commencé : aucune étape, rien de terminé.
                verdict = Some(this_agent::evidence::verdict(&[], false));
                break 'goals;
            }

            let limits = self.with(|s| s.limits.clone());
            let mut run = Run::new(Goal::new(first.text.clone()), limits.max_tours);
            run.max_relances = limits.max_relances;
            if let Some(budget) = limits.budget_ms {
                run.budget(budget);
            }

            'turns: loop {
                if cancel.is_cancelled() {
                    stop = Some(run.finish(Stop::Cancelled));
                    break 'turns;
                }

                self.out.emit(Event::Turn { index: turns });
                turns += 1;
                // 2. Un tour du modèle, en flux — sous la garde du silence et
                //    de la grâce d'annulation, avec des relances bornées.
                // Le contexte du tour est recalculé ici, à chaque tour : il
                // dépend de la conversation telle qu'elle est maintenant.
                let context = self.turn_context().await;
                let request = self.request(toolbox.as_ref(), context);
                let gathered = Rc::new(RefCell::new(String::new()));
                let mut tentative = 0u32;
                let issue: Result<Result<Reply, Failure>, Watch> = loop {
                    // Le dernier signe de vie du flux : un tour qui commence en
                    // est un, chaque morceau reçu en est un.
                    let activity = Rc::new(Cell::new(clock.now_ms()));
                    let delta: Delta = {
                        let gathered = gathered.clone();
                        let out = self.out.clone();
                        let cancel = cancel.clone();
                        let activity = activity.clone();
                        let clock = clock.clone();
                        Rc::new(move |text: &str| {
                            activity.set(clock.now_ms());
                            // Un flux annulé ne doit plus rien afficher : sinon le
                            // texte du tour précédent se mélange au suivant.
                            if cancel.is_cancelled() {
                                return;
                            }
                            gathered.borrow_mut().push_str(text);
                            out.emit(Event::Delta {
                                text: text.to_string(),
                            });
                        })
                    };
                    let guard = watch(
                        clock.clone(),
                        activity,
                        cancel.clone(),
                        limits.silence_ms,
                        INTERRUPT_GRACE_MS,
                    );
                    let issue = race(transport.stream(&request, &cancel, delta), guard).await;

                    // Une panne passagère se rejoue, mais seulement si le tour
                    // n'a rien montré et qu'aucun outil n'a tourné : rejouer
                    // après coup doublerait un texte déjà lu ou un effet déjà
                    // produit.
                    if let Ok(Err(failure)) = &issue
                        && transient(failure)
                        && !failure.is_aborted()
                        && !cancel.is_cancelled()
                        && gathered.borrow().is_empty()
                        && run.steps.is_empty()
                        && tentative < MAX_RETRIES
                    {
                        tentative += 1;
                        let wait = RETRY_BACKOFF_MS << (tentative - 1);
                        self.out.emit(Event::Hint {
                            kind: HintKind::Retry,
                            text: format!(
                                "Panne passagère du moteur ({}) : nouvelle tentative {tentative}/{MAX_RETRIES} dans {} s.",
                                failure.message,
                                wait / 1000
                            ),
                        });
                        if let Some(sleep) = clock.sleep(wait) {
                            sleep.await;
                        }
                        if cancel.is_cancelled() {
                            break Ok(Err(Failure::aborted()));
                        }
                        continue;
                    }
                    break issue;
                };

                let reply = match issue {
                    Ok(Ok(reply)) => reply,
                    Ok(Err(failure)) => {
                        if failure.is_aborted() || cancel.is_cancelled() {
                            stop = Some(run.finish(Stop::Cancelled));
                        } else {
                            code = Some(format!("{:?}", failure.code).to_lowercase());
                            stop = Some(run.finish(Stop::Failed(failure.message)));
                        }
                        break 'turns;
                    }
                    Err(why) => {
                        // Le flux est abandonné (son futur est lâché) : la
                        // session redevient libre, et elle dit pourquoi.
                        let (hint, panne) = match why {
                            Watch::Silent(ms) => (
                                format!(
                                    "Aucun événement du flux depuis {} s : le moteur ne répond plus, le tour est interrompu. Rien n'est perdu : le prochain message reprend la conversation.",
                                    ms / 1000
                                ),
                                Code::Timeout,
                            ),
                            Watch::Abandoned(ms) => (
                                format!(
                                    "Le transport n'a pas rendu la main {} s après l'annulation : le tour est abandonné et la session est libre.",
                                    ms / 1000
                                ),
                                Code::Aborted,
                            ),
                        };
                        self.out.emit(Event::Hint {
                            text: hint,
                            kind: HintKind::Interrupted,
                        });
                        let partial = gathered.borrow().clone();
                        if !partial.trim().is_empty() {
                            said = partial.clone();
                            self.push(Message::Assistant {
                                content: partial,
                                calls: Vec::new(),
                            });
                        }
                        code = Some(format!("{panne:?}").to_lowercase());
                        stop = Some(run.finish(Stop::Interrupted));
                        break 'turns;
                    }
                };

                let streamed = gathered.borrow().clone();
                let text = if reply.text.is_empty() {
                    streamed
                } else {
                    reply.text.clone()
                };
                if !text.trim().is_empty() {
                    said = text.clone();
                }
                self.out.emit(Event::Spoke { text: text.clone() });

                // Un tour qui s'arrête ici ne doit PAS laisser des appels
                // d'outils sans réponse dans le fil : le moteur refuse alors la
                // conversation suivante (« tool_calls without tool messages »),
                // et l'app paraît cassée pour de bon. On garde le texte, on
                // jette les appels qu'on ne jouera pas.
                let coupe = if cancel.is_cancelled() {
                    Some(Stop::Cancelled)
                } else if matches!(reply.finish, Finish::Length | Finish::Aborted) {
                    Some(Stop::Failed(if reply.finish == Finish::Aborted {
                        "flux interrompu par le transport".to_string()
                    } else {
                        "réponse tronquée : le moteur a atteint sa limite de tokens".to_string()
                    }))
                } else {
                    None
                };
                if let Some(motif) = coupe {
                    self.push(Message::Assistant {
                        content: text.clone(),
                        calls: Vec::new(),
                    });
                    stop = Some(run.finish(motif));
                    break 'turns;
                }

                self.push(Message::Assistant {
                    content: text.clone(),
                    calls: reply.calls.clone(),
                });

                // 3. Les outils demandés, un par un, avec leur observation.
                for call in &reply.calls {
                    if cancel.is_cancelled() {
                        // Une annulation au milieu d'un lot laisserait des
                        // appels sans réponse — un fil que le moteur refuse. On
                        // répond « annulé » à chacun, et le fil reste valide.
                        self.out.emit(Event::ToolResult {
                            id: call.id.clone(),
                            tool: call.name.clone(),
                            text: "annulé".to_string(),
                            refused: true,
                            failed: false,
                            failure: None,
                            unchanged: false,
                            before: String::new(),
                            after: String::new(),
                            ms: 0,
                        });
                        self.push(Message::tool(
                            call.id.clone(),
                            "annulé : l'utilisateur a interrompu le tour",
                        ));
                        continue;
                    }
                    let outcome = self.run_tool(toolbox.as_ref(), call, &mut run).await;
                    self.push(Message::tool(call.id.clone(), outcome.text.clone()));
                }

                let spoke = !text.trim().is_empty();
                let called = !reply.calls.is_empty();
                // Le motif se lit AVANT la décision : `after_turn` déplace la
                // frontière du tour, et le refus d'aujourd'hui ne serait plus
                // visible après. Sans ça, une action refusée était suivie d'un
                // « je m'arrête là » accepté comme une conclusion.
                let hint = relaunch_reason(&run, spoke, called);
                // Son genre se lit au même instant, pour la même raison.
                let hint_kind = relaunch_kind(&run);
                // Le récit du tour est confié à la décision : sans lui, « c'est
                // fait » après un refus n'est jamais confronté aux étapes.
                run.said(&text);

                match run.after_turn_with(self.policy.as_ref(), spoke, called) {
                    Decision::Continue => {
                        // Le rappel est consommé dans tous les cas : laissé dans
                        // le run, il serait périmé au tour suivant.
                        let rappel = run.take_rappel();
                        let hint = match (hint, rappel) {
                            (Some(hint), _) => Some((hint, hint_kind)),
                            (None, Some(rappel)) => Some((rappel, HintKind::Claim)),
                            (None, None) => None,
                        };
                        if let Some((hint, kind)) = hint {
                            self.out.emit(Event::Hint {
                                text: hint.clone(),
                                kind,
                            });
                            self.push(Message::user(hint));
                        }
                        continue 'turns;
                    }
                    Decision::Halt(reason) => {
                        stop = Some(run.finish(reason));
                        break 'turns;
                    }
                }
            }
            verdict = Some(run.verdict());

            // Un objectif atteint, mais des messages attendent : la discussion
            // continue, elle ne s'arrête pas parce que le modèle a conclu.
            if self.with(|s| s.pending.is_empty()) {
                break 'goals;
            }
        }

        // Aucun objectif ouvert : annoncer « terminé » serait un faux succès.
        // La session redevient libre, sans Stopped.
        let Some(stop) = stop else {
            self.with(|s| s.busy = false);
            return None;
        };
        let ms = (clock.now_ms() - started).max(0.0) as u64;
        self.with(|s| s.busy = false);
        self.out.emit(Event::Stopped {
            stop: stop.clone(),
            text: said,
            turns,
            ms,
            code,
            verdict,
        });
        Some(stop)
    }

    async fn run_tool(&self, toolbox: &dyn Toolbox, call: &ToolCall, run: &mut Run) -> Outcome {
        self.out.emit(Event::ToolUse {
            id: call.id.clone(),
            tool: call.name.clone(),
            input: call.arguments.clone(),
        });
        let started = self.clock.now_ms();
        let outcome = toolbox.call(call).await;
        let ms = (self.clock.now_ms() - started).max(0.0) as u64;

        self.out.emit(Event::ToolResult {
            id: call.id.clone(),
            tool: call.name.clone(),
            text: outcome.text.clone(),
            refused: outcome.refused.is_some(),
            failed: outcome.failed.is_some(),
            failure: outcome.failed.clone(),
            unchanged: outcome.unchanged(),
            before: outcome.before.clone(),
            after: outcome.after.clone(),
            ms,
        });

        run.record(step_of(&call.name, ms, &outcome));
        outcome
    }

    /// Ce que le hook de contexte rend pour ce tour. L'état n'est pas emprunté
    /// pendant l'attente : le transcript est copié d'abord.
    async fn turn_context(&self) -> Option<String> {
        let hook = self.turn_context.clone()?;
        let transcript = self.transcript();
        hook.before_turn(&transcript).await
    }

    /// La requête du tour : le transcript, plus le contexte éphémère s'il y en
    /// a un — inséré dans la copie, jamais dans le transcript lui-même.
    fn request(&self, toolbox: &dyn Toolbox, context: Option<String>) -> Request {
        let (system, messages) = self.with(|s| (s.system.clone(), s.transcript.clone()));
        Request::new(system, messages, toolbox.specs()).with_context(context)
    }

    fn push(&self, message: Message) {
        self.with(|s| s.transcript.push(message));
    }

    fn take_pending(&self) -> Option<Pending> {
        let first = self.with(|s| {
            if s.pending.is_empty() {
                None
            } else {
                Some(s.pending.remove(0))
            }
        });
        if first.is_some() {
            let left = self.with(|s| s.pending.clone());
            self.out.emit(Event::Pending { messages: left });
        }
        first
    }
}

/// Pourquoi la boucle relance au lieu de conclure.
///
/// `Run::after_turn` dit *qu'il faut* relancer, pas *pourquoi*. Un modèle
/// relancé sans motif répète la même action — c'est exactement ce qui faisait
/// tourner l'Atelier en rond. Le motif se lit donc dans le run.
fn relaunch_reason(run: &Run, spoke: bool, called: bool) -> Option<String> {
    if let Some(step) = run.derniere_refusee() {
        let motif = step.motif.clone().unwrap_or_else(|| step.resume.clone());
        return Some(format!(
            "La dernière tentative a été refusée ({motif}). Change d'approche, ou dis clairement ce qui manque."
        ));
    }
    if let Some(step) = run.derniere_en_echec() {
        let cause = step.failure.clone().unwrap_or_else(|| step.resume.clone());
        return Some(format!(
            "La dernière action a échoué ({cause}). Corrige la cause, ou dis clairement ce qui manque."
        ));
    }
    if run.dernier_sans_effet().is_some() {
        return Some(
            "La dernière action n'a rien changé à l'état : elle ne compte pas comme un progrès. Fais autrement, ou dis pourquoi c'est impossible."
                .to_string(),
        );
    }
    // Ne rien dire n'est un défaut que si rien n'a été fait : un tour qui
    // appelle un outil parle rarement, c'est normal.
    if !spoke && !called {
        return Some(
            "Tu n'as rien écrit et tu n'as appelé aucun outil. Soit tu continues vers l'objectif, soit tu dis explicitement que c'est terminé et ce qui reste."
                .to_string(),
        );
    }
    None
}

/// Le genre de la relance que [`relaunch_reason`] vient de rendre : un refus ou
/// un échec du tour la motive, sinon c'est une autre consigne (sans effet, tour
/// muet). Lu sur le run, pas sur le texte : la phrase peut changer.
fn relaunch_kind(run: &Run) -> HintKind {
    if run.derniere_refusee().is_some() || run.derniere_en_echec().is_some() {
        HintKind::Refused
    } else {
        HintKind::Other
    }
}

/// Le résumé d'une sortie d'outil gardé dans l'étape, en caractères.
const RESUME_CHARS: usize = 200;

/// L'étape qu'un appel d'outil laisse dans le run.
///
/// L'empreinte se prend sur la sortie COMPLÈTE, ici, avant la troncature du
/// résumé : hachée plus loin (le magasin n'a que `resume`), elle ne prouverait
/// que les [`RESUME_CHARS`] premiers caractères.
fn step_of(tool: &str, ms: u64, outcome: &Outcome) -> Step {
    let mut step = Step::tool(tool, ms)
        .state(outcome.before.clone(), outcome.after.clone())
        .resume(outcome.text.chars().take(RESUME_CHARS).collect::<String>())
        .out_hash(out_hash(&outcome.text));
    if let Some(motif) = &outcome.refused {
        step = step.refused(motif.clone());
    }
    if let Some(cause) = &outcome.failed {
        step = step.failed(cause.clone());
    }
    step
}

/// L'empreinte d'une sortie d'outil : SHA-256 en hexadécimal minuscule.
///
/// Même fonction que celle du magasin (`src/storage/harness_graph.rs`), pour
/// qu'un `_out_hash` ait un seul format quel que soit l'endroit où il est
/// calculé. `sha2` est du Rust pur : il compile en wasm32 sans `asm`.
pub fn out_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// La poignée que l'interface garde : elle commande sans attendre la fin d'un
/// tour.
#[derive(Clone)]
pub struct Handle {
    state: Shared<State>,
    out: Out,
}

impl Handle {
    fn emit_pending(&self) {
        let messages = self.state.borrow().pending.clone();
        self.out.emit(Event::Pending { messages });
    }

    /// Met un message en file. Rend son identifiant, pour pouvoir le retirer
    /// ou le modifier tant qu'il attend.
    pub fn enqueue(&self, text: impl Into<String>) -> u64 {
        let id = {
            let mut state = self.state.borrow_mut();
            let id = state.next_id;
            state.next_id += 1;
            let text = text.into();
            if !text.trim().is_empty() {
                state.pending.push(Pending {
                    id,
                    text,
                    at_ms: crate::now_ms(),
                });
            }
            id
        };
        self.emit_pending();
        id
    }

    pub fn pending(&self) -> Vec<Pending> {
        self.state.borrow().pending.clone()
    }

    pub fn drop_pending(&self, id: u64) -> bool {
        let removed = {
            let mut state = self.state.borrow_mut();
            let before = state.pending.len();
            state.pending.retain(|entry| entry.id != id);
            state.pending.len() != before
        };
        if removed {
            self.emit_pending();
        }
        removed
    }

    pub fn edit_pending(&self, id: u64, text: impl Into<String>) -> bool {
        let text = text.into();
        let edited = {
            let mut state = self.state.borrow_mut();
            match state.pending.iter_mut().find(|entry| entry.id == id) {
                Some(entry) => {
                    entry.text = text;
                    true
                }
                None => false,
            }
        };
        if edited {
            self.emit_pending();
        }
        edited
    }

    /// Remet un message en tête de file : c'est celui qui partira au tour
    /// suivant, quel que soit l'ordre dans lequel ils ont été tapés.
    pub fn bump(&self, id: u64) -> bool {
        let moved = {
            let mut state = self.state.borrow_mut();
            match state.pending.iter().position(|entry| entry.id == id) {
                Some(index) if index > 0 => {
                    let entry = state.pending.remove(index);
                    state.pending.insert(0, entry);
                    true
                }
                Some(_) => false,
                None => false,
            }
        };
        if moved {
            self.emit_pending();
        }
        moved
    }

    /// Arrête le run en cours. Il s'arrête pour de vrai : le drapeau est lu
    /// par le transport (qui peut couper la connexion) et par la boucle entre
    /// deux outils — pas seulement par l'interface, qui continuerait sinon à
    /// recevoir le texte d'une réponse qu'on croyait annulée.
    pub fn interrupt(&self) {
        self.state.borrow().cancel.cancel();
    }

    pub fn busy(&self) -> bool {
        self.state.borrow().busy
    }

    /// Le numéro du run en cours : une interface s'en sert pour ignorer les
    /// événements d'un run qu'elle a déjà remplacé.
    pub fn generation(&self) -> u64 {
        self.state.borrow().generation
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.borrow().cancel.is_cancelled()
    }

    pub fn transcript(&self) -> Vec<Message> {
        self.state.borrow().transcript.clone()
    }

    /// Remplace la conversation que le modèle verra : reprise d'une session
    /// sauvegardée, ou nouvelle conversation.
    pub fn seed(&self, transcript: Vec<Message>) {
        self.state.borrow_mut().transcript = transcript;
    }

    pub fn snapshot(&self) -> Snapshot {
        let (transcript, pending) = {
            let state = self.state.borrow();
            (state.transcript.clone(), state.pending.clone())
        };
        Snapshot {
            version: 1,
            transcript,
            pending,
        }
    }

    pub fn restore(&self, snapshot: Snapshot) {
        {
            let mut state = self.state.borrow_mut();
            state.transcript = snapshot.transcript;
            state.pending = snapshot.pending;
            state.next_id = state
                .pending
                .iter()
                .map(|entry| entry.id)
                .max()
                .unwrap_or(0)
                + 1;
        }
        self.emit_pending();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_hash_est_le_sha256_hex() {
        assert_eq!(
            out_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Deux sorties qui ne diffèrent qu'après le 200ᵉ caractère ont le même
    /// résumé, mais pas la même empreinte : c'est la sortie complète qui est hachée.
    #[test]
    fn l_empreinte_de_l_etape_porte_sur_la_sortie_complete() {
        let debut = "x".repeat(RESUME_CHARS);
        let a = step_of("lire", 1, &Outcome::ok(format!("{debut}A")));
        let b = step_of("lire", 1, &Outcome::ok(format!("{debut}B")));
        assert_eq!(a.resume, b.resume, "le résumé est tronqué");
        assert_eq!(a.out_hash, Some(out_hash(&format!("{debut}A"))));
        assert_ne!(a.out_hash, b.out_hash, "l'empreinte voit la fin");
    }

    #[test]
    fn l_etape_garde_refus_echec_et_empreintes() {
        let refus = step_of("ecrire", 2, &Outcome::refused("non", "même version"));
        assert!(refus.refused);
        assert_eq!(refus.motif.as_deref(), Some("même version"));
        let echec = step_of(
            "ecrire",
            2,
            &Outcome::failed("échec", "disque plein").observed("a", "a"),
        );
        assert_eq!(echec.failure.as_deref(), Some("disque plein"));
        assert_eq!((echec.before.as_str(), echec.after.as_str()), ("a", "a"));
    }
}
