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
use this_agent::evidence::Declaration;

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

/// Ce que le harnais peut faire faire au noyau de l'application.
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

    /// Exécute un appel et rend ce que le modèle lira, plus l'observation.
    async fn call(&self, call: &ToolCall) -> Outcome;
}

/// Un [`Toolbox`] fait de plusieurs : l'hôte apporte ses outils, un module
/// apporte les siens, et le harnais ne sait pas lesquels viennent d'où.
///
/// C'est la pièce qui rend le harnais **composable** côté outils : un module
/// installé peut ajouter une capacité sans qu'on touche au `Toolbox` de
/// l'application, et sans que la boucle change d'une ligne.
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

    /// Chaque part ne déclare que **ses** outils, et seulement ceux qu'elle exécute : une
    /// déclaration est gardée si sa part est la première, dans l'ordre de composition, à
    /// exposer ce nom dans ses `specs()` — la même règle que le routage de l'appel. Un module
    /// qui ne fournit que `memo` ne peut donc pas déclarer `reads("install_module")`, ni
    /// redéclarer un outil qu'une part précédente expose déjà. Le reste est ignoré.
    fn declarations(&self) -> Vec<Declaration> {
        let mut proprietaire: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (i, part) in self.parts.iter().enumerate() {
            for spec in part.specs() {
                proprietaire.entry(spec.name).or_insert(i);
            }
        }
        self.parts
            .iter()
            .enumerate()
            .flat_map(|(i, part)| {
                let proprietaire = &proprietaire;
                part.declarations()
                    .into_iter()
                    .filter(move |x| proprietaire.get(&x.tool) == Some(&i))
            })
            .collect()
    }

    async fn call(&self, call: &ToolCall) -> Outcome {
        // Le premier qui déclare l'outil l'exécute : deux modules qui
        // exposeraient le même nom ne se disputent pas l'appel, l'ordre de
        // composition décide — et il est visible dans l'application.
        for part in &self.parts {
            if part.specs().iter().any(|spec| spec.name == call.name) {
                return part.call(call).await;
            }
        }
        Outcome::refused(
            format!("outil inconnu : {}", call.name),
            format!("aucun toolbox ne déclare « {} »", call.name),
        )
    }
}
