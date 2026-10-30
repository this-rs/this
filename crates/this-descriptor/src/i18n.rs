//! i18n declarations.
//!
//! The descriptor carries the **keys** a module uses (with their expected parameters) and
//! its default locale; the **texts** live in one [`MessageBundle`] per locale, loaded on
//! demand (`ModuleStore`: `i18n/<locale>.json`; compiled module: `include_str!`). The
//! translations are therefore stored in ONE place, and a wasm front downloads only the
//! locale it shows.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Message key, namespaced by module: `billing.invoice.amount.label`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageKey(pub String);

impl MessageKey {
    pub fn new(k: impl Into<String>) -> Self {
        Self(k.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for MessageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl From<&str> for MessageKey {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}

/// BCP 47 tag: `fr`, `fr-CA`, `en`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Locale(pub String);

impl Locale {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Primary language subtag (`fr` for `fr-CA`), lower-cased.
    pub fn language(&self) -> String {
        self.0
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }
}
impl Default for Locale {
    fn default() -> Self {
        Self("fr".into())
    }
}
impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Template syntax of a bundle. `Simple` = `{param}` substitution. `Fluent` is reserved
/// (plural rules); `check()` refuses it until an engine implements it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MessageSyntax {
    #[default]
    Simple,
    Fluent,
}

/// A key declared by a module, with the parameters its message expects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDecl {
    pub key: MessageKey,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
}

/// What a descriptor says about its messages (no texts).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct I18nDecl {
    #[serde(default)]
    pub default_locale: Locale,
    /// Locales for which a bundle is shipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locales: Vec<Locale>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<MessageDecl>,
}

/// Texts of ONE module in ONE locale.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MessageBundle {
    /// Module the bundle belongs to (`this` for the framework).
    pub module: String,
    pub locale: Locale,
    #[serde(default)]
    pub syntax: MessageSyntax,
    pub messages: BTreeMap<MessageKey, String>,
}

impl MessageBundle {
    pub fn get(&self, key: &MessageKey) -> Option<&str> {
        self.messages.get(key).map(String::as_str)
    }
}
