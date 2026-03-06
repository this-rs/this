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
use crate::events::log::EventLog;
use crate::events::sinks::SinkRegistry;
use crate::events::sinks::device_tokens::DeviceTokenStore;
use crate::events::sinks::in_app::NotificationStore;
use crate::events::sinks::preferences::NotificationPreferencesStore;
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

    /// Optional persistent event log for durable event storage
    ///
    /// When present, the EventBus bridges events to this log for replay,
    /// consumer groups, and FlowRuntime processing.
    pub event_log: Option<Arc<dyn EventLog>>,

    /// Optional sink registry for event delivery pipelines
    ///
    /// Contains all registered sinks (in_app, push, webhook, websocket, counter).
    /// The FlowRuntime's `deliver` operator uses this to dispatch payloads.
    pub sink_registry: Option<Arc<SinkRegistry>>,

    /// Optional in-app notification store
    ///
    /// Provides list, mark_as_read, unread_count operations for notifications.
    /// Used by REST/GraphQL/gRPC notification endpoints.
    pub notification_store: Option<Arc<NotificationStore>>,

    /// Optional device token store for push notifications
    ///
    /// Stores push notification tokens (Expo, APNs, FCM) per user.
    /// Used by the push notification sink and device token endpoints.
    pub device_token_store: Option<Arc<DeviceTokenStore>>,

    /// Optional notification preferences store
    ///
    /// Stores per-user notification preferences (mute, disable types).
    /// Used by sinks to filter notifications and by preference endpoints.
    pub preferences_store: Option<Arc<NotificationPreferencesStore>>,
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
            event_log: None,
            sink_registry: None,
            notification_store: None,
            device_token_store: None,
            preferences_store: None,
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

    /// Set the persistent event log
    pub fn with_event_log(mut self, event_log: Arc<dyn EventLog>) -> Self {
        self.event_log = Some(event_log);
        self
    }

    /// Get a reference to the event log (if configured)
    pub fn event_log(&self) -> Option<&Arc<dyn EventLog>> {
        self.event_log.as_ref()
    }

    /// Set the sink registry
    pub fn with_sink_registry(mut self, registry: SinkRegistry) -> Self {
        self.sink_registry = Some(Arc::new(registry));
        self
    }

    /// Get a reference to the sink registry (if configured)
    pub fn sink_registry(&self) -> Option<&Arc<SinkRegistry>> {
        self.sink_registry.as_ref()
    }

    /// Set the notification store
    pub fn with_notification_store(mut self, store: Arc<NotificationStore>) -> Self {
        self.notification_store = Some(store);
        self
    }

    /// Get a reference to the notification store (if configured)
    pub fn notification_store(&self) -> Option<&Arc<NotificationStore>> {
        self.notification_store.as_ref()
    }

    /// Set the device token store
    pub fn with_device_token_store(mut self, store: Arc<DeviceTokenStore>) -> Self {
        self.device_token_store = Some(store);
        self
    }

    /// Get a reference to the device token store (if configured)
    pub fn device_token_store(&self) -> Option<&Arc<DeviceTokenStore>> {
        self.device_token_store.as_ref()
    }

    /// Set the notification preferences store
    pub fn with_preferences_store(mut self, store: Arc<NotificationPreferencesStore>) -> Self {
        self.preferences_store = Some(store);
        self
    }

    /// Get a reference to the notification preferences store (if configured)
    pub fn preferences_store(&self) -> Option<&Arc<NotificationPreferencesStore>> {
        self.preferences_store.as_ref()
    }

    /// Create a minimal `ServerHost` for unit tests.
    ///
    /// Has empty registries and a mock `LinkService`. Useful for testing
    /// services that only need the `EventBus` (e.g., `EventServiceImpl`).
    #[cfg(test)]
    pub fn minimal_for_test() -> Self {
        use crate::core::link::LinkEntity;

        struct NoopLinkService;

        #[async_trait::async_trait]
        impl crate::core::service::LinkService for NoopLinkService {
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

        let config = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
        };
        let config = Arc::new(config);
        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));

        Self {
            config,
            link_service: Arc::new(NoopLinkService),
            registry,
            entity_registry: EntityRegistry::new(),
            entity_fetchers: Arc::new(HashMap::new()),
            entity_creators: Arc::new(HashMap::new()),
            event_bus: None,
            event_log: None,
            sink_registry: None,
            notification_store: None,
            device_token_store: None,
            preferences_store: None,
        }
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
        async fn update(&self, _id: &uuid::Uuid, link: LinkEntity) -> anyhow::Result<LinkEntity> {
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
            events: None,
            sinks: None,
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

    #[test]
    fn test_is_ready_with_fetchers_returns_true() {
        use crate::core::EntityFetcher;

        struct StubFetcher;

        #[async_trait::async_trait]
        impl EntityFetcher for StubFetcher {
            async fn fetch_as_json(
                &self,
                _entity_id: &uuid::Uuid,
            ) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(StubFetcher));

        let host = ServerHost::from_builder_components(
            Arc::new(MockLinkService),
            test_config(),
            EntityRegistry::new(),
            fetchers,
            HashMap::new(),
        )
        .expect("should build host");

        assert!(host.is_ready());
    }

    #[test]
    fn test_entity_creators_accessible() {
        let host = make_host();
        assert!(host.entity_creators.is_empty());
    }

    #[test]
    fn test_link_service_accessible() {
        let host = make_host();
        // link_service should be accessible (Arc<dyn LinkService>)
        let _ = host.link_service.clone();
    }

    #[test]
    fn test_new_fields_none_by_default() {
        let host = make_host();
        assert!(host.event_log().is_none());
        assert!(host.sink_registry().is_none());
        assert!(host.notification_store().is_none());
        assert!(host.device_token_store().is_none());
        assert!(host.preferences_store().is_none());
    }

    #[test]
    fn test_with_notification_store() {
        use crate::events::sinks::in_app::NotificationStore;
        let host = make_host();
        let store = Arc::new(NotificationStore::new());
        let host = host.with_notification_store(store);
        assert!(host.notification_store().is_some());
    }

    #[test]
    fn test_with_device_token_store() {
        use crate::events::sinks::device_tokens::DeviceTokenStore;
        let host = make_host();
        let store = Arc::new(DeviceTokenStore::new());
        let host = host.with_device_token_store(store);
        assert!(host.device_token_store().is_some());
    }

    #[test]
    fn test_with_preferences_store() {
        use crate::events::sinks::preferences::NotificationPreferencesStore;
        let host = make_host();
        let store = Arc::new(NotificationPreferencesStore::new());
        let host = host.with_preferences_store(store);
        assert!(host.preferences_store().is_some());
    }

    #[test]
    fn test_with_sink_registry() {
        use crate::events::sinks::SinkRegistry;
        let host = make_host();
        let registry = SinkRegistry::new();
        let host = host.with_sink_registry(registry);
        assert!(host.sink_registry().is_some());
    }

    #[test]
    fn test_with_event_log() {
        use crate::events::memory::InMemoryEventLog;
        let host = make_host();
        let log = Arc::new(InMemoryEventLog::new());
        let host = host.with_event_log(log);
        assert!(host.event_log().is_some());
    }
}
