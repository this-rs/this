//! `this-forge` : le service qui compile du Rust en wasm pour des comptes.
//! Voir le README du crate et `docs/modules-wasm-abi.md` §10.

pub mod api;
pub mod auth;
pub mod cli;
pub mod config;
pub mod entities;
pub mod runner;
pub mod service;
pub mod sign;
pub mod store;
pub mod wasm_check;

pub use config::{ForgeConfig, StoreKind};
pub use service::Forge;

use std::sync::Arc;

/// L'application complète : le routeur de la forge porté par le `ServerBuilder` du
/// framework (santé, couches communes) ; les routes `/v1/*` sont celles de la forge.
pub fn app(forge: Arc<Forge>) -> anyhow::Result<axum::Router> {
    this::server::ServerBuilder::new()
        .with_link_service(this::storage::InMemoryLinkService::new())
        // La garde d'autorisation du framework est par ENTITÉ (pluriel enregistré) ; la forge
        // n'expose aucune entité en CRUD générique, et authentifie elle-même chaque route
        // (`api::Authed`). Sans cette ligne, la garde refuse tout `/v1/*` (deny_abstain).
        .without_authorization()
        .with_custom_routes(api::router(forge))
        .build()
}
