//! Runtime module loading.
//!
//! Placed under `server/` (not `core/`): a `ModuleInterface` carries an `axum::Router`, and
//! `core` must not gain new edges towards `server`.
//!
//! - `loader.rs`: `ModuleSource` (compiled, stored, manifest, plugin), `ModuleInterface`,
//!   `ModuleStatus`, trait `ModuleLoader`, `DefaultModuleLoader`.
//! - `runtime.rs`: `ModuleRuntime` (load / replace / unload on a live host, lifecycle,
//!   catalog publication, events).
//! - `dispatch.rs`: the dispatch layer (runtime routes, drain, cancellation, policies).
//! - `json_entity.rs`: `JsonEntityService`, adapter of declarative entities.
//! - `admin.rs`: REST admin interface (list / get / load / unload), admin policy.
//!
//! No native code is loaded at runtime (no stable Rust ABI); the WASM/WAMI extension
//! point is `ModuleSource::Plugin` + `DefaultModuleLoader::with_plugin`.

// `ContractError` (152 bytes) IS the error contract of the runtime, by design of the
// descriptor crate (same allowance there): boxing it would change every public signature.
#![allow(clippy::result_large_err)]

pub mod admin;
pub mod dispatch;
pub mod json_entity;
pub mod loader;
pub mod runtime;

pub use dispatch::ModuleCancellation;
pub use json_entity::JsonEntityService;
pub use loader::{DefaultModuleLoader, ModuleInterface, ModuleLoader, ModuleSource, ModuleStatus};
pub use runtime::{HostParts, ModuleRuntime, RuntimeOptions};
