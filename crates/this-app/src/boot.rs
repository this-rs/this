//! La séquence de démarrage.
//!
//! Le splash n'est pas un dessin : c'est une petite machine à états avec deux règles.
//! 1. Il reste tant que l'app n'est pas prête.
//! 2. Il reste **au moins** `min_ms`, même si tout est prêt tout de suite — sinon il
//!    clignote pendant 40 ms et donne une impression de bug.
//!
//! L'horloge est passée par l'appelant (`now_ms`) : le module reste pur et testable.

/// Où en est le démarrage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootState {
    /// En cours : une étape est peut-être renseignée.
    Loading,
    /// Tout est prêt ; le splash peut disparaître une fois la durée minimale écoulée.
    Ready,
    /// Échec : le splash reste affiché et porte le message.
    Failed(String),
}

/// Suivi d'une séquence de démarrage.
#[derive(Clone, Debug)]
pub struct Boot {
    started_ms: u64,
    min_ms: u64,
    steps: Vec<String>,
    state: BootState,
}

impl Boot {
    /// `now_ms` : l'instant de départ dans l'horloge de l'appelant.
    pub fn new(min_ms: u64, now_ms: u64) -> Self {
        Self {
            started_ms: now_ms,
            min_ms,
            steps: Vec::new(),
            state: BootState::Loading,
        }
    }

    /// Note une étape franchie (`"chargement du module"`), pour que le splash raconte.
    pub fn step(&mut self, label: impl Into<String>) -> &mut Self {
        self.steps.push(label.into());
        self
    }

    /// Tout est prêt.
    pub fn ready(&mut self) -> &mut Self {
        if matches!(self.state, BootState::Loading) {
            self.state = BootState::Ready;
        }
        self
    }

    /// Le démarrage a échoué : le splash reste, avec le message du domaine.
    pub fn failed(&mut self, message: impl Into<String>) -> &mut Self {
        self.state = BootState::Failed(message.into());
        self
    }

    pub fn state(&self) -> &BootState {
        &self.state
    }

    pub fn steps(&self) -> &[String] {
        &self.steps
    }

    /// Le splash doit-il être affiché à cet instant ?
    pub fn visible(&self, now_ms: u64) -> bool {
        match self.state {
            BootState::Loading => true,
            BootState::Failed(_) => true,
            BootState::Ready => now_ms < self.started_ms.saturating_add(self.min_ms),
        }
    }

    /// Temps restant avant que le splash puisse partir (`0` s'il est déjà libre).
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.started_ms
            .saturating_add(self.min_ms)
            .saturating_sub(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_is_always_visible() {
        let boot = Boot::new(400, 1_000);
        assert!(boot.visible(1_000));
        assert!(boot.visible(999_999));
        assert_eq!(boot.state(), &BootState::Loading);
    }

    #[test]
    fn ready_waits_for_the_minimum_duration() {
        let mut boot = Boot::new(400, 1_000);
        boot.ready();
        assert!(boot.visible(1_000));
        assert!(boot.visible(1_399));
        assert!(!boot.visible(1_400));
    }

    #[test]
    fn ready_after_the_delay_hides_immediately() {
        let mut boot = Boot::new(400, 1_000);
        boot.ready();
        // l'app a mis 3 s : plus rien à attendre
        assert!(!boot.visible(4_000));
        assert_eq!(boot.remaining_ms(4_000), 0);
    }

    #[test]
    fn failure_keeps_the_splash_with_its_message() {
        let mut boot = Boot::new(400, 0);
        boot.failed("store injoignable");
        assert!(boot.visible(10_000));
        assert_eq!(boot.state(), &BootState::Failed("store injoignable".into()));
    }

    #[test]
    fn steps_are_recorded_in_order() {
        let mut boot = Boot::new(0, 0);
        boot.step("ouverture du store")
            .step("chargement du module")
            .step("lecture des données");
        assert_eq!(boot.steps().len(), 3);
        assert_eq!(boot.steps()[1], "chargement du module");
    }

    #[test]
    fn remaining_ms_is_clamped() {
        let boot = Boot::new(500, 2_000);
        assert_eq!(boot.remaining_ms(2_100), 400);
        assert_eq!(boot.remaining_ms(9_000), 0);
    }

    #[test]
    fn ready_is_idempotent_and_does_not_overwrite_failure() {
        let mut boot = Boot::new(0, 0);
        boot.failed("boum");
        boot.ready();
        assert_eq!(boot.state(), &BootState::Failed("boum".into()));
    }
}
