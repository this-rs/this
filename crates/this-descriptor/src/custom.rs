//! Named extensions (`Custom { name, params }`) and the registry that resolves them.
//!
//! Every open enumeration (field type, validator, transform, capability) has a `Custom`
//! variant. A `Custom` absent from the registry fails `check()`: it is never ignored.

use crate::error::FieldViolation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// Named extension with free parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Custom {
    pub name: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
}

impl Custom {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            params: serde_json::Value::Null,
        }
    }
}

/// Where a custom validator can run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomScope {
    /// Pure: registered on the server AND on the front (wasm32).
    #[default]
    Both,
    /// Needs the server (database access...). The front reports it as `deferred`.
    ServerOnly,
}

/// Category of a named extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CustomKind {
    FieldType,
    Validator,
    Transform,
    /// Entity or module capability (`this.allow_unknown_fields`...).
    Capability,
}

/// `(field, value, params) -> Ok | violation`.
pub type CustomValidatorFn = Arc<
    dyn Fn(&str, &serde_json::Value, &serde_json::Value) -> Result<(), FieldViolation>
        + Send
        + Sync,
>;
/// `(value, params) -> value`. MUST be idempotent (see [`CustomRegistry::register_transform`]).
pub type CustomTransformFn =
    Arc<dyn Fn(serde_json::Value, &serde_json::Value) -> serde_json::Value + Send + Sync>;

#[derive(Clone)]
enum Entry {
    Name,
    Validator {
        scope: CustomScope,
        f: Option<CustomValidatorFn>,
    },
    Transform(CustomTransformFn),
}

/// Capabilities known by the framework itself.
pub const ALLOW_UNKNOWN_FIELDS: &str = "this.allow_unknown_fields";

/// Registry of known extensions, by category.
#[derive(Clone)]
pub struct CustomRegistry {
    entries: BTreeMap<(CustomKind, String), Entry>,
}

impl Default for CustomRegistry {
    fn default() -> Self {
        let mut r = Self {
            entries: BTreeMap::new(),
        };
        r.register_name(CustomKind::Capability, ALLOW_UNKNOWN_FIELDS);
        r
    }
}

impl fmt::Debug for CustomRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.entries.keys()).finish()
    }
}

/// The named extension is not registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownCustom;

/// Refusal to register a non-idempotent transform.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "custom transform `{0}` must be idempotent: it may run twice (ValidatingCreator after Validated<T>)"
)]
pub struct NotIdempotent(pub String);

impl CustomRegistry {
    /// Declare a name with no behaviour (field types, capabilities, or a server-only
    /// validator known on the front by name only).
    pub fn register_name(&mut self, kind: CustomKind, name: impl Into<String>) -> &mut Self {
        self.entries.insert((kind, name.into()), Entry::Name);
        self
    }

    /// Register a validator with its implementation and scope.
    pub fn register_validator(
        &mut self,
        name: impl Into<String>,
        scope: CustomScope,
        f: CustomValidatorFn,
    ) -> &mut Self {
        self.entries.insert(
            (CustomKind::Validator, name.into()),
            Entry::Validator { scope, f: Some(f) },
        );
        self
    }

    /// Declare a server-only validator on a client: known by name, reported as deferred.
    pub fn declare_server_only_validator(&mut self, name: impl Into<String>) -> &mut Self {
        self.entries.insert(
            (CustomKind::Validator, name.into()),
            Entry::Validator {
                scope: CustomScope::ServerOnly,
                f: None,
            },
        );
        self
    }

    /// Register a transform. `idempotent` must be `true`: a payload can be validated twice
    /// (REST `Validated<T>` then `ValidatingCreator`), and `f(f(x)) != f(x)` would corrupt it.
    pub fn register_transform(
        &mut self,
        name: impl Into<String>,
        idempotent: bool,
        f: CustomTransformFn,
    ) -> Result<&mut Self, NotIdempotent> {
        let name = name.into();
        if !idempotent {
            return Err(NotIdempotent(name));
        }
        self.entries
            .insert((CustomKind::Transform, name), Entry::Transform(f));
        Ok(self)
    }

    pub fn knows(&self, kind: CustomKind, name: &str) -> bool {
        self.entries.contains_key(&(kind, name.to_string()))
    }

    /// Implementation of a validator. `Ok(None)` = known but server-only without an
    /// implementation here (deferred). `Err(())` = unknown.
    pub fn validator(
        &self,
        name: &str,
    ) -> Result<Option<(CustomScope, CustomValidatorFn)>, UnknownCustom> {
        match self.entries.get(&(CustomKind::Validator, name.to_string())) {
            Some(Entry::Validator { scope, f: Some(f) }) => Ok(Some((*scope, f.clone()))),
            Some(Entry::Validator { f: None, .. }) | Some(Entry::Name) => Ok(None),
            _ => Err(UnknownCustom),
        }
    }

    pub fn transform(&self, name: &str) -> Option<CustomTransformFn> {
        match self.entries.get(&(CustomKind::Transform, name.to_string())) {
            Some(Entry::Transform(f)) => Some(f.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_idempotent_transform_is_refused() {
        let mut r = CustomRegistry::default();
        let f: CustomTransformFn = Arc::new(|v, _| v);
        assert!(r.register_transform("x.append", false, f.clone()).is_err());
        assert!(r.register_transform("x.trim", true, f).is_ok());
        assert!(r.knows(CustomKind::Transform, "x.trim"));
        assert!(r.knows(CustomKind::Capability, ALLOW_UNKNOWN_FIELDS));
    }
}
