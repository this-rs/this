//! Runtime module loading (interfaces). Implemented by the `modules-runtime` stream
//! (`runtime.rs`, `dispatch.rs`, `json_entity.rs`).
//!
//! Placed under `server/` (not `core/`): a `ModuleInterface` carries an `axum::Router`, and
//! `core` must not gain new edges towards `server`.
//!
//! Scope decided by the RFC: compiled modules (`Arc<dyn Module>`) and declarative modules
//! read from a `ModuleStore`; no dynamic code loading (no stable Rust ABI).

pub mod loader;

pub use loader::{ModuleInterface, ModuleLoader, ModuleSource, ModuleStatus};
