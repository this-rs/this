//! Core module containing fundamental traits and types for the framework

pub mod auth;
pub mod entity;
pub mod extractors;
pub mod field;
pub mod link;
pub mod module;
pub mod pluralize;
pub mod service;

pub use auth::{AuthContext, AuthPolicy, AuthProvider, NoAuthProvider};
pub use entity::{Data, Entity};
pub use field::{FieldFormat, FieldValue};
pub use link::{EntityReference, Link, LinkAuthConfig, LinkDefinition};
pub use module::{EntityFetcher, Module};
pub use pluralize::Pluralizer;
pub use service::{DataService, LinkService};
