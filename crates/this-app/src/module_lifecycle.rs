//! # A module's lifecycle, as the app shows it
//!
//! Installation, activation, deactivation, update, failure: each is a state of
//! [`ModuleState`], reached by an [`Event`] through [`Lifecycle::apply`]. An event that
//! makes no sense in the current state is refused ([`LifecycleError`]), never ignored.
//! Every state can be told to the person ([`Lifecycle::status`]: a sentence and a
//! [`Level`]) and turned into a notice ([`Lifecycle::notice`]).
//!
//! ```text
//!   install_started ─► Installing ─ install_succeeded ─► Installed
//!   Installed | Disabled ─ activate ─► Active ─ deactivate ─► Disabled
//!   Installed | Active | Disabled ─ update_started ─► Updating ─ update_succeeded ─►
//!       Active (if it was active) | Disabled (otherwise)
//!   any state ─ failed ─► Failed { during }    (Failed ─ activate / update / install retries)
//! ```
//!
//! Only an `Active` module is **loaded**: it contributes views to the menu, sections to the
//! settings, notices to the board. [`Modules::loaded`] is the set the other modules of this
//! crate take as input.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::module_notices::{Level, ModuleNotice};

/// What was going on when a module failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Install,
    Activation,
    Run,
    Deactivation,
    Update,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Phase::Install => "l'installation",
            Phase::Activation => "l'activation",
            Phase::Run => "l'exécution",
            Phase::Deactivation => "la désactivation",
            Phase::Update => "la mise à jour",
        }
    }
}

/// Where a module is in its life.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModuleState {
    Installing {
        version: String,
    },
    /// Installed, never activated.
    Installed,
    Active,
    Disabled,
    Updating {
        to: String,
        was_active: bool,
    },
    Failed {
        during: Phase,
        reason: String,
    },
}

/// What happens to a module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    InstallStarted {
        version: String,
    },
    InstallSucceeded,
    Activate,
    Deactivate,
    UpdateStarted {
        to: String,
    },
    UpdateSucceeded,
    /// Something failed; the phase is taken from the current state (an inactive module
    /// failing is an activation that failed).
    Failed {
        reason: String,
    },
}

/// An event refused in the current state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleError {
    pub module: String,
    pub state: ModuleState,
    pub event: Event,
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "module `{}`: {:?} is not allowed in {:?}",
            self.module, self.event, self.state
        )
    }
}

impl std::error::Error for LifecycleError {}

/// A sentence for the person, with its severity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusLine {
    pub text: String,
    pub level: Level,
}

/// One module's lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub module: String,
    /// The installed version; `None` while the first installation is running or failed.
    pub version: Option<String>,
    pub state: ModuleState,
}

impl Lifecycle {
    /// A module whose first installation starts.
    pub fn installing(module: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            version: None,
            state: ModuleState::Installing {
                version: version.into(),
            },
        }
    }

    /// Does the module contribute to the app right now?
    pub fn is_loaded(&self) -> bool {
        matches!(self.state, ModuleState::Active)
    }

    fn phase(&self) -> Phase {
        match self.state {
            ModuleState::Installing { .. } => Phase::Install,
            ModuleState::Updating { .. } => Phase::Update,
            ModuleState::Active => Phase::Run,
            // a failure reported on an inactive module is an activation that failed
            ModuleState::Installed | ModuleState::Disabled => Phase::Activation,
            ModuleState::Failed { during, .. } => during,
        }
    }

    /// Applies an event, or refuses it and leaves the state as it was.
    pub fn apply(&mut self, event: Event) -> Result<&ModuleState, LifecycleError> {
        use ModuleState as S;
        let next = match (&self.state, &event) {
            (S::Failed { .. }, Event::InstallStarted { version }) if self.version.is_none() => {
                Some(S::Installing {
                    version: version.clone(),
                })
            }
            (S::Installing { version }, Event::InstallSucceeded) => {
                self.version = Some(version.clone());
                Some(S::Installed)
            }
            (S::Installed | S::Disabled, Event::Activate) => Some(S::Active),
            (S::Failed { .. }, Event::Activate) if self.version.is_some() => Some(S::Active),
            (S::Active, Event::Deactivate) => Some(S::Disabled),
            (S::Installed | S::Disabled, Event::UpdateStarted { to }) => Some(S::Updating {
                to: to.clone(),
                was_active: false,
            }),
            (S::Active, Event::UpdateStarted { to }) => Some(S::Updating {
                to: to.clone(),
                was_active: true,
            }),
            (S::Failed { .. }, Event::UpdateStarted { to }) if self.version.is_some() => {
                Some(S::Updating {
                    to: to.clone(),
                    was_active: false,
                })
            }
            (S::Updating { to, was_active }, Event::UpdateSucceeded) => {
                self.version = Some(to.clone());
                Some(if *was_active { S::Active } else { S::Disabled })
            }
            (_, Event::Failed { reason }) => Some(S::Failed {
                during: self.phase(),
                reason: reason.clone(),
            }),
            _ => None,
        };
        match next {
            Some(state) => {
                self.state = state;
                Ok(&self.state)
            }
            None => Err(LifecycleError {
                module: self.module.clone(),
                state: self.state.clone(),
                event,
            }),
        }
    }

    /// What to tell the person about this module now.
    pub fn status(&self) -> StatusLine {
        let m = &self.module;
        let (text, level) = match &self.state {
            ModuleState::Installing { version } => {
                (format!("Installation de « {m} » {version}…"), Level::Info)
            }
            ModuleState::Installed => (
                format!(
                    "« {m} » {} est installé, pas encore activé.",
                    self.version_label()
                ),
                Level::Info,
            ),
            ModuleState::Active => (
                format!("« {m} » {} est actif.", self.version_label()),
                Level::Info,
            ),
            ModuleState::Disabled => (
                format!("« {m} » est désactivé : ses vues, réglages et avis sont masqués."),
                Level::Warning,
            ),
            ModuleState::Updating { to, .. } => (
                format!("Mise à jour de « {m} » {} vers {to}…", self.version_label()),
                Level::Info,
            ),
            ModuleState::Failed { during, reason } => (
                format!("« {m} » a échoué pendant {} : {reason}", during.label()),
                Level::Error,
            ),
        };
        StatusLine { text, level }
    }

    fn version_label(&self) -> &str {
        self.version.as_deref().unwrap_or("?")
    }

    /// The notice to post for this state, keyed `lifecycle` in the module's namespace so a
    /// newer state replaces the older one. Transient states last `ttl_ms`; a failure stays
    /// until dismissed.
    pub fn notice(&self, ttl_ms: u64) -> ModuleNotice {
        let status = self.status();
        let title = match &self.state {
            ModuleState::Installing { .. } => "Installation",
            ModuleState::Installed => "Installé",
            ModuleState::Active => "Actif",
            ModuleState::Disabled => "Désactivé",
            ModuleState::Updating { .. } => "Mise à jour",
            ModuleState::Failed { .. } => "Échec",
        };
        let notice = ModuleNotice::new(self.module.as_str(), "lifecycle", status.level, title)
            .with_body(status.text);
        match self.state {
            ModuleState::Failed { .. } => notice,
            _ => notice.for_ms(ttl_ms.max(1)),
        }
    }
}

/// Every module the app knows, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modules {
    modules: BTreeMap<String, Lifecycle>,
}

impl Modules {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts installing a module. A module already known is refused, never reset: use
    /// [`Modules::apply`] with `UpdateStarted` instead.
    pub fn install(&mut self, module: &str, version: &str) -> Result<&Lifecycle, LifecycleError> {
        if let Some(existing) = self.modules.get(module) {
            return Err(LifecycleError {
                module: module.to_string(),
                state: existing.state.clone(),
                event: Event::InstallStarted {
                    version: version.to_string(),
                },
            });
        }
        let lc = self
            .modules
            .entry(module.to_string())
            .or_insert_with(|| Lifecycle::installing(module, version));
        Ok(lc)
    }

    /// Applies an event to a known module. An unknown module is refused.
    pub fn apply(&mut self, module: &str, event: Event) -> Result<&Lifecycle, LifecycleError> {
        match self.modules.get_mut(module) {
            Some(lc) => {
                lc.apply(event)?;
                Ok(lc)
            }
            None => Err(LifecycleError {
                module: module.to_string(),
                state: ModuleState::Failed {
                    during: Phase::Install,
                    reason: "unknown module".into(),
                },
                event,
            }),
        }
    }

    /// Forgets a module (uninstalled).
    pub fn remove(&mut self, module: &str) -> Option<Lifecycle> {
        self.modules.remove(module)
    }

    pub fn get(&self, module: &str) -> Option<&Lifecycle> {
        self.modules.get(module)
    }

    /// The modules that contribute to the app now (active ones), by name.
    pub fn loaded(&self) -> BTreeSet<&str> {
        self.modules
            .values()
            .filter(|lc| lc.is_loaded())
            .map(|lc| lc.module.as_str())
            .collect()
    }

    /// Every module's status line, by module name.
    pub fn statuses(&self) -> Vec<(&str, StatusLine)> {
        self.modules
            .values()
            .map(|lc| (lc.module.as_str(), lc.status()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_then_activate_makes_a_module_loaded() {
        let mut lc = Lifecycle::installing("chat", "1.0.0");
        assert!(!lc.is_loaded());
        assert_eq!(
            lc.apply(Event::InstallSucceeded).cloned(),
            Ok(ModuleState::Installed)
        );
        assert_eq!(lc.version.as_deref(), Some("1.0.0"));
        assert_eq!(lc.apply(Event::Activate).cloned(), Ok(ModuleState::Active));
        assert!(lc.is_loaded());
    }

    #[test]
    fn deactivation_unloads_and_reactivation_reloads() {
        let mut lc = Lifecycle::installing("chat", "1.0.0");
        lc.apply(Event::InstallSucceeded).unwrap();
        lc.apply(Event::Activate).unwrap();
        lc.apply(Event::Deactivate).unwrap();
        assert_eq!(lc.state, ModuleState::Disabled);
        assert_eq!(lc.status().level, Level::Warning);
        lc.apply(Event::Activate).unwrap();
        assert!(lc.is_loaded());
    }

    #[test]
    fn an_update_returns_to_the_previous_activity() {
        let mut lc = Lifecycle::installing("chat", "1.0.0");
        lc.apply(Event::InstallSucceeded).unwrap();
        lc.apply(Event::Activate).unwrap();
        lc.apply(Event::UpdateStarted { to: "1.1.0".into() })
            .unwrap();
        assert!(!lc.is_loaded(), "not loaded while updating");
        assert!(
            lc.status().text.contains("1.0.0 vers 1.1.0"),
            "{}",
            lc.status().text
        );
        lc.apply(Event::UpdateSucceeded).unwrap();
        assert_eq!(lc.state, ModuleState::Active);
        assert_eq!(lc.version.as_deref(), Some("1.1.0"));

        let mut off = Lifecycle::installing("b", "1");
        off.apply(Event::InstallSucceeded).unwrap();
        off.apply(Event::UpdateStarted { to: "2".into() }).unwrap();
        off.apply(Event::UpdateSucceeded).unwrap();
        assert_eq!(off.state, ModuleState::Disabled);
    }

    #[test]
    fn a_failure_records_its_phase_and_is_told_as_an_error() {
        let mut lc = Lifecycle::installing("chat", "1.0.0");
        lc.apply(Event::Failed {
            reason: "signature refusée".into(),
        })
        .unwrap();
        assert_eq!(
            lc.state,
            ModuleState::Failed {
                during: Phase::Install,
                reason: "signature refusée".into()
            }
        );
        let s = lc.status();
        assert_eq!(s.level, Level::Error);
        assert!(
            s.text.contains("l'installation") && s.text.contains("signature refusée"),
            "{}",
            s.text
        );

        let mut act = Lifecycle::installing("m", "1");
        act.apply(Event::InstallSucceeded).unwrap();
        act.apply(Event::Failed { reason: "x".into() }).unwrap();
        assert!(matches!(
            act.state,
            ModuleState::Failed {
                during: Phase::Activation,
                ..
            }
        ));

        let mut upd = Lifecycle::installing("m", "1");
        upd.apply(Event::InstallSucceeded).unwrap();
        upd.apply(Event::Activate).unwrap();
        upd.apply(Event::UpdateStarted { to: "2".into() }).unwrap();
        upd.apply(Event::Failed { reason: "x".into() }).unwrap();
        assert!(matches!(
            upd.state,
            ModuleState::Failed {
                during: Phase::Update,
                ..
            }
        ));
        assert_eq!(
            upd.version.as_deref(),
            Some("1"),
            "the old version is still installed"
        );
    }

    #[test]
    fn a_failed_module_can_be_retried() {
        let mut first = Lifecycle::installing("m", "1");
        first
            .apply(Event::Failed {
                reason: "réseau".into(),
            })
            .unwrap();
        assert!(
            first.apply(Event::Activate).is_err(),
            "nothing installed to activate"
        );
        first
            .apply(Event::InstallStarted {
                version: "1".into(),
            })
            .unwrap();
        first.apply(Event::InstallSucceeded).unwrap();

        let mut run = Lifecycle::installing("m", "1");
        run.apply(Event::InstallSucceeded).unwrap();
        run.apply(Event::Activate).unwrap();
        run.apply(Event::Failed {
            reason: "panique".into(),
        })
        .unwrap();
        assert!(matches!(
            run.state,
            ModuleState::Failed {
                during: Phase::Run,
                ..
            }
        ));
        run.apply(Event::Activate).unwrap();
        assert!(run.is_loaded());
    }

    #[test]
    fn an_event_out_of_place_is_refused_and_changes_nothing() {
        let mut lc = Lifecycle::installing("m", "1");
        let err = lc.apply(Event::Activate).unwrap_err();
        assert_eq!(err.module, "m");
        assert_eq!(err.event, Event::Activate);
        assert!(matches!(lc.state, ModuleState::Installing { .. }));
        lc.apply(Event::InstallSucceeded).unwrap();
        assert!(lc.apply(Event::Deactivate).is_err());
        assert!(lc.apply(Event::UpdateSucceeded).is_err());
        assert!(lc.apply(Event::InstallSucceeded).is_err());
        assert_eq!(lc.state, ModuleState::Installed);
    }

    #[test]
    fn every_state_has_a_status_line_naming_the_module() {
        let states = [
            ModuleState::Installing {
                version: "1".into(),
            },
            ModuleState::Installed,
            ModuleState::Active,
            ModuleState::Disabled,
            ModuleState::Updating {
                to: "2".into(),
                was_active: true,
            },
            ModuleState::Failed {
                during: Phase::Run,
                reason: "boum".into(),
            },
        ];
        for state in states {
            let lc = Lifecycle {
                module: "chat".into(),
                version: Some("1".into()),
                state,
            };
            let s = lc.status();
            assert!(s.text.contains("chat"), "{}", s.text);
        }
    }

    #[test]
    fn the_lifecycle_notice_replaces_itself_and_a_failure_stays() {
        use crate::module_notices::Board;
        let mut lc = Lifecycle::installing("chat", "1");
        let mut board = Board::new(5);
        board.publish(lc.notice(3_000), 0).unwrap();
        lc.apply(Event::Failed {
            reason: "boum".into(),
        })
        .unwrap();
        board.publish(lc.notice(3_000), 10).unwrap();
        let v = board.visible(1_000_000);
        assert_eq!(v.len(), 1, "one lifecycle notice per module");
        assert_eq!(v[0].notice.level, Level::Error);
        assert_eq!(v[0].expires_at_ms, None);
    }

    #[test]
    fn modules_reports_the_loaded_set() {
        let mut ms = Modules::new();
        ms.install("chat", "1").unwrap();
        ms.install("billing", "1").unwrap();
        assert!(ms.install("chat", "2").is_err(), "already known");
        for m in ["chat", "billing"] {
            ms.apply(m, Event::InstallSucceeded).unwrap();
        }
        ms.apply("chat", Event::Activate).unwrap();
        assert_eq!(ms.loaded().into_iter().collect::<Vec<_>>(), ["chat"]);
        assert!(ms.apply("ghost", Event::Activate).is_err());
        assert_eq!(ms.statuses().len(), 2);
        assert!(ms.remove("billing").is_some());
        assert!(ms.get("billing").is_none());
    }

    #[test]
    fn the_loaded_set_drives_the_menu_settings_and_notices() {
        use crate::menu::{DeclaredView, ModuleViews, Placement, Reason, ViewRef, build_menu};
        use crate::module_notices::Board;
        use crate::module_settings::{Field, Registry, Section};

        let mut ms = Modules::new();
        for m in ["chat", "billing"] {
            ms.install(m, "1").unwrap();
            ms.apply(m, Event::InstallSucceeded).unwrap();
            ms.apply(m, Event::Activate).unwrap();
        }
        ms.apply("billing", Event::Deactivate).unwrap();
        let loaded = ms.loaded();

        let decls = vec![
            ModuleViews::new(
                "chat",
                vec![DeclaredView::new("conversations", Placement::Sidebar)],
            ),
            ModuleViews::new(
                "billing",
                vec![DeclaredView::new("invoices", Placement::Sidebar)],
            ),
        ];
        let menu = build_menu(&decls, loaded.iter().copied());
        assert_eq!(menu.entries.len(), 1);
        assert_eq!(
            menu.why(&ViewRef::new("billing", "invoices"))
                .map(|o| &o.reason),
            Some(&Reason::ModuleNotLoaded)
        );

        let mut reg = Registry::new();
        reg.declare(Section::new(
            "billing",
            "g",
            "G",
            vec![Field::toggle("pdf", "PDF", true)],
        ))
        .unwrap();
        reg.declare(Section::new(
            "chat",
            "g",
            "G",
            vec![Field::toggle("sound", "Son", true)],
        ))
        .unwrap();
        let shown: Vec<_> = reg
            .visible(loaded.iter().copied())
            .iter()
            .map(|s| s.module.as_str())
            .collect();
        assert_eq!(shown, ["chat"]);

        let mut board = Board::new(5);
        board
            .publish(ModuleNotice::new("billing", "k", Level::Info, "b"), 0)
            .unwrap();
        board
            .publish(ModuleNotice::new("chat", "k", Level::Info, "c"), 0)
            .unwrap();
        assert_eq!(board.retain_modules(loaded.iter().copied()), 1);
    }
}
