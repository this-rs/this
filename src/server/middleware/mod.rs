//! Middleware layers for the server
//!
//! Contains authentication, authorization, and tenant resolution middleware
//! that can be auto-wired based on configuration.

pub mod auth;
pub mod tenant;

pub use auth::AuthLayer;
/// Application des politiques d'autorisation (le point qui manquait).
pub mod authorization;
pub use authorization::{
    Authorization, Decision, PolicyOrigin, Requirement, authorization_middleware,
};
pub use tenant::tenant_resolver_middleware;
