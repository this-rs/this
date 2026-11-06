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

pub mod graph;
pub mod substrate;
pub mod surface;

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
                "j'ai atteint la limite de tours sans conclure — dis-moi de continuer".to_string()
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

    pub fn failed(mut self, erreur: impl Into<String>) -> Self {
        self.failure = Some(erreur.into());
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
            stop: None,
        }
    }

    pub fn record(&mut self, mut step: Step) {
        step.index = self.steps.len() as u32;
        if step.at_ms == 0 {
            step.at_ms = self.ecoule_ms;
        }
        self.ecoule_ms += step.ms;
        self.steps.push(step);
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
        self.du_tour()
            .iter()
            .rev()
            .find(|etape| !etape.before.is_empty() && !etape.a_change())
    }

    /// Le dernier refus **du tour en cours**. Cherché sur tout le run, un refus
    /// ancien condamnait chaque tour suivant.
    pub fn derniere_refusee(&self) -> Option<&Step> {
        self.du_tour().iter().rev().find(|etape| etape.refused)
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
        let muet = !a_parle;
        // Une étape qui n'a rien changé n'est pas un progrès : la relance doit
        // exiger un changement, sinon on tourne en rond en croyant avancer.
        let sans_effet = self.dernier_sans_effet().is_some();

        if refus.is_some() || muet || sans_effet {
            let (compteur, max) = match (refus.is_some(), muet) {
                (true, _) => (self.relances_refus, self.max_relances),
                (false, true) => (self.relances_muet, self.max_relances),
                _ => (self.relances_refus, self.max_relances),
            };
            self.tour_debut = fin_du_tour;
            if compteur < max {
                if refus.is_some() {
                    self.relances_refus += 1;
                } else {
                    self.relances_muet += 1;
                }
                return Decision::Continue;
            }
            return self.halt(match refus {
                Some(raison) => Stop::Refused(raison),
                None if muet => Stop::Failed("le modèle n'a rendu ni texte ni outil".to_string()),
                None => Stop::Failed("les actions n'ont rien changé".to_string()),
            });
        }

        // Un tour qui aboutit remet les crédits de relance à zéro.
        self.tour_debut = fin_du_tour;
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
        assert!(Stop::Budget.phrase().contains("limite de tours"));
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
