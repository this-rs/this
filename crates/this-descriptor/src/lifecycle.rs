//! Observable lifecycle of a module at runtime.
//!
//! Six observable states. Transient steps of one `load()` call (resolving, loading,
//! activating) are logs and traces, not states.

use crate::error::{ContractError, ErrorCode};
use crate::framework::codes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ModuleState {
    /// Known (catalog or store), not loaded.
    Discovered,
    /// Dependencies resolved, interface built, not yet served.
    Resolved,
    /// Published in the host catalog: served on every surface.
    Active,
    /// Unload or replace requested: new requests get `this.module.unavailable` (503);
    /// in-flight requests finish, streams are cancelled after the grace delay.
    Draining,
    Unloaded,
    Failed {
        code: ErrorCode,
        at: String,
    },
}

impl ModuleState {
    fn tag(&self) -> &'static str {
        match self {
            ModuleState::Discovered => "discovered",
            ModuleState::Resolved => "resolved",
            ModuleState::Active => "active",
            ModuleState::Draining => "draining",
            ModuleState::Unloaded => "unloaded",
            ModuleState::Failed { .. } => "failed",
        }
    }

    /// Is `self -> to` a legal transition?
    ///
    /// - Discovered -> Resolved | Failed
    /// - Resolved -> Active | Failed
    /// - Active -> Draining (unload, replace) | Failed (backend failure)
    /// - Draining -> Unloaded | Active (unload cancelled) | Failed
    /// - Unloaded -> Discovered (reload); Failed -> Discovered (fix, then load)
    pub fn can_transition(&self, to: &ModuleState) -> bool {
        use ModuleState::*;
        matches!(
            (self, to),
            (Discovered, Resolved)
                | (Discovered, Failed { .. })
                | (Resolved, Active)
                | (Resolved, Failed { .. })
                | (Active, Draining)
                | (Active, Failed { .. })
                | (Draining, Unloaded)
                | (Draining, Active)
                | (Draining, Failed { .. })
                | (Unloaded, Discovered)
                | (Failed { .. }, Discovered)
        )
    }

    /// Checked transition.
    pub fn transition(&mut self, to: ModuleState) -> Result<(), ContractError> {
        if self.can_transition(&to) {
            *self = to;
            Ok(())
        } else {
            Err(ContractError::framework(codes::MODULE_ILLEGAL_TRANSITION)
                .with_param("from", self.tag())
                .with_param("to", to.tag()))
        }
    }
}

/// Grace delay of `Draining` before streams (SSE, WebSocket, gRPC server streams) are
/// cancelled. Default 30 s; a host can change it.
pub const DEFAULT_DRAIN_GRACE_MS: u64 = 30_000;

#[cfg(test)]
mod tests {
    use super::*;
    use ModuleState::*;

    #[test]
    fn lifecycle_transitions_are_legal_only() {
        let failed = Failed {
            code: ErrorCode::new("this.internal"),
            at: "t".into(),
        };
        let all = [
            Discovered,
            Resolved,
            Active,
            Draining,
            Unloaded,
            failed.clone(),
        ];
        let legal = [
            (0, 1),
            (0, 5),
            (1, 2),
            (1, 5),
            (2, 3),
            (2, 5),
            (3, 4),
            (3, 2),
            (3, 5),
            (4, 0),
            (5, 0),
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(
                    a.can_transition(b),
                    legal.contains(&(i, j)),
                    "{a:?} -> {b:?}"
                );
            }
        }
        let mut s = Active;
        assert!(s.transition(Discovered).is_err());
        s.transition(Draining).unwrap();
        s.transition(Active).unwrap();
    }
}
