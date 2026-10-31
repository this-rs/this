//! Core module containing fundamental traits and types for the framework

pub mod auth;
pub mod entity;
/// `ContractError` across layers: `from_anyhow` and protocol adapters (module descriptor RFC).
pub mod errors;
pub mod events;
pub mod extractors;
pub mod field;
/// Locale extraction (Accept-Language, gRPC `accept-language`): errors-i18n stream.
pub mod i18n;
/// Identité d'entité **engine-agnostique** : la même entité, quel que soit le
/// moteur qui la porte (convention `_uuid`/`_data`, propriétés plates, api, chaîne).
pub mod identity;
pub mod link;
pub mod module;
pub mod pluralize;
pub mod query;
pub mod service;
/// Soft delete as a contract over any `DataService`: the README's promise,
/// stated once instead of nine times.
pub mod soft_delete;
pub mod store;
pub mod tenant;
pub mod validation;

pub use auth::{AuthContext, AuthPolicy, AuthProvider, NoAuthProvider};
pub use entity::{Data, Entity, Link};
pub use events::{EntityEvent, EventBus, EventEnvelope, FrameworkEvent, LinkEvent};
pub use field::{FieldFormat, FieldValue};
pub use identity::{
    ContentDigestProjection, ConventionProjection, ENTITY_ID_SCHEME, EntityKey, EntityProjection,
    Fallback, FlatPropertyProjection, IdentityError, IdentityIndex, IdentityRegistry, MotiveSpec,
    NodeView, PropValue, Provenance, Qualifier, ResolvedEntity,
};
pub use link::{LinkAuthConfig, LinkDefinition};
pub use module::{EntityCreator, EntityFetcher, Module};
pub use pluralize::Pluralizer;
pub use query::{PaginatedResponse, PaginationMeta, QueryParams};
pub use service::{DataService, LinkService};
pub use soft_delete::{DeleteOutcome, SoftDeleting};
pub use store::QueryableStore;
pub use tenant::TenantContext;
pub use validation::{EntityValidationConfig, Validated};
