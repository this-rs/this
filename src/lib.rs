//! # This-RS Framework
//!
//! A generic entity and relationship management framework for building RESTful APIs in Rust.
//!
//! ## Features
//!
//! - **Entity/Data/Link Architecture**: Clean hierarchy with macro-based implementation
//! - **Flexible Relationships**: Support multiple link types between entities
//! - **Bidirectional Navigation**: Query relationships from both directions
//! - **Auto-Pluralization**: Intelligent plural forms (company → companies)
//! - **Configuration-Based**: Define relationships via YAML configuration
//! - **Type-Safe**: Leverage Rust's type system for compile-time guarantees
//! - **Soft Delete Support**: Built-in soft deletion with deleted_at
//! - **Automatic Timestamps**: created_at and updated_at managed automatically
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use this::prelude::*;
//!
//! // Define a Data entity (extends Entity base)
//! impl_data_entity!(
//!     User,
//!     "user",
//!     ["name", "email"],
//!     {
//!         email: String,
//!         password_hash: String,
//!     }
//! );
//!
//! // Define a Link entity (extends Entity base)
//! impl_link_entity!(
//!     UserCompanyLink,
//!     "user_company_link",
//!     {
//!         role: String,
//!         start_date: DateTime<Utc>,
//!     }
//! );
//!
//! // Usage
//! let user = User::new(
//!     "John Doe".to_string(),
//!     "active".to_string(),
//!     "john@example.com".to_string(),
//!     "$argon2$...".to_string(),
//! );
//!
//! user.soft_delete(); // Soft delete support
//! user.restore();     // Restore support
//! ```

pub mod config;
pub mod core;
pub mod entities;
pub mod links;
pub mod server;
pub mod storage;

/// Re-exports of commonly used types and traits
pub mod prelude {
    // === Core Traits ===
    pub use crate::core::{
        auth::{AuthContext, AuthPolicy, AuthProvider, NoAuthProvider},
        entity::{Data, Entity, Link},
        field::{FieldFormat, FieldValue},
        link::{LinkAuthConfig, LinkDefinition, LinkEntity},
        module::{EntityCreator, EntityFetcher, Module},
        pluralize::Pluralizer,
        service::{DataService, LinkService},
    };

    // === Macros ===
    pub use crate::{data_fields, entity_fields, impl_data_entity, impl_link_entity, link_fields};

    // === Link Handlers ===
    pub use crate::links::{
        handlers::{
            AppState, create_link, delete_link, get_link, list_available_links, list_links,
            update_link,
        },
        registry::{LinkDirection, LinkRouteRegistry, RouteInfo},
    };

    // === Storage ===
    pub use crate::storage::InMemoryLinkService;
    #[cfg(feature = "dynamodb")]
    pub use crate::storage::{DynamoDBDataService, DynamoDBLinkService};

    // === Config ===
    pub use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig, ValidationRule};

    // === Server ===
    pub use crate::server::{EntityDescriptor, EntityRegistry, ServerBuilder};

    // === External dependencies ===
    pub use anyhow::Result;
    pub use async_trait::async_trait;
    pub use chrono::{DateTime, Utc};
    pub use serde::{Deserialize, Serialize};
    pub use uuid::Uuid;

    // === Axum ===
    pub use axum::{
        Router,
        extract::{Path, State},
        http::HeaderMap,
        routing::{delete, get, post, put},
    };
}
