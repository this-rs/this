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
use std::collections::VecDeque;
use std::rc::Rc;

use crate::event::{Event, Outcome, Pending, Snapshot};
use crate::ports::{Cancel, Delta, Toolbox, Transport};
use crate::wire::{Finish, ImagePart, Message, Request, ToolCall};
use this_agent::{Decision, DefaultPolicy, Goal, Policy, Run, Step, Stop};

/// L'état partagé. `Rc<RefCell<_>>` sans condition : la boucle est sur un seul
/// fil, et un `Arc<Mutex<_>>` ne rendrait pas le crate plus utilisable sur
/// wasm — il rendrait seulement le code plus lourd partout.
pub type Shared<T> = Rc<RefCell<T>>;

type Sink = Box<dyn FnMut(Event)>;

/// Les bornes d'un run : ce qui arrête une boucle qui n'a pas de fin naturelle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Nombre maximal de tours du modèle pour un même objectif.
    pub max_tours: u32,
    /// Nombre de relances accordées après un refus ou un silence.
    pub max_relances: u32,
    /// Budget de temps du run, en millisecondes (`None` : pas de borne).
    pub budget_ms: Option<u64>,
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
///
/// ## Ce que `emit` garantit
///
/// `emit` n'appelle jamais le puits en tenant l'emprunt de la cellule : chaque
/// événement est poussé dans une file, vidée un par un, drapeau « en train de
/// parler » posé. Un rappel qui réagit à un événement en émettant à son tour —
/// le geste naturel pour « renvoyer le message tenu » sur un `Stopped` — ne
/// panique plus : son émission atterrit dans la file, et c'est le vidage en
/// cours qui la livre. L'ordre est préservé (FIFO), aucun événement n'est
/// perdu, et le drapeau retombe même si le puits panique (il tue le run, pas
/// le harnais).
///
/// Ce que ça ne garantit PAS : un puits qui réagit à sa propre émission en
/// émettant sans condition ne s'arrête pas — la file grandit à l'infini. La
/// borne est la volonté du rappel, pas une limite du harnais.
#[derive(Clone)]
struct Out {
    sink: Rc<RefCell<Sink>>,
    /// Les événements à livrer, dans l'ordre. `emit` pousse toujours ici.
    queue: Rc<RefCell<VecDeque<Event>>>,
    /// « Je suis en train de parler au puits » : posé pendant le vidage, pour
    /// qu'une émission réentrante rende la main et laisse le vidage livrer.
    talking: Rc<Cell<bool>>,
}

impl Out {
    fn new(sink: Sink) -> Self {
        Self {
            sink: Rc::new(RefCell::new(sink)),
            queue: Rc::new(RefCell::new(VecDeque::new())),
            talking: Rc::new(Cell::new(false)),
        }
    }

    fn emit(&self, event: Event) {
        self.queue.borrow_mut().push_back(event);
        if self.talking.get() {
            // Un vidage est déjà en cours : il prendra cet événement au
            // passage. Rejouer le puits ici serait de la réentrance.
            return;
        }
        self.talking.set(true);
        // Le drapeau doit retomber même si le puits panique : une petite garde
        // locale s'en charge, sans empoisonner le harnais.
        let _reset = ResetTalking(self.talking.clone());
        loop {
            let next = self.queue.borrow_mut().pop_front();
            match next {
                Some(event) => (self.sink.borrow_mut())(event),
                None => break,
            }
        }
    }
}

/// Remet le drapeau « en train de parler » à faux en quittant [`Out::emit`],
/// y compris quand le puits a paniqué.
struct ResetTalking(Rc<Cell<bool>>);

impl Drop for ResetTalking {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Une garde RAII sur la place prise par [`Harness::claim`].
///
/// `claim` pose `busy = true` avant que [`Harness::pump`] tourne, et `pump` le
/// remet à `false` à la toute fin. Si la future rendue par `send` est lâchée
/// avant cette fin — écran démonté, `Owner` leptos disposé, page suspendue,
/// tâche annulée — `busy` resterait `true` pour toujours : tous les `send`
/// suivants rendraient `Running::Queued` et ne tourneraient jamais, sans erreur
/// ni événement. La garde remet `busy = false` et, si le run n'a pas conclu,
/// émet un `Event::Stopped { stop: Stop::Interrupted }` : l'abandon se dit, il
/// ne se constate pas.
///
/// Un puits qui panique sur cet événement fait remonter sa panique depuis un
/// `Drop` : l’état du harnais, lui, reste cohérent (`busy` et le drapeau
/// d’émission sont déjà remis), mais l’hôte qui lâche des futures doit le savoir.
struct BusyGuard {
    state: Shared<State>,
    out: Out,
    /// L'instant où le run a commencé, pour la durée annoncée.
    started: f64,
    /// Le nombre de tours déjà faits, lu au moment de l'abandon.
    turns: Rc<Cell<u32>>,
    /// Désarmée par la fin normale de `pump` : un seul `Stopped` par run.
    armed: bool,
}

impl BusyGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        // La place se rend quoi qu'il arrive — même si la fin normale l'a déjà
        // fait, c'est idempotent.
        self.state.borrow_mut().busy = false;
        // Un abandon se dit, sauf si on est déjà en train de dérouler une
        // panique : ré-invoquer le puits ici ferait un double panic. Le cas
        // normal (future lâchée, sans panique) passe toujours.
        if self.armed && !std::thread::panicking() {
            let ms = (crate::now_ms() - self.started).max(0.0) as u64;
            self.out.emit(Event::Stopped {
                stop: Stop::Interrupted,
                text: String::new(),
                turns: self.turns.get(),
                ms,
                code: None,
            });
        }
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
            out: Out::new(Box::new(sink)),
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
        self.send_with_images(text, Vec::new()).await
    }

    /// [`Self::send`], avec des images jointes au message.
    pub async fn send_with_images(
        &self,
        text: impl Into<String>,
        images: Vec<ImagePart>,
    ) -> Running {
        self.handle().enqueue_with_images(text, images);
        if !self.claim() {
            return Running::Queued;
        }
        Running::Ran(self.pump().await)
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
        Some(self.pump().await)
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

    async fn pump(&self) -> Stop {
        let started = crate::now_ms();
        let cancel = self.with(|s| s.cancel.clone());
        cancel.reset();
        let transport = self.transport.clone();
        let toolbox = self.toolbox.clone();

        // La garde tient la place prise par `claim` : si cette future est
        // lâchée avant la fin (écran démonté, page suspendue, tâche annulée),
        // elle rend `busy` et dit l'abandon — sinon le harnais restait muet et
        // `busy = true` pour toujours (mesuré : poll unique puis drop, second
        // envoi = `Queued`).
        let turns = Rc::new(Cell::new(0u32));
        let mut garde = BusyGuard {
            state: self.state.clone(),
            out: self.out.clone(),
            started,
            turns: turns.clone(),
            armed: true,
        };
        let mut stop: Option<Stop> = None;
        let mut said = String::new();
        let mut code: Option<String> = None;

        'goals: loop {
            // 1. Le prochain message tenu ouvre un objectif. Sans message, il
            //    n'y a plus rien à faire : le run s'arrête là où il était.
            let Some(first) = self.take_pending() else {
                break 'goals;
            };
            self.push(Message::user_with_images(
                first.text.clone(),
                first.images.clone(),
            ));
            self.out.emit(Event::User {
                text: first.text.clone(),
            });

            if cancel.is_cancelled() {
                stop = Some(Stop::Cancelled);
                break 'goals;
            }

            let limits = self.with(|s| s.limits.clone());
            let mut run = Run::new(Goal::new(first.text.clone()), limits.max_tours);
            run.max_relances = limits.max_relances;
            if let Some(budget) = limits.budget_ms {
                run.budget(budget);
            }

            // Les tours comptent PAR OBJECTIF : un message tenu ouvre un
            // objectif neuf, comme `Run::new` remet ses compteurs à zéro.
            let mut goal_turns = 0u32;

            'turns: loop {
                if cancel.is_cancelled() {
                    stop = Some(run.finish(Stop::Cancelled));
                    break 'turns;
                }

                // La ceinture, indépendante de la politique : une politique
                // branchée qui rend `Continue` ne rencontre aucune borne dans
                // `after_turn` (mesuré : une sonde tuée à 180 s). Avec
                // `DefaultPolicy`, la ceinture et la politique se déclenchent
                // au même tour : sans effet de bord.
                //
                // Elle lit `Limits`, la composition, et non `run` : une
                // politique reçoit `&mut Run` et pouvait donc élargir le budget
                // ou le retirer (`run.budget_ms = None`) tout en restant bornée
                // sur les tours — deux bornes dont une seule tenait. Un plafond
                // de zéro arrête avant le premier tour.
                let budget_epuise = limits.budget_ms.is_some_and(|b| run.ecoule_ms >= b);
                if goal_turns >= limits.max_tours || budget_epuise {
                    stop = Some(run.finish(Stop::Budget));
                    break 'turns;
                }

                self.out.emit(Event::Turn { index: turns.get() });
                turns.set(turns.get() + 1);
                goal_turns += 1;

                // 2. Un tour du modèle, en flux.
                let request = self.request(toolbox.as_ref());
                let gathered = Rc::new(RefCell::new(String::new()));
                let delta: Delta = {
                    let gathered = gathered.clone();
                    let out = self.out.clone();
                    let cancel = cancel.clone();
                    Rc::new(move |text: &str| {
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

                // Le temps du TOUR est de l'horloge murale : on le mesure autour
                // du transport, panne comprise, et on l'ajoute au run AVANT la
                // décision. Sans ça, un modèle lent et des outils instantanés
                // n'avançaient jamais le budget (mesuré : 40 appels au modèle
                // pour un budget de 10 ms qui ne protégeait rien).
                let turn_started = crate::now_ms();
                let reply = match transport.stream(&request, &cancel, delta).await {
                    Ok(reply) => reply,
                    Err(failure) => {
                        run.add_elapsed((crate::now_ms() - turn_started).max(0.0) as u64);
                        if failure.is_aborted() || cancel.is_cancelled() {
                            stop = Some(run.finish(Stop::Cancelled));
                        } else {
                            code = Some(failure.code.as_str().to_string());
                            stop = Some(run.finish(Stop::Failed(failure.message)));
                        }
                        break 'turns;
                    }
                };
                run.add_elapsed((crate::now_ms() - turn_started).max(0.0) as u64);

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
                            unchanged: false,
                            ms: 0,
                        });
                        self.push(Message::tool(
                            call.id.clone(),
                            "annulé : l'utilisateur a interrompu le tour",
                        ));
                        continue;
                    }
                    let outcome = self
                        .run_tool(toolbox.as_ref(), call, &cancel, &mut run)
                        .await;
                    self.push(Message::tool(call.id.clone(), outcome.text.clone()));
                }

                let spoke = !text.trim().is_empty();
                let called = !reply.calls.is_empty();
                // Le motif se lit AVANT la décision : `after_turn` déplace la
                // frontière du tour, et le refus d'aujourd'hui ne serait plus
                // visible après. Sans ça, une action refusée était suivie d'un
                // « je m'arrête là » accepté comme une conclusion.
                let hint = relaunch_reason(&run, spoke, called);

                match run.after_turn_with(self.policy.as_ref(), spoke, called) {
                    Decision::Continue => {
                        if let Some(hint) = hint {
                            self.out.emit(Event::Hint { text: hint.clone() });
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

            // Un objectif atteint, mais des messages attendent : la discussion
            // continue, elle ne s'arrête pas parce que le modèle a conclu.
            if self.with(|s| s.pending.is_empty()) {
                break 'goals;
            }
        }

        let stop = stop.unwrap_or(Stop::Done);
        let ms = (crate::now_ms() - started).max(0.0) as u64;
        // La fin normale rend la place AVANT de l'annoncer (un puits qui relit
        // `busy()` sur `Stopped` voit déjà le run terminé), puis désarme la
        // garde : un seul `Stopped` par run, jamais deux.
        garde.disarm();
        self.with(|s| s.busy = false);
        self.out.emit(Event::Stopped {
            stop: stop.clone(),
            text: said,
            turns: turns.get(),
            ms,
            code,
        });
        stop
    }

    async fn run_tool(
        &self,
        toolbox: &dyn Toolbox,
        call: &ToolCall,
        cancel: &Cancel,
        run: &mut Run,
    ) -> Outcome {
        self.out.emit(Event::ToolUse {
            id: call.id.clone(),
            tool: call.name.clone(),
            input: call.arguments.clone(),
        });
        let started = crate::now_ms();
        let outcome = toolbox.call_with_cancel(call, cancel).await;
        let ms = (crate::now_ms() - started).max(0.0) as u64;

        self.out.emit(Event::ToolResult {
            id: call.id.clone(),
            tool: call.name.clone(),
            text: outcome.text.clone(),
            refused: outcome.refused.is_some(),
            failed: outcome.failed.is_some(),
            unchanged: outcome.unchanged(),
            ms,
        });

        let mut step = Step::tool(call.name.clone(), ms)
            .state(outcome.before.clone(), outcome.after.clone())
            .resume(outcome.text.chars().take(200).collect::<String>());
        if let Some(motif) = &outcome.refused {
            step = step.refused(motif.clone());
        }
        if let Some(cause) = &outcome.failed {
            step = step.failed(cause.clone());
        }
        run.record(step);
        outcome
    }

    fn request(&self, toolbox: &dyn Toolbox) -> Request {
        let (system, messages) = self.with(|s| (s.system.clone(), s.transcript.clone()));
        Request::new(system, messages, toolbox.specs())
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
        self.enqueue_with_images(text, Vec::new())
    }

    /// [`Self::enqueue`], avec des images jointes au message.
    pub fn enqueue_with_images(&self, text: impl Into<String>, images: Vec<ImagePart>) -> u64 {
        let id = {
            let mut state = self.state.borrow_mut();
            let id = state.next_id;
            state.next_id += 1;
            let text = text.into();
            // Une image seule est un message : la jeter parce que le texte est vide
            // serait la perdre en silence.
            if !text.trim().is_empty() || !images.is_empty() {
                state.pending.push(Pending {
                    id,
                    text,
                    at_ms: crate::now_ms(),
                    images,
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
