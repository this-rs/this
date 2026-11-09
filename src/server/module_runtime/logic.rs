//! Business logic attached to a declarative entity: the seam a compiled (wasm) module plugs
//! into, and that a declarative module never touches.
//!
//! [`JsonEntityService`](super::JsonEntityService) validates, stores and publishes. When an
//! [`EntityLogic`] is attached, it is consulted at each operation — create, update, delete,
//! get, list, and named operations — and answers with a [`LogicOutcome`]: an optional
//! replacement value and an ordered list of [`Effect`]s. **The logic decides, the host
//! applies**: the service stores the rows, publishes the events, journals the log lines, and
//! refuses whatever leaves the closed vocabulary of effects.
//!
//! Without logic nothing changes: every hook defaults to "no outcome", and the service's
//! behaviour is strictly the one it had before this seam.
//!
//! # Effects (closed set)
//!
//! | kind | form | applied by the host |
//! |---|---|---|
//! | `put` | `{"kind":"put","entity":"task","body":{…}}` | create (no `id`) or update (`id`) |
//! | `delete` | `{"kind":"delete","entity":"task","id":"…"}` | remove the row |
//! | `emit` | `{"kind":"emit","topic":"…","payload":{…}}` | publish on the event bus |
//! | `log` | `{"kind":"log","level":"info","message":"…"}` | journal |
//!
//! An unknown `kind` does not deserialize ([`Effect`] has no catch-all variant), so the whole
//! response is rejected and nothing is applied.

use crate::descriptor::ContractError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the host does on behalf of the logic. Closed: no variant, no effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    Put {
        entity: String,
        body: Value,
    },
    Delete {
        entity: String,
        id: String,
    },
    Emit {
        topic: String,
        #[serde(default)]
        payload: Value,
    },
    Log {
        #[serde(default)]
        level: LogLevel,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    #[default]
    Info,
    Warn,
    Error,
}

/// What the logic answered: an optional value (replaces the one the host was about to use)
/// and the effects to apply, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogicOutcome {
    pub output: Option<Value>,
    pub effects: Vec<Effect>,
}

/// The business logic of one entity. Every hook is a no-op by default.
///
/// A hook that returns `Err` refuses the operation: nothing is stored and no effect is
/// applied.
#[async_trait]
pub trait EntityLogic: Send + Sync {
    /// `input` is the payload, already validated by the descriptor. `output` (if any) is the
    /// row to store instead — validated again by the host, so a module cannot persist what
    /// its own descriptor forbids.
    async fn create(&self, _input: &Value) -> Result<LogicOutcome, ContractError> {
        Ok(LogicOutcome::default())
    }
    /// `patch` is validated; `current` is the stored row.
    async fn update(
        &self,
        _id: &str,
        _patch: &Value,
        _current: &Value,
    ) -> Result<LogicOutcome, ContractError> {
        Ok(LogicOutcome::default())
    }
    /// Called with the row about to be removed; `Err` keeps it.
    async fn delete(&self, _id: &str, _current: &Value) -> Result<LogicOutcome, ContractError> {
        Ok(LogicOutcome::default())
    }
    /// `output` (if any) is served instead of the stored row.
    async fn get(&self, _row: &Value) -> Result<LogicOutcome, ContractError> {
        Ok(LogicOutcome::default())
    }
    /// `output` (if any, an array) is served instead of the stored rows.
    async fn list(&self, _rows: &[Value]) -> Result<LogicOutcome, ContractError> {
        Ok(LogicOutcome::default())
    }
    /// A named operation. `None` = this logic has no such operation.
    async fn call(
        &self,
        _name: &str,
        _input: &Value,
        _rows: &[Value],
    ) -> Result<Option<LogicOutcome>, ContractError> {
        Ok(None)
    }
}

/// Where the effects go. The service hands the whole list over; the sink validates it
/// **entirely** before applying any of it (an effect that would be refused halfway leaves
/// no half-applied state behind).
#[async_trait]
pub trait EffectSink: Send + Sync {
    /// Validate the effects without applying any: known entity, well-formed id, body that
    /// passes the descriptor's validation. The service calls it **before** it stores the row
    /// of the operation, so a refused effect leaves nothing behind.
    async fn check(&self, effects: &[Effect]) -> Result<(), ContractError>;
    async fn apply(&self, effects: Vec<Effect>) -> Result<(), ContractError>;
}
