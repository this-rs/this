//! Core module containing fundamental traits and types for the framework

pub mod auth;
pub mod entity;
pub mod extractors;
pub mod field;
pub mod link;
pub mod module;
pub mod pluralize;
pub mod query;
pub mod service;
pub mod store;
pub mod validation;

pub use auth::{AuthContext, AuthPolicy, AuthProvider, NoAuthProvider};
pub use entity::{Data, Entity, Link};
pub use field::{FieldFormat, FieldValue};
pub use link::{LinkAuthConfig, LinkDefinition};
pub use module::{EntityCreator, EntityFetcher, Module};
pub use pluralize::Pluralizer;
pub use query::{PaginatedResponse, PaginationMeta, QueryParams};
pub use service::{DataService, LinkService};
pub use store::QueryableStore;
pub use validation::{EntityValidationConfig, Validated};
