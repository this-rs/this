//! Validation and filtering system
//!
//! Two rule sets coexist (RFC module-descriptor §3.5):
//!
//! - **Descriptor** (entities whose module publishes a `ModuleDescriptor`): the shared
//!   engine `this_descriptor::engine::validate` runs in the COMMON write path —
//!   [`gate::ValidatingCreator`] wraps the host's creators (GraphQL, gRPC, link handlers),
//!   and the REST [`layer`] covers CRUD handlers that bypass creators. Required vs
//!   optional, read-only, unknown keys, types, transforms then validators: one
//!   `this.validation.failed` with every violation.
//! - **Legacy** (no descriptor): `EntityValidationConfig` closures, opt-in through the
//!   [`Validated`] extractor, unchanged.

pub mod config;
pub mod extractor;
pub mod filters;
pub mod gate;
pub mod layer;
pub mod validators;

pub use config::EntityValidationConfig;
pub use extractor::Validated;
pub use gate::{SharedRegistry, ValidatingCreator, shared_registry, wrap_creators};
pub use layer::{EngineValidated, ValidationLayerState, validation_middleware};
