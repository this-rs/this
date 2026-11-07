//! # La boucle d'agent, comme donnée
//!
//! Un agent n'est pas une boucle `for` dans un callback d'interface : c'est un
//! **run** — un objectif, des étapes horodatées, une raison d'arrêt nommée.
//! Ce crate ne fait aucune entrée/sortie : il ne connaît ni HTTP, ni DOM, ni le
//! modèle. Il décide seulement s'il faut continuer, et pourquoi on s'arrête.
//!
//! ```
//! use this_agent::{Decision, Goal, Run, Step, Stop};
//!
//! let mut run = Run::new(Goal::new("installe le module conversation"), 6);
//! run.record(Step::tool("install_module", 1840).refused("même version"));
//! // Un refus n'est pas une fin : on relance avec le motif.
//! assert_eq!(run.after_turn(true, false), Decision::Continue);
//! ```
//!
//! ## Pourquoi un crate, et pas un fichier
//!
//! La même boucle tourne dans l'atelier (wasm), dans la coquille Tauri et dans
//! un harnais natif. Écrite trois fois, elle divergera trois fois. Écrite ici,
//! elle se teste une fois — et `cargo test` remplace la foi.

pub mod cite;
pub mod evidence;
pub mod graph;
pub mod masque;
pub mod playbook;
pub mod substrate;
pub mod surface;
pub mod untrusted;

use serde::{Deserialize, Serialize};

/// Pourquoi le run s'est arrêté. Jamais « parce que la boucle est finie ».
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    /// Le modèle a conclu et rien ne contredit l'objectif.
    Done,
    /// Le budget de tours est épuisé sans conclusion.
    Budget,
    /// Une action a été refusée et le modèle n'a pas su rebondir.
    Refused(String),
    /// Le transport ou l'exécution a échoué.
    Failed(String),
    /// L'utilisateur a annulé.
    Cancelled,
    /// La coquille a suspendu le run (app en arrière-plan, flux coupé).
    Interrupted,
}

impl Stop {
    /// Une phrase pour l'utilisateur — un arrêt se dit, il ne se constate pas.
    pub fn phrase(&self) -> String {
        match self {
            Stop::Done => "terminé".to_string(),
            Stop::Budget => {
                "j'ai atteint la limite du run (tours ou temps) sans conclure — dis-moi de continuer".to_string()
            }
            Stop::Refused(raison) => format!("bloqué : {raison}"),
            Stop::Failed(erreur) => format!("échec : {erreur}"),
            Stop::Cancelled => "annulé".to_string(),
            Stop::Interrupted => "interrompu (app suspendue ou flux coupé)".to_string(),
        }
    }

    pub fn est_fin(&self) -> bool {
        matches!(self, Stop::Done)
    }
}

/// Ce que l'utilisateur veut, et à quoi on reconnaîtra que c'est fait.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub text: String,
    /// Facultatif : sans condition, « fini » reste l'avis du modèle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<String>,
}

impl Goal {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            done_when: None,
        }
    }

    pub fn done_when(mut self, condition: impl Into<String>) -> Self {
        self.done_when = Some(condition.into());
        self
    }
}

/// Une étape : ce qui a été tenté, combien de temps, et ce que l'état est devenu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub tool: String,
    pub ms: u64,
    #[serde(default)]
    pub refused: bool,
    /// Pourquoi l'outil a refusé — distinct de `resume`, qui est ce qu'on affiche.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motif: Option<String>,
    /// Rang de l'étape dans le run : le journal devient ordonnable et rejouable.
    #[serde(default)]
    pub index: u32,
    /// Horodatage relatif au début du run, en millisecondes.
    #[serde(default)]
    pub at_ms: u64,
    #[serde(default)]
    pub failure: Option<String>,
    /// Empreinte de l'état avant et après : c'est l'**observation**, pas la
    /// parole de l'outil. Deux empreintes égales = rien n'a bougé.
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
    #[serde(default)]
    pub resume: String,
    /// Empreinte (SHA-256, hex) de la sortie **complète** de l'outil, calculée par la boucle
    /// avant toute troncature. `resume` est borné pour l'affichage ; une empreinte prise sur
    /// lui ne prouverait que les premiers caractères. Absente des journaux anciens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_hash: Option<String>,
}

impl Step {
    pub fn tool(name: impl Into<String>, ms: u64) -> Self {
        Self {
            tool: name.into(),
            ms,
            refused: false,
            motif: None,
            index: 0,
            at_ms: 0,
            failure: None,
            before: String::new(),
            after: String::new(),
            resume: String::new(),
            out_hash: None,
        }
    }

    pub fn refused(mut self, raison: impl Into<String>) -> Self {
        let raison = raison.into();
        self.refused = true;
        self.motif = Some(raison.clone());
        if self.resume.is_empty() {
            self.resume = raison;
        }
        self
    }

    /// L'outil a **échoué** : il a planté, ou n'a pas pu faire ce qu'on lui demandait. Ce n'est
    /// ni un refus (une décision) ni un succès : la cause se garde, et le harnais la relit.
    pub fn failed(mut self, erreur: impl Into<String>) -> Self {
        let erreur = erreur.into();
        if self.resume.is_empty() {
            self.resume = erreur.clone();
        }
        self.failure = Some(erreur);
        self
    }

    pub fn resume(mut self, texte: impl Into<String>) -> Self {
        self.resume = texte.into();
        self
    }

    pub fn state(mut self, before: impl Into<String>, after: impl Into<String>) -> Self {
        self.before = before.into();
        self.after = after.into();
        self
    }

    /// L'empreinte de la sortie complète (voir [`Step::out_hash`]).
    pub fn out_hash(mut self, empreinte: impl Into<String>) -> Self {
        self.out_hash = Some(empreinte.into());
        self
    }

    /// L'état a-t-il bougé ? Un outil qui répond « ok » sans rien changer ment.
    pub fn a_change(&self) -> bool {
        !self.before.is_empty() && self.before != self.after
    }
}

/// Ce que la boucle doit faire après un tour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Continue,
    Halt(Stop),
}

/// Un run : l'objectif, les étapes, et la raison d'arrêt (quand elle existe).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub goal: Goal,
    pub steps: Vec<Step>,
    pub tours: u32,
    pub max_tours: u32,
    /// Relances après un refus, et relances après un tour muet : deux causes,
    /// deux compteurs — sinon un silence consomme le crédit d'un refus.
    pub relances_refus: u32,
    pub relances_muet: u32,
    pub max_relances: u32,
    /// Budget de temps : dans une webview, c'est l'horloge qui s'épuise.
    #[serde(default)]
    pub budget_ms: Option<u64>,
    #[serde(default)]
    pub ecoule_ms: u64,
    /// Première étape du tour en cours : un refus du tour précédent ne doit plus
    /// peser sur la décision d'aujourd'hui.
    #[serde(default)]
    pub tour_debut: usize,
    /// Le texte final du tour en cours, confié par la boucle avant la décision
    /// ([`Run::said`]). Il est consommé par la décision : un récit ne vaut que pour son tour.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recit: String,
    /// Rappels « ton récit n'est pas prouvé » déjà envoyés : un seul par run.
    #[serde(default)]
    pub rappels_recit: u32,
    /// Le rappel à pousser au modèle quand la décision relance pour un récit non prouvé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rappel: Option<String>,
    /// Ce que l'hôte déclare de ses outils (effet, mots du récit) : voir
    /// [`evidence::Declaration`]. Vide, la preuve devine d'après les noms des outils.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declarations: Vec<evidence::Declaration>,
    /// Rangs des étapes dont l'effet est **inconnu** : un outil abandonné (plafond dépassé,
    /// sourd à l'annulation) a pu finir après coup. Son étape porte « délai dépassé » pour
    /// que le modèle ne la croie pas réussie, mais ce n'est ni un échec prouvé, ni un refus
    /// à rattraper, ni une preuve : la décision ne la lit pas (voir [`Run::effet_inconnu`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inconnues: Vec<usize>,
    pub stop: Option<Stop>,
}

/// La politique d'un run : ce qui décide « on continue » ou « on s'arrête, et pourquoi ».
///
/// C'est le port qui rend le harnais composable. Les règles ci-dessous
/// (`DefaultPolicy`) ne sont pas *le* comportement du crate : elles sont **une**
/// politique, remplaçable par un module qu'un agent écrit lui-même — « relance
/// deux fois puis abandonne » est un choix, pas une loi.
///
/// `&self` et pas `&mut self` : une politique qui garde un état le porte dans sa
/// propre cellule (le run, lui, porte les compteurs). Le harnais ne doit pas
/// imposer sa façon de partager l'état — même raison que pour ses autres ports.
pub trait Policy {
    /// Décide de la suite après un tour.
    ///
    /// `run` est **mutable** exprès : c'est lui qui porte l'état (compteurs de
    /// relance, frontière du tour), pas la politique. Une politique qui devrait
    /// écrire dans un état extérieur au run le ferait au détriment de la
    /// rejouabilité du journal.
    fn after_turn(&self, run: &mut Run, spoke: bool, called: bool) -> Decision;
}

/// Les règles appliquées depuis le début : un tour qui appelle un outil
/// continue, un tour muet ou refusé relance dans la limite des crédits, un tour
/// qui conclut termine, et un budget épuisé s'arrête en le disant.
pub struct DefaultPolicy;

impl Policy for DefaultPolicy {
    fn after_turn(&self, run: &mut Run, spoke: bool, called: bool) -> Decision {
        run.decide_default(spoke, called)
    }
}

impl Run {
    pub fn new(goal: Goal, max_tours: u32) -> Self {
        Self {
            goal,
            steps: Vec::new(),
            tours: 0,
            max_tours,
            relances_refus: 0,
            relances_muet: 0,
            max_relances: 2,
            budget_ms: None,
            ecoule_ms: 0,
            tour_debut: 0,
            recit: String::new(),
            rappels_recit: 0,
            rappel: None,
            declarations: Vec::new(),
            inconnues: Vec::new(),
            stop: None,
        }
    }

    /// Les déclarations d'outils de l'hôte ([`evidence::Declaration`]) ; elles **remplacent**
    /// les précédentes : la boucle les relit à chaque tour, un module installé entre-temps
    /// apporte les siennes.
    pub fn declare(&mut self, declarations: impl IntoIterator<Item = evidence::Declaration>) {
        self.declarations = declarations.into_iter().collect();
    }

    /// L'étape de rang `index` a un effet **inconnu** (outil abandonné) : la décision ne la
    /// compte ni comme refus ou échec à rattraper, ni comme preuve. Un récit qui s'appuie
    /// sur elle reçoit un rappel qui demande de VÉRIFIER l'effet, pas de refaire l'action.
    pub fn effet_inconnu(&mut self, index: usize) {
        if !self.inconnues.contains(&index) {
            self.inconnues.push(index);
        }
    }

    /// L'étape a-t-elle un effet inconnu ?
    pub fn est_inconnue(&self, etape: &Step) -> bool {
        self.inconnues.contains(&(etape.index as usize))
    }

    /// Les étapes dont l'effet est connu : ce que la décision confronte au récit.
    fn connues(&self) -> Vec<Step> {
        self.steps
            .iter()
            .filter(|etape| !self.est_inconnue(etape))
            .cloned()
            .collect()
    }

    pub fn record(&mut self, mut step: Step) {
        step.index = self.steps.len() as u32;
        if step.at_ms == 0 {
            step.at_ms = self.ecoule_ms;
        }
        self.ecoule_ms += step.ms;
        self.steps.push(step);
    }

    /// Confie à la décision le texte final du tour qui s'achève. La boucle l'appelle juste
    /// avant [`Run::after_turn_with`] ; sans lui, le récit n'est pas confronté aux preuves
    /// (mais un refus non rattrapé empêche toujours `Done`).
    pub fn said(&mut self, texte: impl Into<String>) {
        self.recit = texte.into();
    }

    /// Le rappel que la dernière décision `Continue` demande de pousser au modèle, s'il y en a
    /// un. Le prendre le consomme.
    pub fn take_rappel(&mut self) -> Option<String> {
        self.rappel.take()
    }

    /// Ce que les faits disent du run : voir [`evidence::verdict`].
    /// Les étapes à l'effet inconnu ([`Run::effet_inconnu`]) n'y comptent pas.
    pub fn verdict(&self) -> evidence::Verdict {
        evidence::verdict_with(
            &self.connues(),
            self.stop == Some(Stop::Done),
            &self.declarations,
        )
    }

    /// Un budget de temps, en plus du budget de tours.
    pub fn budget(&mut self, ms: u64) -> &mut Self {
        self.budget_ms = Some(ms);
        self
    }

    /// Étapes vues depuis le début du tour en cours.
    fn du_tour(&self) -> &[Step] {
        &self.steps[self.tour_debut.min(self.steps.len())..]
    }

    /// La dernière étape du tour qui n'a rien produit sur l'état.
    pub fn dernier_sans_effet(&self) -> Option<&Step> {
        // Seules les étapes qui portent une observation peuvent être jugées :
        // sans empreinte, on ne sait pas si l'état a bougé, et conclure « rien
        // n'a changé » serait une affirmation sans mesure.
        self.du_tour().iter().rev().find(|etape| {
            !self.est_inconnue(etape) && !etape.before.is_empty() && !etape.a_change()
        })
    }

    /// Le dernier échec d'outil **du tour en cours**. Comme pour les refus, un échec ancien ne
    /// doit pas condamner chaque tour suivant.
    pub fn derniere_en_echec(&self) -> Option<&Step> {
        self.du_tour()
            .iter()
            .rev()
            .find(|etape| !self.est_inconnue(etape) && etape.failure.is_some())
    }

    /// Le dernier refus **du tour en cours**. Cherché sur tout le run, un refus
    /// ancien condamnait chaque tour suivant.
    pub fn derniere_refusee(&self) -> Option<&Step> {
        self.du_tour()
            .iter()
            .rev()
            .find(|etape| etape.refused && !self.est_inconnue(etape))
    }

    /// Décide de la suite, après un tour du modèle.
    ///
    /// `a_parle` : le modèle a produit du texte ; `a_appele` : il a demandé des
    /// outils. Le silence n'est pas une fin, et un refus n'en est pas une non
    /// plus — les deux relancent, dans la limite des relances accordées.
    pub fn after_turn(&mut self, a_parle: bool, a_appele: bool) -> Decision {
        self.after_turn_with(&DefaultPolicy, a_parle, a_appele)
    }

    /// La même décision, avec la politique de l'appelant.
    ///
    /// Le harnais passe par ici : remplacer la politique ne touche ni la boucle,
    /// ni les transports, ni les outils.
    pub fn after_turn_with(
        &mut self,
        policy: &dyn Policy,
        a_parle: bool,
        a_appele: bool,
    ) -> Decision {
        let decision = policy.after_turn(self, a_parle, a_appele);
        // La fin s'écrit ICI, pas dans la politique : une politique qui rend
        // `Halt` sans tenir le journal laisserait un run « en cours » arrêté —
        // exactement le mensonge que ce crate interdit (`halt` reste idempotent
        // pour la politique par défaut, qui l'a déjà posé).
        if let Decision::Halt(stop) = &decision {
            self.stop = Some(stop.clone());
        }
        decision
    }

    /// Le corps des règles par défaut, déplacé tel quel.
    fn decide_default(&mut self, a_parle: bool, a_appele: bool) -> Decision {
        self.tours += 1;
        // Le tour qui vient de s'achever ne doit plus être relu au suivant.
        let fin_du_tour = self.steps.len();
        // Un récit ne vaut que pour son tour ; un rappel non pris est périmé.
        let recit = std::mem::take(&mut self.recit);
        self.rappel = None;

        if let Some(stop) = &self.stop {
            return Decision::Halt(stop.clone());
        }
        // Le temps avant les tours : une webview meurt d'horloge, pas de compteur.
        if let Some(budget) = self.budget_ms
            && self.ecoule_ms >= budget
        {
            self.tour_debut = fin_du_tour;
            return self.halt(Stop::Budget);
        }
        if self.tours >= self.max_tours {
            self.tour_debut = fin_du_tour;
            return self.halt(Stop::Budget);
        }
        if a_appele {
            self.tour_debut = fin_du_tour;
            return Decision::Continue;
        }

        let refus = self
            .derniere_refusee()
            .and_then(|etape| etape.motif.clone());
        // Un échec d'outil est une cause à corriger, au même titre qu'un refus : sans cette
        // ligne, un tour conclu après un outil planté passait pour un succès.
        let echec = self
            .derniere_en_echec()
            .and_then(|etape| etape.failure.clone());
        let muet = !a_parle;
        // Une étape qui n'a rien changé n'est pas un progrès : la relance doit
        // exiger un changement, sinon on tourne en rond en croyant avancer.
        let sans_effet = self.dernier_sans_effet().is_some();

        if refus.is_some() || echec.is_some() || muet || sans_effet {
            let (compteur, max) = match (refus.is_some() || echec.is_some(), muet) {
                (true, _) => (self.relances_refus, self.max_relances),
                (false, true) => (self.relances_muet, self.max_relances),
                _ => (self.relances_refus, self.max_relances),
            };
            self.tour_debut = fin_du_tour;
            if compteur < max {
                if refus.is_some() || echec.is_some() {
                    self.relances_refus += 1;
                } else {
                    self.relances_muet += 1;
                }
                return Decision::Continue;
            }
            return self.halt(match refus {
                Some(raison) => Stop::Refused(raison),
                None if echec.is_some() => Stop::Failed(format!(
                    "l'outil a échoué : {}",
                    echec.clone().unwrap_or_default()
                )),
                None if muet => Stop::Failed("le modèle n'a rendu ni texte ni outil".to_string()),
                None => Stop::Failed("les actions n'ont rien changé".to_string()),
            });
        }

        // Le tour n'a ni refus ni échec — mais le RUN peut en avoir : un refus au tour N
        // (tour qui appelait, donc `Continue`) suivi d'un tour texte seul « c'est fait »
        // passait pour une conclusion, la frontière du tour ayant effacé le refus.
        self.tour_debut = fin_du_tour;
        // Une étape à l'effet inconnu (outil abandonné) n'est ni un refus non rattrapé, ni une
        // preuve : les faits se lisent sur les autres étapes.
        let connues = self.connues();
        let inconnue = self
            .steps
            .iter()
            .rev()
            .find(|etape| self.est_inconnue(etape))
            .cloned();
        let en_suspens = evidence::unresolved_with(&connues, &self.declarations).cloned();
        let non_prouve =
            evidence::unproven_with(&recit, &connues, &self.declarations).or_else(|| {
                // Retirée des faits, l'écriture abandonnée ne compte plus comme « tentée » : un
                // récit au passif qui ne s'appuie que sur elle passerait. Relu sur tout le run,
                // il n'est retenu que s'il vise justement l'étape inconnue.
                inconnue.as_ref()?;
                evidence::unproven_with(&recit, &self.steps, &self.declarations)
                    .filter(|u| u.etape.as_ref().is_some_and(|e| self.est_inconnue(e)))
            });
        if let Some(mut non_prouve) = non_prouve {
            // Le récit s'appuie sur l'étape inconnue (il la vise, ou rien d'autre ne le prouve) :
            // le rappel demande de vérifier l'effet, pas de refaire l'action.
            let outil_inconnu = match &non_prouve.etape {
                Some(etape) if self.est_inconnue(etape) => Some(etape.tool.clone()),
                Some(_) => None,
                None => inconnue.as_ref().map(|etape| etape.tool.clone()),
            };
            if non_prouve
                .etape
                .as_ref()
                .is_some_and(|etape| self.est_inconnue(etape))
            {
                non_prouve.etape = None;
            }
            if self.rappels_recit == 0 {
                self.rappels_recit += 1;
                let rappel = match &outil_inconnu {
                    Some(outil) => non_prouve.rappel_effet_inconnu(outil),
                    None => non_prouve.rappel(),
                };
                self.rappel = Some(match &en_suspens {
                    Some(etape) if !rappel.contains(&format!("`{}`", etape.tool)) => format!(
                        "{rappel} Pour mémoire, `{}` n'a pas abouti dans ce run : {}.",
                        etape.tool,
                        etape
                            .motif
                            .clone()
                            .or_else(|| etape.failure.clone())
                            .unwrap_or_default()
                    ),
                    _ => rappel,
                });
                return Decision::Continue;
            }
            // Le rappel a été envoyé et le récit reste non prouvé. Un refus en suspens se dit
            // plus bas ; sinon, l'écriture contredite (refusée, jamais reprise) dit son motif.
            // « Récit non vérifié » n'est un échec que si une écriture refusée ou en échec
            // reste en plan dans le run : le récit couvre alors peut-être ce qui a raté. Sans
            // écriture en plan (en particulier sans aucune écriture), le récit non prouvé
            // n'est contredit par aucun fait : le run finit en `Done` et le verdict
            // `Unverified` le dit — une explication honnête ne devient pas un échec.
            if en_suspens.is_none() {
                let en_plan = !evidence::unrepaired_with(&connues, &self.declarations).is_empty();
                match &non_prouve.etape {
                    Some(etape) if etape.refused => {
                        return self.halt(Stop::Refused(
                            etape.motif.clone().unwrap_or(etape.resume.clone()),
                        ));
                    }
                    Some(etape) => {
                        return self.halt(Stop::Failed(format!(
                            "l'outil a échoué : {}",
                            etape.failure.clone().unwrap_or(etape.resume.clone())
                        )));
                    }
                    None if en_plan => {
                        return self.halt(Stop::Failed(format!(
                            "récit non vérifié : « {} » — {}, même après un rappel",
                            non_prouve.phrase, non_prouve.manque
                        )));
                    }
                    None => {}
                }
            }
        }
        if let Some(etape) = en_suspens {
            return self.halt(match (&etape.motif, &etape.failure) {
                (Some(motif), _) if etape.refused => Stop::Refused(motif.clone()),
                (_, Some(cause)) => Stop::Failed(format!("l'outil a échoué : {cause}")),
                _ => Stop::Refused(etape.resume.clone()),
            });
        }

        // Un tour qui aboutit remet les crédits de relance à zéro.
        self.relances_refus = 0;
        self.relances_muet = 0;
        self.halt(Stop::Done)
    }

    /// Un run ne ment jamais sur sa fin : la décision s'écrit dans le run.
    fn halt(&mut self, stop: Stop) -> Decision {
        self.stop = Some(stop.clone());
        Decision::Halt(stop)
    }

    /// Clôt le run avec une raison explicite.
    pub fn finish(&mut self, stop: Stop) -> Stop {
        self.stop = Some(stop.clone());
        stop
    }

    /// La durée totale des étapes mesurées.
    pub fn duree_totale(&self) -> u64 {
        self.steps.iter().map(|etape| etape.ms).sum()
    }

    pub fn resume_pour_l_utilisateur(&self) -> String {
        match &self.stop {
            Some(stop) => stop.phrase(),
            None => "en cours".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Run {
        Run::new(Goal::new("installe un module"), 6)
    }

    #[test]
    fn un_tour_avec_outil_continue() {
        let mut r = run();
        assert_eq!(r.after_turn(false, true), Decision::Continue);
    }

    #[test]
    fn une_conclusion_parle_et_n_appelle_plus_termine() {
        let mut r = run();
        r.record(Step::tool("list_modules", 3).state("a", "b"));
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
    }

    #[test]
    fn un_refus_ne_termine_pas_le_run() {
        let mut r = run();
        r.record(Step::tool("install_module", 1800).refused("même version"));
        // Le modèle parle mais n'appelle plus : sans cette règle, le run
        // s'arrêtait sur un échec, en silence.
        assert_eq!(r.after_turn(true, false), Decision::Continue);
    }

    #[test]
    fn un_refus_repete_finit_par_bloquer_et_le_dit() {
        let mut r = run();
        // Un refus ne vaut que pour le tour qui l'a produit : chaque tentative
        // se réenregistre, comme dans la vraie boucle.
        r.record(Step::tool("install_module", 10).refused("même version"));
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        r.record(Step::tool("install_module", 10).refused("même version"));
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        r.record(Step::tool("install_module", 10).refused("même version"));
        match r.after_turn(true, false) {
            Decision::Halt(Stop::Refused(raison)) => assert_eq!(raison, "même version"),
            autre => panic!("attendu Refused, obtenu {autre:?}"),
        }
    }

    #[test]
    fn un_tour_muet_relance_puis_echoue_en_le_disant() {
        let mut r = run();
        assert_eq!(r.after_turn(false, false), Decision::Continue);
        assert_eq!(r.after_turn(false, false), Decision::Continue);
        assert!(matches!(
            r.after_turn(false, false),
            Decision::Halt(Stop::Failed(_))
        ));
    }

    #[test]
    fn le_budget_est_une_raison_nommee() {
        let mut r = Run::new(Goal::new("boucle"), 2);
        r.after_turn(false, true);
        assert_eq!(r.after_turn(false, true), Decision::Halt(Stop::Budget));
    }

    // ---- un échec d'outil est vu ----

    /// Un outil qui plante, puis un modèle qui conclut : ce n'est pas un succès. Le run relance,
    /// et quand les relances sont épuisées, il s'arrête en disant la cause.
    #[test]
    fn un_echec_d_outil_relance_puis_echoue_en_disant_la_cause() {
        let mut r = Run::new(Goal::new("installe"), 20);
        r.max_relances = 1;
        r.record(Step::tool("install_module", 5).failed("module introuvable"));
        assert_eq!(
            r.after_turn(true, false),
            Decision::Continue,
            "première relance"
        );

        r.record(Step::tool("install_module", 5).failed("module introuvable"));
        match r.after_turn(true, false) {
            Decision::Halt(Stop::Failed(cause)) => {
                assert!(
                    cause.contains("module introuvable"),
                    "la cause est dite : {cause}"
                )
            }
            autre => panic!("attendu un arrêt sur échec, obtenu {autre:?}"),
        }
    }

    /// Un échec d'un tour ancien ne condamne pas les tours suivants — une fois rattrapé.
    /// (Avant la vague 2, ce test concluait `Done` SANS reprise : c'était le défaut.)
    #[test]
    fn un_echec_ancien_ne_condamne_pas_le_tour_suivant() {
        let mut r = Run::new(Goal::new("installe"), 20);
        r.record(Step::tool("install_module", 5).failed("réseau"));
        assert_eq!(r.after_turn(true, false), Decision::Continue);

        // Le tour suivant reprend l'action avec succès : un modèle qui conclut conclut.
        r.record(Step::tool("install_module", 5).state("v1", "v2"));
        assert!(r.derniere_en_echec().is_none());
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
    }

    /// Le cas relu en vague 1 : un échec non rattrapé, puis un tour texte seul, ne finit pas
    /// en `Done`.
    #[test]
    fn un_echec_non_rattrape_puis_texte_seul_n_est_pas_done() {
        let mut r = Run::new(Goal::new("installe"), 20);
        r.record(Step::tool("install_module", 5).failed("réseau"));
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Failed("l'outil a échoué : réseau".into()))
        );
    }

    #[test]
    fn refus_puis_c_est_fait_relance_avec_un_rappel() {
        let mut r = run();
        // Tour N : le modèle appelle, l'outil refuse — un tour qui appelle continue.
        r.record(Step::tool("install_module", 10).refused("même version"));
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        // Tour N+1 : texte seul, il prétend avoir fini.
        r.said("C'est fait !");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        let rappel = r.take_rappel().expect("un rappel à pousser");
        assert!(rappel.starts_with(evidence::RAPPEL));
        assert!(rappel.contains("même version"));
        assert_eq!(r.take_rappel(), None);
    }

    #[test]
    fn refus_puis_c_est_fait_sans_recit_transmis_bloque_au_lieu_de_done() {
        // Sans `said` (boucle pas encore branchée), le refus du run suffit à refuser `Done`.
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Refused("même version".into()))
        );
    }

    /// Un hôte qui déclare `deploy_app` obtient ce qu'`install_module` obtient sans rien
    /// déclarer : refus, une écriture sans rapport réussie, « j'ai déployé » ⇒ rappel.
    #[test]
    fn un_outil_declare_par_l_hote_est_controle_comme_install_module() {
        let scenario = |outil: &str, recit: &str, declarations: Vec<evidence::Declaration>| {
            let mut r = run();
            r.declare(declarations);
            r.record(Step::tool(outil, 10).refused("quota atteint"));
            r.record(Step::tool("row_create", 3).state("0", "1"));
            assert_eq!(r.after_turn(false, true), Decision::Continue);
            r.said(recit);
            (r.after_turn(true, false), r.take_rappel())
        };
        let (decision, rappel) = scenario("install_module", "J'ai installé le module.", vec![]);
        assert_eq!(decision, Decision::Continue);
        assert!(rappel.unwrap().contains("`install_module`"));

        let deploy = evidence::Declaration::writes("deploy_app").words(["déployer", "deploy"]);
        let (decision, rappel) =
            scenario("deploy_app", "J'ai déployé l'application.", vec![deploy]);
        assert_eq!(decision, Decision::Continue);
        let rappel = rappel.expect("un rappel");
        assert!(rappel.starts_with(evidence::RAPPEL));
        assert!(rappel.contains("`deploy_app`") && rappel.contains("quota atteint"));

        // Sans déclaration, le même récit passe : c'est le trou que la déclaration ferme.
        let (decision, rappel) = scenario("deploy_app", "J'ai déployé l'application.", vec![]);
        assert_eq!(decision, Decision::Halt(Stop::Done));
        assert_eq!(rappel, None);
    }

    #[test]
    fn un_seul_rappel_de_recit_par_run() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.said("C'est fait.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        // Il insiste : pas de second rappel, et le refus en suspens interdit `Done`.
        r.said("C'est fait, vraiment.");
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Refused("même version".into()))
        );
        assert_eq!(r.rappels_recit, 1);
        assert_eq!(r.take_rappel(), None);
    }

    /// Y09 BLOQUANT 1 (remplace W12 MOY 5) : sans aucune écriture dans le run, un récit qui
    /// reste non prouvé après le rappel n'est contredit par aucun fait. Le run finit en
    /// `Done`, et c'est le verdict `Unverified` qui le dit — pas un `Failed` qui ferait
    /// d'une réponse peut-être honnête un échec.
    #[test]
    fn un_seul_rappel_puis_done_non_verifie_sans_aucune_ecriture() {
        let mut r = run();
        r.said("J'ai créé la fiche.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        assert!(r.take_rappel().is_some());
        r.said("J'ai créé la fiche.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.rappels_recit, 1);
        assert_eq!(r.verdict(), evidence::Verdict::Unverified);
    }

    /// Avec une écriture refusée restée en plan (rattrapée pour le run par une autre
    /// écriture, mais jamais reprise par le même outil), le récit non prouvé après le rappel
    /// reste un échec : « récit non vérifié ».
    #[test]
    fn un_seul_rappel_puis_non_verifie_si_une_ecriture_reste_en_plan() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.record(Step::tool("row_create", 3).state("0", "1"));
        r.after_turn(false, true);
        r.said("Je me souviens que tu préfères le thé.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        r.said("Je me souviens que tu préfères le thé.");
        match r.after_turn(true, false) {
            Decision::Halt(Stop::Failed(raison)) => {
                assert!(raison.contains("récit non vérifié"), "{raison}");
                assert!(raison.contains("Je me souviens"), "{raison}");
            }
            autre => panic!("attendu un arrêt non vérifié, obtenu {autre:?}"),
        }
        assert_eq!(r.rappels_recit, 1);
        assert!(r.resume_pour_l_utilisateur().contains("non vérifié"));
    }

    /// Une explication de l'Atelier, sans aucune écriture : ni rappel, ni échec.
    #[test]
    fn une_explication_au_passif_sans_ecriture_termine_sans_rappel() {
        let mut r = run();
        r.said(
            "Un module est installé dans l'espace de travail : il apporte ses tables. \
             La fiche est créée automatiquement à la première saisie.",
        );
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.take_rappel(), None);
        assert_eq!(r.rappels_recit, 0);
        assert_eq!(r.verdict(), evidence::Verdict::Unverified);
    }

    #[test]
    fn un_rappel_puis_une_phrase_honnete_termine() {
        let mut r = run();
        r.said("J'ai créé la fiche.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        r.said("Je n'ai pas créé la fiche : je n'ai appelé aucun outil.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.verdict(), evidence::Verdict::Unverified);
    }

    #[test]
    fn un_rappel_puis_l_outil_reellement_appele_termine() {
        let mut r = run();
        r.said("J'ai créé la fiche.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        r.record(Step::tool("row_create", 3).state("0", "1"));
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        r.said("J'ai créé la fiche.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.verdict(), evidence::Verdict::Proven);
    }

    /// W10#2 : une écriture sans rapport ne blanchit pas un refus que le récit nomme.
    #[test]
    fn une_ecriture_sans_rapport_ne_rattrape_pas_le_refus_nomme() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.record(Step::tool("row_create", 3).state("0", "1"));
        r.after_turn(false, true);
        r.said("C'est fait : j'ai installé le module.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        let rappel = r.take_rappel().unwrap();
        assert!(rappel.contains("`install_module`"), "{rappel}");
        r.said("C'est fait.");
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Refused("même version".into()))
        );
    }

    #[test]
    fn une_ecriture_sans_empreinte_ne_rattrape_pas_un_refus() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.record(Step::tool("row_create", 3));
        r.after_turn(false, true);
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Refused("même version".into()))
        );
    }

    #[test]
    fn une_ecriture_observee_rattrape_un_refus_quand_le_recit_dit_ce_qui_a_ete_fait() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.record(Step::tool("row_create", 3).state("0", "1"));
        r.after_turn(false, true);
        r.said("Le module était déjà à jour ; j'ai ajouté la ligne demandée.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.take_rappel(), None);
    }

    #[test]
    fn ecriture_reussie_puis_c_est_fait_termine() {
        let mut r = run();
        r.record(Step::tool("row_create", 10).state("0 ligne", "1 ligne"));
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        r.said("C'est fait : la ligne est créée.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.take_rappel(), None);
        assert_eq!(r.verdict(), evidence::Verdict::Proven);
    }

    #[test]
    fn refus_rattrape_puis_c_est_fait_termine() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.record(Step::tool("install_module", 10).state("v1", "v2"));
        r.after_turn(false, true);
        r.said("C'est fait.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
    }

    #[test]
    fn je_me_souviens_sans_recall_relance_avec_un_rappel() {
        let mut r = run();
        r.said("Je me souviens que tu préfères le thé.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        assert!(r.take_rappel().unwrap().contains("recall"));
    }

    #[test]
    fn je_me_souviens_apres_recall_termine() {
        let mut r = run();
        r.record(Step::tool("recall", 3).resume("préférence : thé"));
        r.after_turn(false, true);
        r.said("Je me souviens que tu préfères le thé.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
    }

    #[test]
    fn negation_apres_refus_ne_rappelle_pas_et_dit_le_blocage() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.said("Je n'ai pas installé le module : il est déjà à la même version.");
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Refused("même version".into()))
        );
        assert_eq!(r.rappels_recit, 0);
    }

    #[test]
    fn une_note_remember_ne_rattrape_pas_un_refus() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        r.after_turn(false, true);
        r.record(Step::tool("remember", 1).state("a", "b"));
        r.after_turn(false, true);
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Refused("même version".into()))
        );
    }

    #[test]
    fn le_recit_ne_vaut_que_pour_son_tour() {
        let mut r = run();
        r.said("C'est fait.");
        // Tour qui appelle : le récit est consommé sans être jugé.
        r.record(Step::tool("graph_query", 1));
        assert_eq!(r.after_turn(true, true), Decision::Continue);
        assert!(r.recit.is_empty());
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
    }

    // ---- Z01 : étape abandonnée (effet inconnu) ------------------------------------

    /// L'étape d'un outil abandonné, telle que la boucle l'enregistre (« délai dépassé »).
    fn abandonnee(r: &mut Run, outil: &str) {
        r.record(Step::tool(outil, 30_000).failed("délai dépassé (30 s)"));
        let index = r.steps.len() - 1;
        r.effet_inconnu(index);
    }

    #[test]
    fn une_etape_abandonnee_n_est_pas_un_echec_pour_la_decision() {
        let mut r = run();
        abandonnee(&mut r, "install_module");
        assert_eq!(r.derniere_en_echec(), None);
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        r.said("L'outil n'a pas répondu à temps : je ne sais pas si le module est installé.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.take_rappel(), None);
    }

    /// Le même tour sans le marquage : l'échec bloque (c'est ce que la session évitait mal).
    #[test]
    fn sans_marquage_le_delai_depasse_reste_un_echec() {
        let mut r = run();
        r.record(Step::tool("install_module", 30_000).failed("délai dépassé (30 s)"));
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        r.said("L'outil n'a pas répondu à temps.");
        assert!(matches!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Failed(_))
        ));
    }

    #[test]
    fn un_recit_appuye_sur_une_etape_abandonnee_demande_de_verifier() {
        let mut r = run();
        abandonnee(&mut r, "install_module");
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        r.said("J'ai installé le module.");
        assert_eq!(r.after_turn(true, false), Decision::Continue);
        let rappel = r.take_rappel().expect("un rappel");
        assert!(rappel.starts_with(evidence::RAPPEL), "{rappel}");
        assert!(rappel.contains("`install_module`"), "{rappel}");
        assert!(
            rappel.contains("vérifie son effet en relisant l'état"),
            "{rappel}"
        );
        assert!(rappel.contains("Ne refais pas l'action"), "{rappel}");
        // Il insiste sans rien vérifier : aucun fait ne le contredit, le run finit et le
        // verdict dit `Unverified` (pas un échec inventé).
        r.said("J'ai installé le module.");
        assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
        assert_eq!(r.verdict(), evidence::Verdict::Unverified);
    }

    /// L'étape inconnue ne blanchit pas un vrai refus antérieur.
    #[test]
    fn une_etape_abandonnee_ne_rattrape_pas_un_refus() {
        let mut r = run();
        r.record(Step::tool("install_module", 10).refused("même version"));
        abandonnee(&mut r, "install_module");
        assert_eq!(r.after_turn(false, true), Decision::Continue);
        assert_eq!(
            r.after_turn(true, false),
            Decision::Halt(Stop::Refused("même version".into()))
        );
    }
    /// Un échec n'est pas un refus, et inversement : les deux restent distinguables.
    #[test]
    fn un_echec_n_est_pas_un_refus() {
        let mut r = Run::new(Goal::new("x"), 20);
        r.record(Step::tool("a", 1).failed("cassé"));
        r.record(Step::tool("b", 1).refused("interdit"));
        assert_eq!(r.derniere_en_echec().map(|e| e.tool.as_str()), Some("a"));
        assert_eq!(r.derniere_refusee().map(|e| e.tool.as_str()), Some("b"));
        assert!(!r.steps[0].refused);
        assert!(r.steps[1].failure.is_none());
    }

    // ---- la politique est un port, pas une loi ----

    /// Une politique qui refuse de relancer : un refus arrête le run tout de suite.
    struct SansRelance;

    impl Policy for SansRelance {
        fn after_turn(&self, run: &mut Run, _spoke: bool, called: bool) -> Decision {
            if called {
                return Decision::Continue;
            }
            match run.derniere_refusee().and_then(|e| e.motif.clone()) {
                Some(motif) => Decision::Halt(Stop::Refused(motif)),
                None => Decision::Halt(Stop::Done),
            }
        }
    }

    /// Une politique qui ignore le budget de temps (et le dit en le dépassant).
    struct SansBudget;

    impl Policy for SansBudget {
        fn after_turn(&self, run: &mut Run, spoke: bool, called: bool) -> Decision {
            run.max_tours = run.max_tours.max(1);
            DefaultPolicy.after_turn(run, spoke, called)
        }
    }

    #[test]
    fn la_politique_par_defaut_est_exactement_l_ancien_comportement() {
        // Les deux chemins doivent rendre la même décision, tour après tour.
        let mut a = run();
        let mut b = run();
        for (spoke, called) in [(false, true), (true, false)] {
            assert_eq!(
                a.after_turn(spoke, called),
                b.after_turn_with(&DefaultPolicy, spoke, called)
            );
        }
    }

    #[test]
    fn remplacer_la_politique_change_la_decision() {
        let mut doux = run();
        doux.record(Step::tool("install_module", 10).refused("même version"));
        // Par défaut : on relance (un refus n'est pas une fin).
        assert_eq!(doux.after_turn(true, false), Decision::Continue);

        let mut dur = run();
        dur.record(Step::tool("install_module", 10).refused("même version"));
        // Avec la politique stricte : on s'arrête, et on dit pourquoi.
        match dur.after_turn_with(&SansRelance, true, false) {
            Decision::Halt(Stop::Refused(motif)) => assert_eq!(motif, "même version"),
            autre => panic!("attendu Refused, obtenu {autre:?}"),
        }
        assert_eq!(dur.stop, Some(Stop::Refused("même version".to_string())));
    }

    #[test]
    fn une_politique_peut_changer_le_budget() {
        let mut r = Run::new(Goal::new("boucle"), 0);
        // Le budget de tours à 0 arrête même la politique stricte…
        assert!(matches!(
            r.after_turn_with(&SansBudget, false, true),
            Decision::Halt(Stop::Budget)
        ));
    }

    #[test]
    fn une_etape_sans_effet_est_reperable() {
        let step = Step::tool("set_chrome", 4).state("A", "A");
        assert!(!step.a_change(), "un outil qui répond ok sans rien changer");
        let step = Step::tool("install_module", 900).state("A", "B");
        assert!(step.a_change());
    }

    #[test]
    fn un_halt_ecrit_toujours_la_raison_dans_le_run() {
        let mut r = Run::new(Goal::new("x"), 1);
        assert!(matches!(
            r.after_turn(false, true),
            Decision::Halt(Stop::Budget)
        ));
        assert_eq!(r.stop, Some(Stop::Budget), "un run ne ment pas sur sa fin");
    }

    #[test]
    fn une_etape_sans_effet_relance_au_lieu_de_conclure() {
        let mut r = run();
        r.record(Step::tool("set_chrome", 4).state("A", "A"));
        assert_eq!(r.after_turn(true, false), Decision::Continue);
    }

    #[test]
    fn le_budget_de_temps_arrete_le_run() {
        let mut r = run();
        r.budget(1500);
        r.record(Step::tool("install_module", 1600).state("a", "b"));
        r.after_turn(false, true);
        assert_eq!(r.stop, Some(Stop::Budget));
    }

    #[test]
    fn un_succes_remet_les_relances_a_zero() {
        let mut r = run();
        r.record(Step::tool("a", 1).refused("non"));
        r.after_turn(true, false);
        assert_eq!(r.relances_refus, 1);
        r.record(Step::tool("b", 1).state("x", "y"));
        r.after_turn(true, false);
        assert_eq!(r.relances_refus, 0);
    }

    #[test]
    fn le_motif_du_refus_se_relit_sans_l_affichage() {
        let etape = Step::tool("install_module", 5)
            .refused("même version")
            .resume("installation refusée");
        assert_eq!(etape.motif.as_deref(), Some("même version"));
        assert_eq!(etape.resume, "installation refusée");
    }

    #[test]
    fn chaque_arret_sait_se_dire() {
        assert_eq!(Stop::Done.phrase(), "terminé");
        assert!(Stop::Budget.phrase().contains("limite du run"));
        assert!(Stop::Interrupted.phrase().contains("interrompu"));
        assert!(Stop::Refused("x".into()).phrase().contains("bloqué"));
    }

    #[test]
    fn un_run_survit_a_un_aller_retour_json() {
        let mut r = run();
        r.record(Step::tool("run_program", 12).resume("42"));
        let texte = serde_json::to_string(&r).unwrap();
        let relu: Run = serde_json::from_str(&texte).unwrap();
        assert_eq!(relu, r);
    }
}
