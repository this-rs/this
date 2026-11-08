//! SDK invité des modules `this` compilés (ABI v1, `docs/modules-wasm-abi.md`).
//!
//! Un module est une **fonction pure** : `handle(Request) -> Response`. Il ne touche ni le
//! disque, ni l'horloge, ni le réseau (aucun import wasm) : ce qu'il lui faut — lignes, temps,
//! configuration — est dans la requête ; ce qu'il décide sort en `Effect`s que l'hôte applique.
//!
//! ```ignore
//! use this_module_sdk::{Module, Manifest, Request, Response};
//! pub struct Mon;
//! impl Module for Mon {
//!     fn manifest() -> Manifest { /* ... */ }
//!     fn handle(req: Request) -> Response { /* ... */ }
//! }
//! this_module_sdk::export!(Mon);
//! ```
//!
//! Pas de proc-macro, pas de `build.rs` : la forge compile ce code hors ligne.

pub mod manifest;
pub mod module;
pub mod rt;
pub mod wire;

pub use manifest::{Entity, Field, FieldType, Manifest, OperationDecl, Transform, Validator};
pub use module::Module;
pub use wire::{Ctx, Effect, Error, Level, Op, Request, Response};
// Réexportés pour que le module n'ait pas à déclarer `serde_json` pour `json!`.
pub use serde_json::{self, Value, json};
