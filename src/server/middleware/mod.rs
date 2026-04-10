//! Middleware layers for the server
//!
//! Contains authentication, authorization, and tenant resolution middleware
//! that can be auto-wired based on configuration.

pub mod auth;
pub mod tenant;

pub use auth::AuthLayer;
pub use tenant::tenant_resolver_middleware;
