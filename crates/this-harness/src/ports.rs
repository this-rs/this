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
    /// Consommé en **fin** de run (jamais au début) : une annulation ne vaut que pour le run
    /// qui la voit, et une annulation arrivée entre deux runs est vue dès le premier tour du suivant.
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

    /// Ce que chaque outil fait au monde (écrit, lit, note) et les mots qui le désignent
    /// dans un récit : la preuve s'en sert pour confronter « j'ai déployé » à `deploy_app`
    /// refusé ([`Declaration`]). Par défaut aucune : la preuve devine d'après les noms
    /// (`install_module`, `row_create`, `remember`…), comme avant.
    fn declarations(&self) -> Vec<Declaration> {
        Vec::new()
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
#[async_trait(?Send)]
pub trait Recorder {
    /// Une étape vient d'être mesurée. `run` la porte déjà : la boucle a appelé
    /// `Run::record` juste avant, et `run.steps` la contient — l'hôte qui veut
    /// écrire le run **partiel** (son `_data`, son décompte de tours) l'a sous
    /// la main sans rien reconstruire.
    async fn step(&self, run_id: &str, run: &Run, step: &Step);

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
    }
}
