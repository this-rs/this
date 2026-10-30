//! Intelligent pluralization for English nouns
//!
//! The implementation lives in `this_descriptor::naming` so that this-rs, this-cli and
//! the Leptos front share ONE pluralizer (RFC module-descriptor, objection "plural declared
//! in 6 places"). This module keeps the historical path `this::core::pluralize::Pluralizer`.

pub use this_descriptor::naming::Pluralizer;
