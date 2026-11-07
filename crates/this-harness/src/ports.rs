//! Les trois ports par lesquels le harnais touche le monde.
//!
//! Le harnais ne fait aucune entrée/sortie lui-même : il **appelle**. Un crate
//! qui décide de son transport est un crate qu'on ne peut pas brancher deux
//! fois — dans une page et dans une coquille native, par exemple.

use std::cell::Cell;
use std::rc::Rc;

use async_trait::async_trait;

use crate::event::Outcome;
use crate::wire::{Failure, Reply, Request, ToolCall, ToolSpec};

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
/// Un `bool` dans un `Rc<Cell>` suffit : la boucle est sur un seul fil. Un
/// transport qui peut vraiment interrompre (couper une connexion) le consulte
/// lui-même ; sinon, le harnais s'arrête à la frontière suivante et le dit.
#[derive(Clone, Default)]
pub struct Cancel(Rc<Cell<bool>>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.set(true);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.get()
    }
    /// Remis à zéro au début d'un run : une annulation ne vaut que pour elle.
    pub fn reset(&self) {
        self.0.set(false);
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

#[async_trait(?Send)]
impl Toolbox for Toolboxes {
    fn specs(&self) -> Vec<ToolSpec> {
        self.parts.iter().flat_map(|part| part.specs()).collect()
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
            if part.specs().iter().any(|spec| spec.name == call.name) {
                return part.call_with_cancel(call, cancel).await;
            }
        }
        Outcome::refused(
            format!("outil inconnu : {}", call.name),
            format!("aucun toolbox ne déclare « {} »", call.name),
        )
    }
}
