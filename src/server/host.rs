//! Server host for transport-agnostic API exposure
//!
//! This module provides a `ServerHost` structure that contains all framework state
//! needed to expose the API via any protocol (REST, GraphQL, gRPC, etc.)
//!
//! The host is completely agnostic to the transport protocol and serves as the
//! single source of truth for the application state.

use crate::config::LinksConfig;
use crate::core::events::EventBus;
use crate::core::{EntityCreator, EntityFetcher, service::LinkService};
use crate::links::registry::LinkRouteRegistry;
use crate::server::entity_registry::EntityRegistry;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

/// Host context containing all framework state
///
/// This structure is transport-agnostic and contains all the information
/// needed to expose the API via any protocol (REST, GraphQL, gRPC, etc.)
///
/// # Example
///
/// ```rust,ignore
/// let host = ServerHost::from_builder_components(
///     link_service,
///     config,
///     entity_registry,
///     fetchers,
///     creators,
/// )?;
///
/// // Use host with any exposure
/// let host_arc = Arc::new(host);
/// let rest_app = RestExposure::build_router(host_arc.clone())?;
/// let graphql_app = GraphQLExposure::build_router(host_arc)?;
/// ```
pub struct ServerHost {
    /// Merged configuration from all modules
    pub config: Arc<LinksConfig>,

    /// Link service for relationship management
    pub link_service: Arc<dyn LinkService>,

    /// Link route registry for semantic URL resolution
    pub registry: Arc<LinkRouteRegistry>,

    /// Entity registry for CRUD routes
    pub entity_registry: EntityRegistry,

    /// Entity fetchers map (for link enrichment)
    pub entity_fetchers: Arc<HashMap<String, Arc<dyn EntityFetcher>>>,

    /// Entity creators map (for automatic entity + link creation)
    pub entity_creators: Arc<HashMap<String, Arc<dyn EntityCreator>>>,

    /// Optional event bus for real-time notifications (WebSocket, SSE)
    ///
    /// When present, REST/GraphQL handlers will publish events for mutations.
    /// WebSocket and other real-time exposures subscribe to this bus.
    pub event_bus: Option<Arc<EventBus>>,
}

impl ServerHost {
    /// Build the host from builder components
    ///
    /// This method takes all the components that have been registered with
    /// the builder and constructs the host structure.
    ///
    /// # Arguments
    ///
    /// * `link_service` - The link service for relationship management
    /// * `config` - Merged configuration from all modules
    /// * `entity_registry` - Registry of all entity descriptors
    /// * `fetchers` - Map of entity type to fetcher implementation
    /// * `creators` - Map of entity type to creator implementation
    ///
    /// # Returns
    ///
    /// Returns a `ServerHost` ready to be used with any exposure (REST, GraphQL, gRPC, etc.)
    pub fn from_builder_components(
        link_service: Arc<dyn LinkService>,
        config: LinksConfig,
        entity_registry: EntityRegistry,
        fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
        creators: HashMap<String, Arc<dyn EntityCreator>>,
    ) -> Result<Self> {
        let config = Arc::new(config);
        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));

        Ok(Self {
            config,
            link_service,
            registry,
            entity_registry,
            entity_fetchers: Arc::new(fetchers),
            entity_creators: Arc::new(creators),
            event_bus: None,
        })
    }

    /// Get entity types registered in the host
    pub fn entity_types(&self) -> Vec<&str> {
        self.entity_registry.entity_types()
    }

    /// Check if host is properly initialized
    pub fn is_ready(&self) -> bool {
        !self.entity_fetchers.is_empty()
    }

    /// Set the event bus for real-time notifications
    pub fn with_event_bus(mut self, event_bus: EventBus) -> Self {
        self.event_bus = Some(Arc::new(event_bus));
        self
    }

    /// Get a reference to the event bus (if configured)
    pub fn event_bus(&self) -> Option<&Arc<EventBus>> {
        self.event_bus.as_ref()
    }
}
