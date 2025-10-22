//! # This-RS Framework
//!
//! A generic entity and relationship management framework for building RESTful APIs in Rust.
//!
//! ## Features
//!
//! - **Generic Entity System**: Define entities without modifying core framework code
//! - **Flexible Relationships**: Support multiple link types between same entities
//! - **Bidirectional Navigation**: Query relationships from both directions
//! - **Auto-Pluralization**: Intelligent plural forms (company → companies)
//! - **Configuration-Based**: Define relationships via YAML configuration
//! - **Multi-tenant Support**: Built-in tenant isolation
//! - **Type-Safe**: Leverage Rust's type system for compile-time guarantees
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use this::prelude::*;
//!
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! struct User {
//!     id: Uuid,
//!     tenant_id: Uuid,
//!     name: String,
//!     email: String,
//! }
//!
//! impl_data_entity!(User, "user", ["name", "email"]);
//! ```

pub mod config;
pub mod core;
pub mod entities;
pub mod links;

/// Re-exports of commonly used types and traits
pub mod prelude {
    pub use crate::core::{
        entity::{Data, Entity},
        extractors::{extract_tenant_id, DirectLinkExtractor, ExtractorError, LinkExtractor},
        field::{FieldFormat, FieldValue},
        link::{EntityReference, Link, LinkDefinition},
        pluralize::Pluralizer,
        service::{DataService, LinkService},
    };

    pub use crate::links::{
        handlers::{create_link, delete_link, list_available_links, list_links, AppState},
        registry::{LinkDirection, LinkRouteRegistry, RouteInfo},
        service::InMemoryLinkService,
    };

    pub use crate::config::{EntityConfig, LinksConfig};

    // Re-export common external dependencies
    pub use anyhow::Result;
    pub use async_trait::async_trait;
    pub use chrono::{DateTime, Utc};
    pub use serde::{Deserialize, Serialize};
    pub use uuid::Uuid;

    // Re-export Axum types for convenience
    pub use axum::{
        extract::{Path, State},
        http::HeaderMap,
        routing::{delete, get, post},
        Router,
    };
}

// Re-export macros at crate root for easier access
pub use entities::macros::*;
