//! Les ports par lesquels le harnais touche le monde.
//!
//! Le harnais ne fait aucune entrée/sortie lui-même : il **appelle**. Un crate
//! qui décide de son transport est un crate qu'on ne peut pas brancher deux
//! fois — dans une page et dans une coquille native, par exemple.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use this_agent::cite::SeenLedger;

use async_trait::async_trait;

use crate::event::Outcome;
use crate::wire::{Failure, Message, Reply, Request, ToolCall, ToolSpec};
use this_agent::evidence::Declaration;
use this_agent::{Run, Step, Stop};

/// Un fragment de texte poussé vers l'interface **pendant** la lecture du flux.
///
/// `Rc<dyn Fn(&str)>` et pas `&mut dyn FnMut` : le transport doit pouvoir
/// garder ce rappel dans un écouteur ('static) et l'appeler depuis un autre
/// point du système — c'est exactement ce que fait une coquille native qui
/// relaie les morceaux par événement. Un `&mut` obligerait à faire passer le
/// rappel par le corps de la fonction, donc à ne rien afficher avant la fin.
pub type Delta = Rc<dyn Fn(&str)>;

/// Le drapeau d'annulation, partagé entre l'interface et le tour en cours.
///
/// Un drapeau dans un `Rc` suffit : la boucle est sur un seul fil. Un transport
/// qui peut vraiment interrompre (couper une connexion) le consulte lui-même ;
/// sinon, le harnais s'arrête à la frontière suivante et le dit.
///
/// L'annulation **réveille** : une attente du harnais sur [`Cancel::cancelled`]
/// est relancée dès [`Cancel::cancel`], même sans aucun minuteur (une page wasm
/// qui n'en a pas injecté). Sans réveil, une future suspendue sur un port qui ne
/// répond jamais ne relirait le drapeau… jamais.
#[derive(Clone, Default)]
pub struct Cancel(Rc<CancelState>);

#[derive(Default)]
struct CancelState {
    flag: Cell<bool>,
    /// Un réveil par future [`Cancelled`] vivante, rangé sous son numéro : un
    /// exécuteur qui fabrique un réveil neuf à chaque sondage ne fait pas grossir
    /// la liste, il remplace le sien. Borné par le nombre d'attentes en cours.
    wakers: RefCell<Vec<(u64, std::task::Waker)>>,
    next_slot: Cell<u64>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    /// Pose le drapeau et réveille toutes les attentes de [`Cancel::cancelled`].
    pub fn cancel(&self) {
        self.0.flag.set(true);
        let wakers = std::mem::take(&mut *self.0.wakers.borrow_mut());
        for (_, waker) in wakers {
            waker.wake();
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.flag.get()
    }
    /// Consommé en **fin** de run (jamais au début) : une annulation ne vaut que pour le run
    /// qui la voit, et une annulation arrivée entre deux runs est vue dès le premier tour du suivant.
    pub fn reset(&self) {
        self.0.flag.set(false);
        self.0.wakers.borrow_mut().clear();
    }
    /// Une future qui rend dès que l'annulation est posée — immédiatement si elle
    /// l'est déjà. Elle n'a besoin d'aucun minuteur : `cancel` la réveille.
    pub fn cancelled(&self) -> Cancelled {
        let slot = self.0.next_slot.get();
        self.0.next_slot.set(slot.wrapping_add(1));
        Cancelled {
            state: self.0.clone(),
            slot,
        }
    }
}

/// L'attente d'une annulation (voir [`Cancel::cancelled`]). Elle tient un seul
/// réveil, le dernier avec lequel elle a été sondée, et le retire quand elle
/// est lâchée.
pub struct Cancelled {
    state: Rc<CancelState>,
    slot: u64,
}

impl std::future::Future for Cancelled {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.state.flag.get() {
            return std::task::Poll::Ready(());
        }
        let mut wakers = self.state.wakers.borrow_mut();
        match wakers.iter_mut().find(|(slot, _)| *slot == self.slot) {
            Some((_, waker)) => waker.clone_from(cx.waker()),
            None => wakers.push((self.slot, cx.waker().clone())),
        }
        std::task::Poll::Pending
    }
}

impl Drop for Cancelled {
    fn drop(&mut self) {
        self.state
            .wakers
            .borrow_mut()
            .retain(|(slot, _)| *slot != self.slot);
    }
}

/// Comment le harnais parle à un moteur.
///
/// `?Send` : le même trait doit vivre dans une page (`!Send`) et dans un
/// binaire. Un serveur qui a besoin de `Send` enveloppe la boucle dans un
/// `LocalSet` ; l'inverse (exiger `Send` partout) rend le crate inutilisable
/// sur wasm, c'est-à-dire là où il sert.
#[async_trait(?Send)]
pub trait Transport {
    /// Un tour : le modèle répond, éventuellement en demandant des outils.
    ///
    /// `delta` est appelé pour chaque morceau de texte — c'est ce qui rend le
    /// flux visible. `cancel` est à consulter pendant la lecture : un
    /// transport qui l'ignore n'est pas faux, il est seulement plus lent à
    /// s'arrêter.
    async fn stream(
        &self,
        request: &Request,
        cancel: &Cancel,
        delta: Delta,
    ) -> Result<Reply, Failure>;

    /// Un transport de la **famille A** (le moteur exécute les outils pendant
    /// `stream`) reçoit la porte du run avant le flux : c'est par elle qu'il
    /// consulte `gate` et journalise ses `ToolUse`/`ToolResult` hors-bande.
    ///
    /// Un transport qui n'exécute pas les outils lui-même l'ignore (défaut) :
    /// rien ne change, le harnais continue de passer par `run_tool`.
    ///
    /// La porte ne porte pas `Toolbox::specs()` : dans ce mode, la surface est
    /// déclarée au moteur par le transport lui-même (MCP, `--allowedTools`…),
    /// pas par le Toolbox — voir [`Permission`].
    fn set_permission(&self, _permission: Rc<dyn Permission>) {}

    /// Ce transport **honore-t-il** la paire que le routeur (mode Auto) a choisie
    /// pour le tour — lit-il [`Request::model`] et [`Request::provider`] pour
    /// parler au bon moteur ? Faux par défaut : un transport qui ne le déclare
    /// pas sert son modèle à lui.
    /// Et le déclarer ne suffit pas : la réponse doit NOMMER le modèle servi (`Reply::served`),
    /// égal à celui demandé ou instantané daté de celui-ci ([`crate::served_matches`]), pour que
    /// la paire soit réputée atteinte. Une réponse qui n'en nomme aucun, ou un autre, n'apprend
    /// rien au routeur (#288, routé ≠ servi).
    ///
    /// Tant que c'est faux, la décision du routeur est **consultative** :
    /// [`crate::Event::Routed`] porte `advisory: true`, l'état servi du routeur ne
    /// bouge pas, et la fin du tour n'est pas rendue à l'apprentissage (un bras ne
    /// se nourrit pas d'un tour qu'il n'a pas servi).
    fn honors_route(&self) -> bool {
        false
    }

    /// Le **profil d'EXÉCUTION** de cet hôte (voir [`ProfilRuntime`]).
    ///
    /// Le défaut est [`ProfilRuntime::minimal`] : **rien** n'est promis (ni délai, ni
    /// annulation, ni isolation, ni supervision). Un hôte qui tient une garantie doit la
    /// DÉCLARER — [`ProfilRuntime::natif`] pour une coquille native complète,
    /// [`ProfilRuntime::wasm_sans_minuteur`] pour une page wasm, ou un profil sur mesure. Un
    /// défaut « natif » annoncerait un minuteur et une isolation par processus sans preuve,
    /// exactement ce que #285 interdit.
    ///
    /// Ce profil **ne lit pas l'horloge** : `minuteur` décrit ce que l'hôte réalise. L'horloge
    /// du harnais est injectée à part (`Harness::with_clock`) ; un hôte qui déclare
    /// `minuteur: true` doit injecter une horloge qui le réalise.
    fn profil_runtime(&self) -> ProfilRuntime {
        ProfilRuntime::minimal()
    }
}

/// Le profil d'**exécution** d'un hôte : ce qu'il garantit à l'exécution, pas
/// ce que sa compilation laisse espérer.
///
/// Une compilation `wasm32` n'égale pas une exécution native : sans minuteur,
/// une borne temporelle n'existe pas ; `catch_unwind` ne stoppe ni une boucle
/// synchrone ni `panic=abort`. Ce type rend ces limites **explicites et
/// interrogeables** ([`ProfilRuntime::exiger`]), pour qu'un hôte ne puisse pas
/// annoncer un délai qu'il ne peut pas tenir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfilRuntime {
    /// Un délai/timeout s'applique-t-il **réellement** ? Faux en wasm sans
    /// minuteur injecté (`Clock::sleep` rend `None`).
    pub minuteur: bool,
    /// L'annulation coopérative réveille-t-elle une attente ? Vrai même sans
    /// minuteur : [`Cancel`] réveille sur son drapeau, pas sur une horloge.
    pub annulation: bool,
    /// Ce qui arrête un outil qui boucle ou panique.
    pub isolation: Isolation,
    /// Un processus enfant est-il **réellement** arrêté quand on le demande ?
    pub supervision: bool,
}

/// Le degré d'isolation d'un outil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Rien ne protège : ni panique, ni boucle.
    Aucune,
    /// `catch_unwind` : couvre une panique `unwind`, **pas** une boucle
    /// synchrone ni un `panic=abort`.
    CatchUnwind,
    /// Une frontière de processus : l'outil s'arrête vraiment (boucle et
    /// mémoire comprises).
    Processus,
}

/// Ce que le harnais a besoin de garantir pour un run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Besoin {
    /// Une borne temporelle (budget, timeout) qui doit s'appliquer.
    Delai,
    /// Une attente qui doit se réveiller à l'annulation.
    Annulation,
    /// Un outil qui peut boucler/paniquer doit être arrêté.
    IsolationDure,
    /// Un processus enfant doit être réellement arrêté.
    Supervision,
}

/// Pourquoi un profil n'honore pas un besoin. Nommé, pour que le refus soit
/// diagnosticable — jamais un booléen muet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refus {
    SansMinuteur,
    SansAnnulation,
    IsolationInsuffisante(Isolation),
    SansSupervision,
}

impl ProfilRuntime {
    /// Le profil qui ne promet RIEN : le défaut d'un transport qui ne se déclare pas.
    pub fn minimal() -> Self {
        Self {
            minuteur: false,
            annulation: false,
            isolation: Isolation::Aucune,
            supervision: false,
        }
    }

    /// Le profil d'un hôte natif complet (LocalSet, minuteur, processus).
    pub fn natif() -> Self {
        Self {
            minuteur: true,
            annulation: true,
            isolation: Isolation::Processus,
            supervision: true,
        }
    }

    /// Le profil d'une page **wasm sans minuteur injecté** : l'annulation
    /// réveille (elle ne dépend pas d'une horloge), mais un délai ne s'applique
    /// pas, et l'isolation est au mieux `catch_unwind`.
    pub fn wasm_sans_minuteur() -> Self {
        Self::wasm_sans_minuteur_avec(cfg!(panic = "unwind"))
    }

    /// [`ProfilRuntime::wasm_sans_minuteur`] selon que la panique est rattrapable.
    /// Sur wasm32 (`panic = "abort"`), `catch_unwind` ne protège rien : l'isolation
    /// est alors `Aucune`, pas `CatchUnwind`.
    fn wasm_sans_minuteur_avec(panique_rattrapable: bool) -> Self {
        Self {
            minuteur: false,
            annulation: true,
            isolation: if panique_rattrapable {
                Isolation::CatchUnwind
            } else {
                Isolation::Aucune
            },
            supervision: false,
        }
    }

    /// Ce profil honore-t-il `besoin` ? Sinon, **nomme le manque**. À appeler
    /// avant de publier un budget, une annulation, une isolation ou une
    /// supervision : un hôte ne doit JAMAIS annoncer une garantie qu'il ne tient
    /// pas.
    pub fn exiger(&self, besoin: Besoin) -> Result<(), Refus> {
        match besoin {
            Besoin::Delai if !self.minuteur => Err(Refus::SansMinuteur),
            Besoin::Annulation if !self.annulation => Err(Refus::SansAnnulation),
            Besoin::IsolationDure if self.isolation != Isolation::Processus => {
                Err(Refus::IsolationInsuffisante(self.isolation))
            }
            Besoin::Supervision if !self.supervision => Err(Refus::SansSupervision),
            _ => Ok(()),
        }
    }
}

/// La porte du run, rendue à un transport de la famille A.
///
/// `run_tool` (dans la boucle) est ce qui applique trois choses à un appel
/// d'outil : la déclaration (`Toolbox::specs()`), l'autorisation (`gate`) et le
/// journal (`Recorder`). Un moteur qui exécute l'outil **pendant** `stream`
/// n'atteint jamais `run_tool` : sans cette porte, les trois sont contournés.
///
/// La porte rend au transport, hors-bande :
/// - la **décision d'autorisation** ([`Permission::allows`]) : la même que
///   `gate`, avec le même motif de refus — le transport la renvoie au moteur
///   (permission refusée), il ne se contente pas de la noter ;
/// - la **journalisation** ([`Permission::record`]) : un `ToolUse`/`ToolResult`
///   du flux devient la même [`Step`](this_agent::Step) que `run_tool` en
///   produirait, poussée au `Recorder` du run en cours.
///
/// La **déclaration** n'y figure pas, et c'est une décision écrite : dans ce
/// mode, la surface est déclarée au moteur par le transport (MCP,
/// `--allowedTools`…), pas par `Toolbox::specs()`. Le transport ignore donc
/// `Request::tools()` **explicitement** — jamais « par oubli » : un état où
/// `specs()` existerait sans effet est interdit (décision `famille-a-specs-ignorees`).
#[async_trait(?Send)]
pub trait Permission {
    /// La même décision que `gate` : `Ok(())` si `tool` peut partir, sinon le
    /// motif du refus — à renvoyer au moteur, pas seulement à enregistrer.
    fn allows(&self, tool: &str) -> Result<(), String>;

    /// Journalise un appel venu du flux, avec la même forme d'étape que
    /// `run_tool` : `outcome` (refus, échec, empreinte compris) devient une
    /// [`Step`](this_agent::Step) poussée au `Recorder` sous l'identité du run
    /// en cours. `ms` est la durée mesurée de l'appel (comme dans `run_tool`).
    async fn record(&self, call: &ToolCall, ms: u64, outcome: &Outcome);
}

/// Ce que le harnais peut faire faire au noyau de l’application.
///
/// `&self` et non `&mut self` : les outils d'une interface vivent dans des
/// signaux, pas dans une structure qu'on possède. Le harnais ne doit pas
/// imposer sa façon de partager l'état.
#[async_trait(?Send)]
pub trait Toolbox {
    /// La surface exposée au modèle. Le harnais ne la connaît pas — c'est tout
    /// l'intérêt : ajouter un outil ne touche pas au crate.
    fn specs(&self) -> Vec<ToolSpec>;

    /// Ce que chaque outil fait au monde (écrit, lit, note) et les mots qui le désignent
    /// dans un récit : la preuve s'en sert pour confronter « j'ai déployé » à `deploy_app`
    /// refusé ([`Declaration`]). Par défaut aucune : la preuve devine d'après les noms
    /// (`install_module`, `row_create`, `remember`…), comme avant.
    fn declarations(&self) -> Vec<Declaration> {
        Vec::new()
    }

    /// Les autorisations que cet outillage tient **à jour**, ou `None` (défaut).
    ///
    /// POURQUOI ce crochet, à côté de [`Toolbox::declarations`] : les outils d'un hôte
    /// peuvent apparaître APRÈS la construction du harnais — un module installé pendant
    /// la session en ajoute. Un ensemble figé par [`Harness::with_grants`] les
    /// refuserait tous, et le refus serait juste en forme, faux en fait.
    ///
    /// Quand il rend `Some`, cet ensemble fait foi **à chaque appel** et remplace celui
    /// de `with_grants` : un hôte qui le tient à jour est la meilleure source. Quand il
    /// rend `None`, `with_grants` décide, comme avant.
    fn grants(&self) -> Option<this_agent::substrate::Grants> {
        None
    }

    /// Le registre des ids que cet outillage a fait PARAÎTRE.
    ///
    /// Le contexte du tour s'en sert pour que les ids qu'il montre au modèle
    /// comptent comme lus. Sans ce partage, le contexte et les outils tiennent
    /// deux comptes séparés : une citation vérifiée par l'un est « jamais
    /// parue » pour l'autre, et la preuve se contredit elle-même.
    ///
    /// Par défaut `None` : un outillage qui ne tient pas de registre n'en
    /// partage pas. Une implémentation qui en tient un doit rendre LE SIEN,
    /// pas une copie — c'est le partage qui fait la garantie.
    ///
    /// **Ce que le harnais y inscrit** : le texte de chaque résultat, seulement
    /// CITÉ ; et, pour un appel réussi, ce que l'outil déclare avoir fait lire par
    /// `Outcome::observable`, en lecture directe. Un outil qui ne déclare rien ne
    /// prouve rien : c'est le défaut sûr.
    ///
    /// **Un registre par SESSION** : le harnais ne le crée pas, il lit celui de
    /// l'outillage. Deux harnais construits sur le même `Rc<dyn Toolbox>`
    /// partagent donc le registre, et un `restore`/`seed` l'écrase. Un hôte qui
    /// veut un registre par session construit un outillage par session.
    fn vus(&self) -> Option<Rc<RefCell<SeenLedger>>> {
        None
    }

    /// Exécute un appel et rend ce que le modèle lira, plus l'observation.
    async fn call(&self, call: &ToolCall) -> Outcome;

    /// [`Self::call`], avec le drapeau d'annulation du run en cours.
    ///
    /// Un outil qui attend quelque chose (une réponse de permission, un
    /// `fetch`, un `sleep`) doit consulter `cancel` et rendre un
    /// `Outcome::failed(...)` au lieu de continuer à attendre : « l'arrêt est
    /// un état, pas un espoir » vaut aussi pour les outils. Par défaut, un
    /// outil qui ne la surcharge pas garde exactement le comportement d'avant.
    async fn call_with_cancel(&self, call: &ToolCall, cancel: &Cancel) -> Outcome {
        let _ = cancel;
        self.call(call).await
    }
}

/// Comment le harnais résume ses tours anciens au lieu de les jeter.
///
/// POURQUOI un port et pas un appel interne : résumer demande un modèle, donc
/// une entrée/sortie, et le harnais n'en fait aucune lui-même. L'hôte branche ce
/// qu'il veut — le même moteur, un modèle moins cher, une extraction sans modèle.
///
/// Contrat : recevoir une suite de messages anciens (jamais coupée au milieu d'un
/// groupe d'outils) et rendre **un** message qui la remplace. Le harnais ne lui
/// fait pas confiance : un message vide, un `Tool`/`System`, un `Assistant` qui
/// porte des appels, ou un résumé qui ne coûte pas strictement moins que ce qu'il
/// remplace est jeté, et l'on retombe sur la jetée pure (voir `Harness::with_compactor`).
/// Un échec ne tue jamais le tour.
#[async_trait(?Send)]
pub trait Compactor {
    /// Résume une suite de messages anciens en UN message qui la remplace.
    async fn compact(&self, anciens: &[Message]) -> Result<Message, Failure>;
}

/// Un [`Toolbox`] fait de plusieurs : l'hôte apporte ses outils, un module
/// apporte les siens, et le harnais ne sait pas lesquels viennent d'où.
///
/// C'est la pièce qui rend le harnais **composable** côté outils : un module
/// installé peut ajouter une capacité sans qu'on touche au `Toolbox` de
/// l’application, et sans que la boucle change d'une ligne.
///
/// La table de routage est reconstruite **à chaque appel** : un module peut être
/// installé pendant un run, et un cache figé servirait l'ancien monde. Le coût
/// est celui d'une poignée de `specs()` — le prix de ne pas mentir.
#[derive(Default)]
pub struct Toolboxes {
    parts: Vec<Rc<dyn Toolbox>>,
}

impl Toolboxes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute une source d'outils (constructeur chaînable).
    pub fn with(mut self, part: Rc<dyn Toolbox>) -> Self {
        self.parts.push(part);
        self
    }

    /// Ajoute une source d'outils à un `Toolboxes` déjà partagé.
    pub fn push(&mut self, part: Rc<dyn Toolbox>) {
        self.parts.push(part);
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

/// Les `specs()` d'une part, isolées : une part qui **panique** est ignorée (liste vide),
/// elle ne fait pas échouer le routage des autres.
///
/// Ce que cela ne protège pas : une part qui **boucle sans fin** dans `specs()` (la méthode
/// est synchrone, rien ne peut la borner), ni un build `panic = "abort"` (wasm32 inclus),
/// où la panique n'est pas rattrapable.
fn part_specs(part: &Rc<dyn Toolbox>) -> Vec<ToolSpec> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| part.specs())).unwrap_or_default()
}

/// [`part_specs`] pour `declarations()` : même isolation, même limite.
fn part_declarations(part: &Rc<dyn Toolbox>) -> Vec<Declaration> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| part.declarations()))
        .unwrap_or_default()
}

#[async_trait(?Send)]
impl Toolbox for Toolboxes {
    /// DÉFAUT FERMÉ : deux parts qui exposaient le même nom l'annonçaient deux fois au
    /// modèle (un fournisseur rejette des noms d'outil dupliqués : tout le tour en 400),
    /// alors que l'appel, lui, va à la première. L'annonce suit donc le routage : un nom,
    /// sa première part.
    fn specs(&self) -> Vec<ToolSpec> {
        let mut vus = std::collections::HashSet::new();
        self.parts
            .iter()
            .flat_map(part_specs)
            .filter(|spec| vus.insert(spec.name.clone()))
            .collect()
    }

    /// Chaque part ne déclare que **ses** outils, et seulement ceux qu'elle exécute : une
    /// déclaration est gardée si sa part est la première, dans l'ordre de composition, à
    /// exposer ce nom dans ses `specs()` — la même règle que le routage de l'appel. Un module
    /// qui ne fournit que `memo` ne peut donc pas déclarer `reads("install_module")`, ni
    /// redéclarer un outil qu'une part précédente expose déjà. Le reste est ignoré.
    fn declarations(&self) -> Vec<Declaration> {
        let mut proprietaire: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (i, part) in self.parts.iter().enumerate() {
            for spec in part_specs(part) {
                proprietaire.entry(spec.name).or_insert(i);
            }
        }
        self.parts
            .iter()
            .enumerate()
            .flat_map(|(i, part)| {
                let proprietaire = &proprietaire;
                part_declarations(part)
                    .into_iter()
                    .filter(move |x| proprietaire.get(&x.tool) == Some(&i))
            })
            .collect()
    }

    /// Le registre de la **première** part qui en tient un — même règle que
    /// l'annonce, les déclarations et l'appel : le premier qui expose gagne.
    ///
    /// Sans ça, la composition qui sert les vrais hôtes rendrait `None` (le
    /// défaut du trait) et toute la provenance resterait **inerte en
    /// production**, alors que chaque part tiendrait son registre.
    fn vus(&self) -> Option<Rc<RefCell<SeenLedger>>> {
        self.parts.iter().find_map(|part| part.vus())
    }

    /// Les droits que la composition porte, **par propriété de l'outil** (#287).
    ///
    /// POURQUOI cette méthode existe alors que le défaut du trait (`None`) est plus simple :
    /// `None` ne veut pas dire « rien à vérifier » dans un hôte qui a des droits installés —
    /// il veut dire que la composition n'en porte aucun, et le harnais retombe alors sur
    /// `with_grants`, ou sur rien du tout. Mesuré le 24/11/2026 : un outillage qui RÉVOQUE
    /// un de ses outils perdait sa révocation en entrant dans une composition — le même
    /// outil était refusé seul (effets 0) et exécuté composé (effets 1). Une révocation qui
    /// disparaît est un octroi.
    ///
    /// RÈGLE : la composition ne retient les droits d'une part que pour les outils que
    /// CETTE part exécute — le propriétaire d'un nom étant, comme pour `specs`,
    /// `declarations` et l'appel, la **première part qui l'expose**. Aucune part ne peut
    /// donc accorder ni révoquer un outil qu'une autre exécute : elle ne dépasse pas son
    /// domaine. Ni « la première gagne » ni source désignée à la construction : ce serait un
    /// choix arbitraire (l'ordre de composition, ou un index que `push` périme), alors que la
    /// propriété de l'outil est déjà la règle du routage.
    ///
    /// Si aucune part n'expose de droits : `None`, et `with_grants` décide comme avant. Si
    /// une part en expose mais qu'aucun ne vise ses outils, la composition rend `Some` vide :
    /// tout est refusé (défaut fermé), jamais « tout est permis ».
    fn grants(&self) -> Option<this_agent::substrate::Grants> {
        let mut proprietaire: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (i, part) in self.parts.iter().enumerate() {
            for spec in part_specs(part) {
                proprietaire.entry(spec.name).or_insert(i);
            }
        }
        let mut droits = this_agent::substrate::Grants::new();
        let mut une_part_en_porte = false;
        for (i, part) in self.parts.iter().enumerate() {
            let Some(siens) = part.grants() else {
                continue;
            };
            une_part_en_porte = true;
            for grant in siens.grants {
                if proprietaire.get(&grant.tool) == Some(&i) {
                    droits.put(grant);
                }
            }
        }
        une_part_en_porte.then_some(droits)
    }

    async fn call(&self, call: &ToolCall) -> Outcome {
        // Le premier qui déclare l’outil l’exécute : deux modules qui
        // exposeraient le même nom ne se disputent pas l’appel, l’ordre de
        // composition décide — et il est visible dans l’application.
        //
        // Une seule table de routage, celle de `call_with_cancel` : deux copies
        // de la même règle divergent au premier changement. Un appel hors run
        // n’a rien à annuler, d’où le drapeau neuf.
        self.call_with_cancel(call, &Cancel::new()).await
    }

    async fn call_with_cancel(&self, call: &ToolCall, cancel: &Cancel) -> Outcome {
        // La table de routage, avec le drapeau du run : le premier qui déclare
        // l’outil l’exécute et reçoit l’annulation.
        for part in &self.parts {
            if part_specs(part).iter().any(|spec| spec.name == call.name) {
                return part.call_with_cancel(call, cancel).await;
            }
        }
        Outcome::refused(
            format!("outil inconnu : {}", call.name),
            format!("aucun toolbox ne déclare « {} »", call.name),
        )
    }
}

// ---- enregistrement -------------------------------------------------------

/// L'enregistrement d'un run : chaque étape dès qu'elle est mesurée, la fin
/// dès qu'elle est sue.
///
/// Le harnais ne sait pas ce que devient une étape : elle peut être écrite dans
/// un graphe (obrain), poussée sur un canal, rejouée par un test, ou jetée.
/// C'est tout l'intérêt du port : la boucle **pousse**, l'hôte **décide**. Un
/// run qui tourne n'est plus un secret jusqu'à sa fin — un crash, une page
/// fermée, une interruption laissent les étapes déjà poussées, au lieu d'un
/// journal entier perdu parce qu'il n'était recollé qu'à la sortie.
///
/// `?Send`, comme [`Transport`] et [`Toolbox`] : le trait doit vivre dans une
/// page (`!Send`) et dans une coquille native.
///
/// # Un échec d'enregistrement n'interrompt jamais le run
///
/// Les méthodes ne rendent **rien** : la boucle n'a aucun `Result` à propager,
/// donc aucune raison de s'arrêter parce qu'une écriture a échoué. C'est
/// l'implémentation (l'hôte) qui avale ses propres erreurs ; le harnais, lui,
/// n'a même pas la possibilité d'en voir une. La composition [`Recorders`]
/// pousse chaque événement à toutes les parts, sans s'arrêter si l'une échoue.
///
/// # Le MASQUE n'est pas dans ce port — c'est un contrat, pas un oubli
///
/// La boucle pousse la [`Step`](this_agent::Step) **telle que l'outil l'a
/// produite**, empreintes et extraits compris. Ce crate ne masque rien : il ne
/// fait aucune I/O, ne connaît aucune table de motifs, et ne peut donc pas
/// garantir qu'un secret recopié dans un résultat d'outil ne sera pas écrit.
///
/// **Le masquage appartient à l'implémentation** (`this_agent::masque` dans
/// l'application), qui l'applique au point d'écriture. Ce n'est pas une couche
/// qu'on peut oublier en silence : c'est un choix de l'hôte, et il doit être
/// VISIBLE. Deux moyens de le rendre visible, l'un ou l'autre selon l'hôte :
///
/// - exposer un booléen d'état (dans this-rs :
///   `HarnessGraph::is_mask_installed`) qu'un hôte, un test ou un diagnostic peut
///   interroger — un journal sans masque se constate, il ne se devine pas ;
/// - écrire son propre `Recorder` : il assume alors le contrat entier, masque compris.
///
/// Conséquence à retenir, et elle est de sécurité : **un `Recorder` sans masque
/// écrit ce que l'outil a rendu**, y compris un secret. Un hôte qui branche un
/// `Recorder` doit pouvoir répondre à « le masque est-il installé ? ».
#[async_trait(?Send)]
pub trait Recorder {
    /// Une étape vient d'être mesurée. `run` la porte déjà : la boucle a appelé
    /// `Run::record` juste avant, et `run.steps` la contient — l'hôte qui veut
    /// écrire le run **partiel** (son `_data`, son décompte de tours) l'a sous
    /// la main sans rien reconstruire.
    async fn step(&self, run_id: &str, run: &Run, step: &Step);

    /// Un appel d'outil va être TENTÉ : son résultat est encore **inconnu**.
    ///
    /// Écrite AVANT l'exécution de l'outil (contrairement à [`Self::step`], qui
    /// n'arrive qu'après). C'est la seule trace durable qu'un arrêt brutal entre
    /// l'intention et le résultat peut laisser : sans elle, un effet produit puis
    /// perdu ne laisse rien dans le journal. `step` porte `inconnu = true` — une
    /// reprise qui la relit sait que l'effet n'est ni accompli ni échoué, et ne
    /// doit ni conclure ni rejouer.
    ///
    /// Comme [`Self::step`], c'est **au mieux** : un échec n'interrompt jamais le
    /// run, et aucune promesse de durabilité ne repose sur ce succès. Le défaut
    /// est de ne rien faire : un enregistreur qui n'écrit pas l'intention garde
    /// exactement le comportement d'avant (la fenêtre aveugle demeure). Écrire
    /// l'intention est un choix d'hôte, pas une obligation du cœur portable.
    async fn intention(&self, _run_id: &str, _run: &Run, _step: &Step) {}

    /// Le run s'est arrêté — `run.stop` vaut déjà `stop` (la boucle a appelé
    /// `Run::finish`). Après cela, plus aucune étape n'arrive pour ce `run_id`.
    async fn stopped(&self, run_id: &str, run: &Run, stop: &Stop);
}

/// Un [`Recorder`] fait de plusieurs : l'hôte apporte le sien, un module peut
/// apporter le sien, et la boucle ne sait pas combien il y en a.
///
/// Même composition que [`Toolboxes`] : chaque événement est poussé à toutes
/// les parts, dans l'ordre d'ajout. Une part qui échoue n'arrête pas les
/// autres — elle n'a de toute façon rien à rendre (voir [`Recorder`]).
///
/// **Le masque est celui de chaque part, pas de la composition** : `Recorders`
/// ne masque rien (voir « Le MASQUE n'est pas dans ce port »). Composer un
/// journal masqué et un export brut est donc possible — et c'est à l'hôte de
/// savoir lequel des deux il branche.
#[derive(Default)]
pub struct Recorders {
    parts: Vec<Rc<dyn Recorder>>,
}

impl Recorders {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un enregistreur (constructeur chaînable).
    pub fn with(mut self, part: Rc<dyn Recorder>) -> Self {
        self.parts.push(part);
        self
    }

    /// Ajoute un enregistreur à un `Recorders` déjà partagé.
    pub fn push(&mut self, part: Rc<dyn Recorder>) {
        self.parts.push(part);
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

#[async_trait(?Send)]
impl Recorder for Recorders {
    async fn step(&self, run_id: &str, run: &Run, step: &Step) {
        for part in &self.parts {
            part.step(run_id, run, step).await;
        }
    }

    async fn intention(&self, run_id: &str, run: &Run, step: &Step) {
        for part in &self.parts {
            part.intention(run_id, run, step).await;
        }
    }

    async fn stopped(&self, run_id: &str, run: &Run, stop: &Stop) {
        for part in &self.parts {
            part.stopped(run_id, run, stop).await;
        }
    }
}

#[cfg(test)]
mod tests_vus {
    use super::*;
    use crate::wire::{ToolCall, ToolSpec};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Un outillage qui ne tient aucun registre — le cas de l'immense majorité.
    struct SansRegistre;

    #[async_trait(?Send)]
    impl Toolbox for SansRegistre {
        fn specs(&self) -> Vec<ToolSpec> {
            Vec::new()
        }
        async fn call(&self, _call: &ToolCall) -> Outcome {
            Outcome::failed("aucun outil", "outillage de test")
        }
    }

    /// Un outillage qui tient le sien.
    struct AvecRegistre(Rc<RefCell<SeenLedger>>);

    #[async_trait(?Send)]
    impl Toolbox for AvecRegistre {
        fn specs(&self) -> Vec<ToolSpec> {
            Vec::new()
        }
        fn vus(&self) -> Option<Rc<RefCell<SeenLedger>>> {
            Some(Rc::clone(&self.0))
        }
        async fn call(&self, _call: &ToolCall) -> Outcome {
            Outcome::failed("aucun outil", "outillage de test")
        }
    }

    #[test]
    fn le_registre_se_partage_quand_l_outillage_en_tient_un() {
        assert!(
            SansRegistre.vus().is_none(),
            "par défaut, aucun registre n'est partagé : une implémentation muette ne casse rien"
        );
        let mien = Rc::new(RefCell::new(SeenLedger::new()));
        let partage = AvecRegistre(Rc::clone(&mien))
            .vus()
            .expect("cet outillage en tient un");
        assert!(
            Rc::ptr_eq(&partage, &mien),
            "c'est LE registre de l'outillage, pas une copie : le partage fait la garantie"
        );

        // Les deux outillages de test traversent aussi leur `call` : ces lignes-là
        // sont du code ajouté, et un refus muet qui ne s'exécute jamais ne prouve rien.
        let appel = ToolCall {
            id: "1".into(),
            name: "rien".into(),
            arguments: serde_json::json!({}),
        };
        assert!(
            futures::executor::block_on(SansRegistre.call(&appel))
                .failed
                .is_some()
        );
        assert!(
            futures::executor::block_on(AvecRegistre(Rc::clone(&mien)).call(&appel))
                .failed
                .is_some()
        );
    }
}

#[cfg(test)]
mod tests_cancel {
    //! Relecture de #254, constat 9 : les réveils ne s'accumulent pas.
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct Flag(std::sync::atomic::AtomicBool);

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Un réveil neuf à chaque appel, comme un exécuteur qui ne les réutilise pas.
    fn fresh() -> (Arc<Flag>, Waker) {
        let flag = Arc::new(Flag(std::sync::atomic::AtomicBool::new(false)));
        (flag.clone(), Waker::from(flag))
    }

    #[test]
    fn une_attente_ne_garde_qu_un_reveil_quel_que_soit_le_nombre_de_sondages() {
        let cancel = Cancel::new();
        let mut waiting = pin!(cancel.cancelled());
        let mut last = None;
        for _ in 0..1_000 {
            let (flag, waker) = fresh();
            assert!(
                waiting
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
            last = Some(flag);
        }
        assert_eq!(
            cancel.0.wakers.borrow().len(),
            1,
            "un réveil par attente vivante"
        );
        // Deux attentes : deux réveils, pas plus.
        let mut other = pin!(cancel.cancelled());
        let (other_flag, waker) = fresh();
        assert!(
            other
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
        assert_eq!(cancel.0.wakers.borrow().len(), 2);
        // L'annulation réveille le DERNIER réveil de chaque attente.
        cancel.cancel();
        assert!(last.unwrap().0.load(std::sync::atomic::Ordering::SeqCst));
        assert!(other_flag.0.load(std::sync::atomic::Ordering::SeqCst));
        assert!(matches!(
            waiting.as_mut().poll(&mut Context::from_waker(&fresh().1)),
            Poll::Ready(())
        ));
    }

    #[test]
    fn une_attente_lachee_retire_son_reveil() {
        let cancel = Cancel::new();
        {
            let mut waiting = pin!(cancel.cancelled());
            assert!(
                waiting
                    .as_mut()
                    .poll(&mut Context::from_waker(&fresh().1))
                    .is_pending()
            );
            assert_eq!(cancel.0.wakers.borrow().len(), 1);
        }
        assert!(cancel.0.wakers.borrow().is_empty());
    }
}

#[cfg(test)]
mod tests_profil {
    use super::*;

    #[test]
    fn le_defaut_ne_promet_rien() {
        // Le défaut d'un transport qui ne se déclare pas : aucune garantie annoncée.
        let p = ProfilRuntime::minimal();
        assert!(!p.minuteur && !p.annulation && !p.supervision);
        assert_eq!(p.isolation, Isolation::Aucune);
        for b in [
            Besoin::Delai,
            Besoin::Annulation,
            Besoin::IsolationDure,
            Besoin::Supervision,
        ] {
            assert!(
                p.exiger(b).is_err(),
                "{b:?} ne doit pas être tenu par défaut"
            );
        }
    }

    #[test]
    fn un_hote_natif_honore_tout() {
        let p = ProfilRuntime::natif();
        for b in [
            Besoin::Delai,
            Besoin::Annulation,
            Besoin::IsolationDure,
            Besoin::Supervision,
        ] {
            assert_eq!(p.exiger(b), Ok(()), "{b:?}");
        }
    }

    #[test]
    fn une_page_sans_minuteur_refuse_un_delai_mais_garde_lannulation() {
        let p = ProfilRuntime::wasm_sans_minuteur();
        // Le cœur de #285 : jamais de délai annoncé mais inapplicable.
        assert_eq!(p.exiger(Besoin::Delai), Err(Refus::SansMinuteur));
        // L'annulation réveille SANS horloge : elle reste honnête.
        assert_eq!(p.exiger(Besoin::Annulation), Ok(()));
    }

    #[test]
    fn catch_unwind_ne_vaut_pas_une_isolation_dure() {
        let p = ProfilRuntime::wasm_sans_minuteur();
        assert_eq!(
            p.exiger(Besoin::IsolationDure),
            Err(Refus::IsolationInsuffisante(Isolation::CatchUnwind)),
            "catch_unwind ne stoppe ni une boucle synchrone ni panic=abort"
        );
        assert_eq!(p.exiger(Besoin::Supervision), Err(Refus::SansSupervision));
    }
}

#[cfg(test)]
mod profil_isolation_tests {
    use super::*;

    /// Sur une cible où la panique n'est pas rattrapable (wasm32 `panic=abort`),
    /// `catch_unwind` ne protège rien : le profil ne doit PAS l'annoncer.
    #[test]
    fn wasm_sans_minuteur_n_annonce_pas_catch_unwind_si_la_panique_n_est_pas_rattrapable() {
        let p = ProfilRuntime::wasm_sans_minuteur_avec(false);
        assert_eq!(p.isolation, Isolation::Aucune);
    }

    /// Témoin : avec une panique rattrapable, `catch_unwind` reste annoncé.
    #[test]
    fn wasm_sans_minuteur_garde_catch_unwind_si_la_panique_est_rattrapable() {
        let p = ProfilRuntime::wasm_sans_minuteur_avec(true);
        assert_eq!(p.isolation, Isolation::CatchUnwind);
    }
}
