//! Core module containing fundamental traits and types for the framework

pub mod entity;
pub mod extractors;
pub mod field;
pub mod link;
pub mod pluralize;
pub mod service;

pub use entity::{Data, Entity};
pub use field::{FieldFormat, FieldValue};
pub use link::{EntityReference, Link, LinkDefinition};
pub use pluralize::Pluralizer;
pub use service::{DataService, LinkService};
