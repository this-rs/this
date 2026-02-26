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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig};
    use crate::core::link::LinkEntity;

    /// Minimal mock LinkService for testing
    struct MockLinkService;

    #[async_trait::async_trait]
    impl crate::core::service::LinkService for MockLinkService {
        async fn create(&self, link: LinkEntity) -> anyhow::Result<LinkEntity> {
            Ok(link)
        }
        async fn get(&self, _id: &uuid::Uuid) -> anyhow::Result<Option<LinkEntity>> {
            Ok(None)
        }
        async fn list(&self) -> anyhow::Result<Vec<LinkEntity>> {
            Ok(vec![])
        }
        async fn find_by_source(
            &self,
            _source_id: &uuid::Uuid,
            _link_type: Option<&str>,
            _target_type: Option<&str>,
        ) -> anyhow::Result<Vec<LinkEntity>> {
            Ok(vec![])
        }
        async fn find_by_target(
            &self,
            _target_id: &uuid::Uuid,
            _link_type: Option<&str>,
            _source_type: Option<&str>,
        ) -> anyhow::Result<Vec<LinkEntity>> {
            Ok(vec![])
        }
        async fn update(
            &self,
            _id: &uuid::Uuid,
            link: LinkEntity,
        ) -> anyhow::Result<LinkEntity> {
            Ok(link)
        }
        async fn delete(&self, _id: &uuid::Uuid) -> anyhow::Result<()> {
            Ok(())
        }
        async fn delete_by_entity(&self, _entity_id: &uuid::Uuid) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn test_config() -> LinksConfig {
        LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
        }
    }

    fn make_host() -> ServerHost {
        ServerHost::from_builder_components(
            Arc::new(MockLinkService),
            test_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .expect("should build host")
    }

    #[test]
    fn test_from_builder_components_creates_host() {
        let host = make_host();
        assert!(host.event_bus.is_none());
    }

    #[test]
    fn test_entity_types_empty_registry() {
        let host = make_host();
        assert!(host.entity_types().is_empty());
    }

    #[test]
    fn test_is_ready_no_fetchers_returns_false() {
        let host = make_host();
        assert!(!host.is_ready());
    }

    #[test]
    fn test_with_event_bus_sets_bus() {
        let host = make_host();
        let bus = EventBus::new(16);
        let host = host.with_event_bus(bus);
        assert!(host.event_bus().is_some());
    }

    #[test]
    fn test_event_bus_none_by_default() {
        let host = make_host();
        assert!(host.event_bus().is_none());
    }

    #[test]
    fn test_config_accessible_from_host() {
        let host = make_host();
        assert_eq!(host.config.entities.len(), 1);
        assert_eq!(host.config.entities[0].singular, "order");
    }

    #[test]
    fn test_registry_built_from_config() {
        let host = make_host();
        // Registry should exist and be built from config
        let routes = host.registry.list_routes_for_entity("order");
        // No links → no routes, but it shouldn't panic
        assert!(routes.is_empty());
    }
}
