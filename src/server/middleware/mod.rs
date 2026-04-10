//! Middleware layers for the server
//!
//! Contains authentication and authorization middleware that can be
//! auto-wired based on configuration.

pub mod auth;

pub use auth::AuthLayer;
