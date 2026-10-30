//! Server host for transport-agnostic API exposure
//!
//! This module provides a `ServerHost` structure that contains all framework state
//! needed to expose the API via any protocol (REST, GraphQL, gRPC, etc.)
//!
//! The host is completely agnostic to the transport protocol and serves as the
//! single source of truth for the application state.

use crate::config::LinksConfig;
use crate::core::auth::{AuthProvider, AuthResolverRegistry};
use crate::core::events::EventBus;
use crate::core::identity::IdentityRegistry;
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

    /// Live catalog (descriptors, fetchers, creators, links config, plurals) published
    /// atomically. Equal to the fields above at construction; runtime module loading
    /// publishes new snapshots here. New readers MUST go through it (see `server::catalog`).
    pub catalog: Arc<crate::server::catalog::LiveCatalog>,

    /// Runtime module loader of this host (`ServerBuilder::with_module_runtime`). `None` =
    /// the host is frozen at build time (historical behaviour, no admin routes).
    pub module_runtime: Option<Arc<crate::server::module_runtime::ModuleRuntime>>,

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
    /// État d'interface exposé (`/ui/state/{portee}`) — voir `crate::ui`.
    ///
    /// Absent = aucune route d'interface : mieux vaut pas de surface du tout
    /// qu'une surface qui devrait inventer un état qu'elle ne détient pas.
    pub ui_state: Option<Arc<crate::ui::state::UiStateStore>>,

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

    /// Optional obrain graph handle — the relief source.
    ///
    /// When present, `RestExposure` auto-wires the self-describing `/graph`
    /// surface (`crate::graph::routes`), which publishes what the engine can
    /// actually do: procedures introspected from obrain's registry, the
    /// declared dispatch families, the simplicial layer state, and the known
    /// limits. Absent = no `/graph` routes at all (no half-truths).
    #[cfg(feature = "obrain")]
    pub graph_store: Option<Arc<crate::storage::ObrainStore>>,

    /// How identities are resolved for the entities this host serves.
    ///
    /// Engine-agnostic by construction (see `crate::core::identity`): the default
    /// registry resolves only the `_uuid`/`_data` convention this-rs writes, so
    /// existing deployments behave exactly as before. Attach
    /// `IdentityRegistry::obrain_default()` to also resolve entities from stores
    /// written by another tool (flat properties, no `_uuid`) — which is what
    /// makes it *the same entity whatever the engine*.
    pub identity: Arc<IdentityRegistry>,

    /// L'application des politiques se fait-elle en **observation** ?
    ///
    /// `true` = chaque refus est calculé, journalisé et **publié dans la réponse**
    /// (en-tête `x-this-rs-authz`), mais la requête passe. C'est le mode
    /// d'adoption d'un projet existant : on voit ce qui serait refusé sur le
    /// trafic réel avant d'armer. Voir `ServerBuilder::authorization_report_only`.
    pub authorize_report_only: bool,

    /// Le mappage d'identité fourni par l'application, s'il y en a un.
    ///
    /// Quand il est présent, il remplace le mappage de this-rs pour toutes les
    /// politiques `perm:` — c'est ce qui rend l'IAM **natif** pour un modèle
    /// d'authentification plus ancien.
    pub principal_mapping: Option<Arc<crate::server::middleware::authorization::PrincipalMapping>>,

    /// **Le repli multi-sites**, quand l'application en déclare un.
    ///
    /// Monté par `RestExposure::build_router` **avant** les couches : c'est ce
    /// qui le rend couvert par le CORS, la garde de domaine,
    /// l'authentification et les politiques. Un repli posé après elles les
    /// contourne (c'est le piège mesuré dans `tests/frontend_surface.rs`).
    #[cfg(feature = "frontend")]
    pub frontend_sites: Option<Arc<crate::server::frontend::FrontendSites>>,

    /// **Exposition** : CORS, domaines servis, délégation de proxy.
    ///
    /// Voir [`crate::config::ServerSecurityConfig`] et `config/server.yaml`.
    /// `ServerHost::caddyfile()` en rend le `Caddyfile` délégué.
    pub security: Arc<crate::config::ServerSecurityConfig>,

    /// Les politiques déclarées (`auth:`) sont-elles **appliquées** ?
    ///
    /// **`true` par défaut** : un framework qui laisse passer par défaut n'est
    /// pas sûr par défaut. Se désarme sciemment
    /// (`ServerBuilder::without_authorization`) — et l'appel est journalisé.
    /// Voir `server::middleware::authorization`.
    pub authorize: bool,

    /// Optional auth provider for JWT/token verification
    ///
    /// When present, the auth middleware is auto-wired on all routes.
    /// The provider extracts AuthContext from incoming requests.
    pub auth_provider: Option<Arc<dyn AuthProvider>>,

    /// Registry of custom auth resolvers
    ///
    /// Named resolvers referenced from YAML config as `resolver:name`.
    /// Registered via `ServerBuilder::with_auth_resolver()`.
    pub resolver_registry: Arc<AuthResolverRegistry>,

    /// Custom validators / transforms used by the descriptor validation (`ValidatingCreator`
    /// and the REST validation layer). Shared through an `ArcSwap`: replacing it with
    /// [`ServerHost::with_custom_registry`] reaches creators already wrapped.
    pub validation_registry: crate::core::validation::SharedRegistry,

    /// Message catalog of the host (framework bundles + module bundles), shared through an
    /// `ArcSwap`: every surface renders a `ContractError` in the request locale from it
    /// (REST `localize_errors`, GraphQL handler, gRPC services). Runtime modules add their
    /// bundles on load and remove them on unload.
    pub locales: crate::core::i18n::LocaleCatalog,

    /// Optional STS (Security Token Service) state for embedded token issuance
    ///
    /// When present, auth endpoints (POST /auth/token, GET /auth/keys,
    /// POST /auth/refresh, POST /auth/revoke) are auto-wired.
    /// Only available in `embedded` or `bootstrap` WAMI modes.
    #[cfg(feature = "wami")]
    pub sts_state: Option<Arc<crate::server::exposure::rest::auth_routes::StsState>>,
}

/// Ce que l'application applique à l'entrée HTTP — sans divulguer ses listes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExposureSummary {
    /// Une politique CORS est-elle posée ?
    pub cors_enabled: bool,
    /// Combien d'origines sont autorisées.
    pub cors_origins: usize,
    /// Les identifiants (cookies, `Authorization`) sont-ils autorisés en CORS ?
    pub cors_credentials: bool,
    /// Une garde de domaine est-elle active (les autres hôtes sont refusés) ?
    pub domains_guarded: bool,
    /// Combien d'hôtes sont servis.
    pub hosts_allowed: usize,
    /// Combien de domaines désignent un locataire.
    pub tenants_mapped: usize,
    /// Les sous-domaines désignent-ils le locataire ?
    pub subdomain_tenants: bool,
    /// `X-Forwarded-Host` est-il cru (délégation à un proxy) ?
    pub forwarded_host_trusted: bool,
    /// Une délégation `Caddyfile` est-elle configurée ?
    pub caddy_delegated: bool,
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
        let fetchers = Arc::new(fetchers);
        let creators = Arc::new(creators);
        let catalog = Arc::new(crate::server::catalog::LiveCatalog::new(
            crate::server::catalog::HostCatalog {
                descriptors: Arc::new(crate::descriptor::DescriptorSet::new()),
                fetchers: fetchers.clone(),
                creators: creators.clone(),
                links_config: config.clone(),
                plurals: Arc::new(entity_registry.entity_plurals()),
                generation: 0,
            },
        ));

        Ok(Self {
            config,
            link_service,
            registry,
            entity_registry,
            entity_fetchers: fetchers,
            entity_creators: creators,
            catalog,
            module_runtime: None,
            event_bus: None,
            event_log: None,
            sink_registry: None,
            notification_store: None,
            ui_state: None,
            device_token_store: None,
            preferences_store: None,
            #[cfg(feature = "obrain")]
            graph_store: None,
            identity: Arc::new(IdentityRegistry::new()),
            #[cfg(feature = "frontend")]
            frontend_sites: None,
            security: Arc::new(crate::config::ServerSecurityConfig::default()),
            authorize: true,
            authorize_report_only: false,
            principal_mapping: None,
            auth_provider: None,
            resolver_registry: Arc::new(AuthResolverRegistry::new()),
            validation_registry: crate::core::validation::shared_registry(),
            locales: crate::core::i18n::LocaleCatalog::framework(),
            #[cfg(feature = "wami")]
            sts_state: None,
        })
    }

    /// Publish the active descriptors in the live catalog (called ONCE by `build_host`).
    ///
    /// Validation stream: the creator of every DESCRIBED entity is wrapped in
    /// `ValidatingCreator` here, in both `entity_creators` and the catalog, so GraphQL,
    /// gRPC and the link handlers validate through the same engine. Undescribed entities
    /// keep their creator untouched. Expects the RAW creators (a second call would wrap
    /// twice: harmless, transforms are idempotent, but wasted).
    pub fn with_descriptors(mut self, descriptors: crate::descriptor::DescriptorSet) -> Self {
        let creators = Arc::new(crate::core::validation::wrap_creators(
            &self.entity_creators,
            &descriptors,
            &self.validation_registry,
        ));
        self.entity_creators = creators.clone();
        let mut next = (*self.catalog.load()).clone();
        next.creators = creators;
        next.descriptors = Arc::new(descriptors);
        self.catalog.publish(next);
        self
    }

    /// Replace the message catalog (framework bundles plus the application's own).
    pub fn with_locales(mut self, locales: crate::core::i18n::LocaleCatalog) -> Self {
        self.locales = locales;
        self
    }

    /// Custom validators / transforms for the descriptor validation. May be called before
    /// or after `with_descriptors`: wrapped creators read the registry at each call.
    pub fn with_custom_registry(self, registry: crate::descriptor::CustomRegistry) -> Self {
        self.validation_registry.store(Arc::new(registry));
        self
    }

    /// State of the REST validation layer: reads the CURRENT catalog at each request, so a
    /// module published at runtime is validated from its first request.
    pub fn validation_layer_state(&self) -> crate::core::validation::ValidationLayerState {
        let catalog = self.catalog.clone();
        crate::core::validation::ValidationLayerState::new(
            Arc::new(move || catalog.load().descriptors.clone()),
            self.validation_registry.clone(),
        )
    }

    /// Current snapshot of the live catalog (one atomic load; keep it for the request).
    pub fn catalog(&self) -> Arc<crate::server::catalog::HostCatalog> {
        self.catalog.load()
    }

    /// THE access to the fetcher of `entity` for `ctx` running `op`: the policy is judged
    /// (same evaluator as REST), then the fetcher comes from the LIVE catalog. GraphQL,
    /// gRPC, WebSocket and links reach fetchers only through it (or through
    /// `SurfaceGuard::fetcher`, which shares the evaluation of one request).
    #[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
    #[allow(clippy::result_large_err)] // `ContractError` is the cross-surface contract
    pub fn guarded_fetcher(
        &self,
        entity: &str,
        op: crate::descriptor::Operation,
        ctx: &crate::core::auth::AuthContext,
    ) -> Result<Arc<dyn EntityFetcher>, crate::descriptor::ContractError> {
        crate::server::exposure::SurfaceGuard::for_host(self, ctx.clone(), "host")
            .fetcher(self, entity, op, None)
    }

    /// Creator counterpart of [`ServerHost::guarded_fetcher`].
    #[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
    #[allow(clippy::result_large_err)]
    pub fn guarded_creator(
        &self,
        entity: &str,
        op: crate::descriptor::Operation,
        ctx: &crate::core::auth::AuthContext,
    ) -> Result<Arc<dyn EntityCreator>, crate::descriptor::ContractError> {
        crate::server::exposure::SurfaceGuard::for_host(self, ctx.clone(), "host")
            .creator(self, entity, op, None)
    }

    /// Fetcher for SCHEMA DERIVATION only (SDL, proto: sample entity to infer field names
    /// and types). No caller, no policy: the schema endpoints are descriptive. Never use it
    /// to serve data (the coverage test `no_entity_fetchers_access_outside_guard` pins it).
    #[cfg(any(feature = "graphql", feature = "grpc"))]
    pub(crate) fn schema_fetcher(&self, entity: &str) -> Option<Arc<dyn EntityFetcher>> {
        self.catalog.load().fetchers.get(entity).cloned()
    }

    /// Active descriptors (current snapshot).
    pub fn descriptors(&self) -> Arc<crate::descriptor::DescriptorSet> {
        self.catalog.load().descriptors.clone()
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
    /// Attache le store d'état d'interface (routes `/ui/state/{portee}`).
    pub fn with_ui_state(mut self, store: Arc<crate::ui::state::UiStateStore>) -> Self {
        self.ui_state = Some(store);
        self
    }

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

    /// Attach an obrain graph handle — enables the self-describing `/graph` surface.
    ///
    /// ```rust,ignore
    /// let store = ObrainStore::open("./data/app.obrain")?;
    /// let host = ServerBuilder::new()
    ///     .with_link_service(ObrainLinkService::new(store.clone()))
    ///     .with_graph_store(store)
    ///     .build_host()?;
    /// // GET /graph now publishes the relief.
    /// ```
    #[cfg(feature = "obrain")]
    pub fn with_graph_store(mut self, store: Arc<crate::storage::ObrainStore>) -> Self {
        self.graph_store = Some(store);
        self
    }

    /// Get a reference to the obrain graph handle (if attached)
    #[cfg(feature = "obrain")]
    pub fn graph_store(&self) -> Option<&Arc<crate::storage::ObrainStore>> {
        self.graph_store.as_ref()
    }

    /// Attach an identity registry — how entities are named across engines.
    ///
    /// ```rust,ignore
    /// let host = ServerBuilder::new()
    ///     .with_link_service(links)
    ///     .with_graph_store(store)
    ///     .with_identity_registry(IdentityRegistry::obrain_default())
    ///     .build_host()?;
    /// ```
    pub fn with_identity_registry(mut self, registry: IdentityRegistry) -> Self {
        self.identity = Arc::new(registry);
        self
    }

    /// The identity registry in force.
    pub fn identity_registry(&self) -> &Arc<IdentityRegistry> {
        &self.identity
    }

    /// Set the auth provider
    pub fn with_auth_provider(mut self, provider: Arc<dyn AuthProvider>) -> Self {
        self.auth_provider = Some(provider);
        self
    }

    /// Get a reference to the auth provider (if configured)
    pub fn auth_provider(&self) -> Option<&Arc<dyn AuthProvider>> {
        self.auth_provider.as_ref()
    }

    /// Set the auth resolver registry
    ///
    /// The registry contains named resolvers that can be referenced from
    /// YAML config as `resolver:name`.
    pub fn with_resolver_registry(mut self, registry: AuthResolverRegistry) -> Self {
        self.resolver_registry = Arc::new(registry);
        self
    }

    /// Get a reference to the auth resolver registry
    pub fn resolver_registry(&self) -> &Arc<AuthResolverRegistry> {
        &self.resolver_registry
    }

    /// Set the STS state for embedded token issuance (WAMI embedded/bootstrap modes)
    #[cfg(feature = "wami")]
    pub fn with_sts_state(
        mut self,
        state: Arc<crate::server::exposure::rest::auth_routes::StsState>,
    ) -> Self {
        self.sts_state = Some(state);
        self
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
            auth: None,
        };
        let config = Arc::new(config);
        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));
        let catalog = Arc::new(crate::server::catalog::LiveCatalog::new(
            crate::server::catalog::HostCatalog {
                descriptors: Arc::new(crate::descriptor::DescriptorSet::new()),
                fetchers: Arc::new(HashMap::new()),
                creators: Arc::new(HashMap::new()),
                links_config: config.clone(),
                plurals: Arc::new(vec![]),
                generation: 0,
            },
        ));

        Self {
            config,
            link_service: Arc::new(NoopLinkService),
            registry,
            entity_registry: EntityRegistry::new(),
            entity_fetchers: Arc::new(HashMap::new()),
            entity_creators: Arc::new(HashMap::new()),
            catalog,
            module_runtime: None,
            event_bus: None,
            event_log: None,
            sink_registry: None,
            notification_store: None,
            device_token_store: None,
            preferences_store: None,
            #[cfg(feature = "obrain")]
            graph_store: None,
            identity: Arc::new(IdentityRegistry::new()),
            #[cfg(feature = "frontend")]
            frontend_sites: None,
            security: Arc::new(crate::config::ServerSecurityConfig::default()),
            authorize: true,
            authorize_report_only: false,
            principal_mapping: None,
            auth_provider: None,
            resolver_registry: Arc::new(AuthResolverRegistry::new()),
            validation_registry: crate::core::validation::shared_registry(),
            locales: crate::core::i18n::LocaleCatalog::framework(),
            #[cfg(feature = "wami")]
            sts_state: None,
            ui_state: None,
        }
    }
    /// **Résumé de l'exposition** — ce que l'application applique à l'entrée.
    ///
    /// Des **comptes et des faits**, jamais les valeurs : la liste des origines
    /// CORS et celle des domaines servis appartiennent à l'application, et les
    /// publier dirait à un inconnu quels hôtes viser. Un opérateur qui veut les
    /// valeurs lit sa configuration ; un consommateur qui veut savoir *si* une
    /// garde est active lit ceci.
    pub fn exposure(&self) -> ExposureSummary {
        let domains = &self.security.domains;
        ExposureSummary {
            cors_enabled: self.security.cors.enabled,
            cors_origins: self.security.cors.origins.len(),
            cors_credentials: self.security.cors.credentials,
            domains_guarded: domains.guards(),
            hosts_allowed: domains.allowed.len(),
            tenants_mapped: domains.tenants.len(),
            subdomain_tenants: domains.subdomain_base.is_some(),
            forwarded_host_trusted: domains.trust_forwarded_host,
            caddy_delegated: self.security.proxy.caddy.enabled,
        }
    }

    /// Le `Caddyfile` **délégué** par la configuration, s'il est demandé.
    ///
    /// this-rs ne pilote pas Caddy : il décrit ce qu'il applique (domaines,
    /// upstream, en-têtes) et l'exploitant relit puis applique.
    pub fn caddyfile(&self) -> Option<String> {
        self.security.caddyfile()
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
            auth: None,
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

    #[tokio::test]
    async fn with_descriptors_wraps_described_creators_and_registry_is_live() {
        use crate::core::validation::gate::tests::{RecordingCreator, billing};
        use crate::descriptor::{Applies, Custom, CustomScope, DescriptorSet, ValidatorSpec};
        use std::sync::Arc;

        let mut m = billing();
        m.entities[0].fields[1]
            .validators
            .push(crate::descriptor::ValidatorRule {
                rule: ValidatorSpec::Custom(Custom::named("billing.even")),
                on: Applies::Always,
            });
        let mut set = DescriptorSet::new();
        set.insert(m).unwrap();

        let rec = Arc::new(RecordingCreator::default());
        let legacy: Arc<dyn EntityCreator> = Arc::new(RecordingCreator::default());
        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("invoice".into(), rec.clone());
        creators.insert("legacy".into(), legacy.clone());
        let host = ServerHost::from_builder_components(
            Arc::new(MockLinkService),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            creators,
        )
        .unwrap()
        .with_descriptors(set);

        // Same wrapped creator in the legacy field and in the catalog.
        let snapshot = host.catalog.load();
        assert!(Arc::ptr_eq(
            &host.entity_creators["invoice"],
            &snapshot.creators["invoice"]
        ));
        assert!(Arc::ptr_eq(&host.entity_creators["legacy"], &legacy));

        // Custom validator without implementation: fail-closed (internal), store untouched.
        let payload = serde_json::json!({"number": "abc", "amount": 3});
        let err = host.entity_creators["invoice"]
            .create_from_json(payload.clone())
            .await
            .unwrap_err();
        assert_eq!(
            crate::core::errors::from_anyhow(&err).code.as_str(),
            "this.internal"
        );

        // Registry installed AFTER wrapping: reached through the ArcSwap.
        let mut reg = crate::descriptor::CustomRegistry::default();
        reg.register_validator(
            "billing.even",
            CustomScope::Both,
            Arc::new(|f, v, _| {
                if v.as_f64().is_some_and(|x| x % 2.0 == 0.0) {
                    Ok(())
                } else {
                    Err(crate::descriptor::FieldViolation::new(
                        f,
                        "billing.not_even",
                    ))
                }
            }),
        );
        let host = host.with_custom_registry(reg);
        let err = host.entity_creators["invoice"]
            .create_from_json(payload)
            .await
            .unwrap_err();
        assert_eq!(
            crate::core::errors::from_anyhow(&err).fields[0]
                .code
                .as_str(),
            "billing.not_even"
        );
        host.entity_creators["invoice"]
            .create_from_json(serde_json::json!({"number": "abc", "amount": 4}))
            .await
            .unwrap();
        assert_eq!(rec.seen.lock().unwrap().len(), 1);
    }
}
