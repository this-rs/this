//! ServerBuilder for fluent API to build HTTP servers

use super::entity_registry::EntityRegistry;
use super::exposure::RestExposure;
use super::host::ServerHost;
use crate::config::LinksConfig;
use crate::core::auth::{AuthResolver, AuthResolverRegistry, FnResolver};
use crate::core::events::EventBus;
use crate::core::module::Module;
use crate::core::service::LinkService;
use crate::core::{EntityCreator, EntityFetcher};
use crate::events::SinkFactory;
use crate::events::sinks::SinkRegistry;
use crate::events::sinks::device_tokens::DeviceTokenStore;
use crate::events::sinks::in_app::NotificationStore;
use crate::events::sinks::preferences::NotificationPreferencesStore;
use anyhow::Result;
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Builder for creating HTTP servers with auto-registered routes
///
/// # Example
///
/// ```ignore
/// let app = ServerBuilder::new()
///     .with_link_service(InMemoryLinkService::new())
///     .register_module(MyModule)
///     .build()?;
/// ```
pub struct ServerBuilder {
    link_service: Option<Arc<dyn LinkService>>,
    entity_registry: EntityRegistry,
    configs: Vec<LinksConfig>,
    modules: Vec<Arc<dyn Module>>,
    custom_routes: Vec<Router>,
    event_bus: Option<EventBus>,

    // Manual overrides for event system stores
    sink_registry: Option<SinkRegistry>,
    notification_store: Option<Arc<NotificationStore>>,
    /// Store d'état d'interface (voir `crate::ui`).
    ui_state: Option<Arc<crate::ui::state::UiStateStore>>,
    device_token_store: Option<Arc<DeviceTokenStore>>,
    preferences_store: Option<Arc<NotificationPreferencesStore>>,

    // Obrain graph handle — the relief source for the `/graph` surface
    #[cfg(feature = "obrain")]
    graph_store: Option<Arc<crate::storage::ObrainStore>>,

    // Identité des entités — comment elles sont nommées, quel que soit le moteur
    identity: Option<crate::core::identity::IdentityRegistry>,

    // Armer l'application des politiques `auth:` (défaut : armé)
    authorize: bool,

    // Application en OBSERVATION (défaut : non) — l'adoption d'un projet existant
    authorize_report_only: bool,

    // Exposition : CORS, domaines servis, délégation de proxy (config/server.yaml)
    security: crate::config::ServerSecurityConfig,

    // Repli multi-sites (N domaines, N contenus), monté AVANT les couches
    #[cfg(feature = "frontend")]
    frontend_sites: Option<std::sync::Arc<crate::server::frontend::FrontendSites>>,

    // Mappage d'identité de l'application, pour l'IAM natif d'un modèle existant
    principal_mapping: Option<Arc<crate::server::middleware::authorization::PrincipalMapping>>,

    // Armer le bail d'écriture sur un store obrain inscriptible (défaut : armé)
    #[cfg_attr(not(feature = "obrain"), allow(dead_code))]
    fence_writes: bool,

    // Auth config loaded from external file
    auth_config: Option<crate::config::auth::AuthConfig>,

    // Custom auth resolvers registry
    resolver_registry: AuthResolverRegistry,

    // Runtime module loading (`with_module_runtime`); `None` = host frozen at build.
    module_runtime: Option<RuntimeSetup>,
}

/// What `with_module_runtime*` collected, consumed by `build_host`.
#[derive(Default)]
struct RuntimeSetup {
    options: crate::server::module_runtime::RuntimeOptions,
    loader: Option<Arc<dyn crate::server::module_runtime::ModuleLoader>>,
    store: Option<Arc<dyn crate::storage::vfs::ModuleStore>>,
    offers: Vec<(String, crate::server::module_runtime::ModuleSource)>,
}

impl ServerBuilder {
    /// Create a new ServerBuilder
    pub fn new() -> Self {
        Self {
            link_service: None,
            entity_registry: EntityRegistry::new(),
            configs: Vec::new(),
            modules: Vec::new(),
            custom_routes: Vec::new(),
            event_bus: None,
            sink_registry: None,
            notification_store: None,
            ui_state: None,
            device_token_store: None,
            preferences_store: None,
            #[cfg(feature = "obrain")]
            graph_store: None,
            identity: None,
            authorize: true,
            authorize_report_only: false,
            security: crate::config::ServerSecurityConfig::default(),
            #[cfg(feature = "frontend")]
            frontend_sites: None,
            principal_mapping: None,
            fence_writes: true,
            auth_config: None,
            resolver_registry: AuthResolverRegistry::new(),
            module_runtime: None,
        }
    }

    /// Enable runtime module loading on the host: `ServerHost::module_runtime`, the
    /// dispatch layer of hot-loaded routes and the admin interface
    /// (`RuntimeOptions::admin_prefix`, default `/admin/modules`, policy `admin_only`).
    /// Without this call the host is frozen at build time, exactly as before. Hot loading
    /// itself stays CLOSED (`this.module.hot_load_disabled`) until
    /// [`Self::with_hot_load_enabled`]`(true)` (decision: closed by default).
    pub fn with_module_runtime(mut self) -> Self {
        self.module_runtime
            .get_or_insert_with(RuntimeSetup::default);
        self
    }

    /// Same as [`Self::with_module_runtime`], with explicit options (drain grace, admin
    /// policy and prefix).
    pub fn with_module_runtime_options(
        mut self,
        options: crate::server::module_runtime::RuntimeOptions,
    ) -> Self {
        self.module_runtime
            .get_or_insert_with(RuntimeSetup::default)
            .options = options;
        self
    }

    /// `hot_load.enabled`: open (or close) runtime load / replace / unload and the admin
    /// interface. Closed by default, and `with_module_runtime*` never opens it implicitly:
    /// the host must say so explicitly. Enables the runtime (read-only admin routes stay).
    pub fn with_hot_load_enabled(mut self, enabled: bool) -> Self {
        self.module_runtime
            .get_or_insert_with(RuntimeSetup::default)
            .options
            .hot_load_enabled = enabled;
        self
    }

    /// Replace the default loader (`DefaultModuleLoader`), e.g. by one with plugin
    /// loaders (WASM/WAMI extension point) or a `CustomRegistry`. Enables the runtime.
    pub fn with_module_loader(
        mut self,
        loader: Arc<dyn crate::server::module_runtime::ModuleLoader>,
    ) -> Self {
        self.module_runtime
            .get_or_insert_with(RuntimeSetup::default)
            .loader = Some(loader);
        self
    }

    /// Store that admin `{"store": {...}}` load requests read from. Enables the runtime.
    pub fn with_module_store(mut self, store: Arc<dyn crate::storage::vfs::ModuleStore>) -> Self {
        self.module_runtime
            .get_or_insert_with(RuntimeSetup::default)
            .store = Some(store);
        self
    }

    /// Offer a compiled module for loading at runtime (NOT loaded at build): listed as
    /// `discovered`, loaded by `POST {admin_prefix}` with `{"module": "<name>"}` or by
    /// `ModuleRuntime::load_offered`. Enables the runtime.
    pub fn offer_module(mut self, module: impl Module + 'static) -> Self {
        let name = crate::descriptor::naming::to_snake_identifier(module.name());
        let source = crate::server::module_runtime::ModuleSource::Compiled(Arc::new(module));
        self.module_runtime
            .get_or_insert_with(RuntimeSetup::default)
            .offers
            .push((name, source));
        self
    }

    /// Set the link service (required)
    pub fn with_link_service(mut self, service: impl LinkService + 'static) -> Self {
        self.link_service = Some(Arc::new(service));
        self
    }

    /// Attach an obrain graph handle — publishes the self-describing `/graph` surface.
    ///
    /// The handle is independent of the link service: an app can serve entities
    /// from any backend and still expose the relief of an obrain store.
    #[cfg(feature = "obrain")]
    pub fn with_graph_store(mut self, store: Arc<crate::storage::ObrainStore>) -> Self {
        self.graph_store = Some(store);
        self
    }

    /// **Branche un store obrain — le chemin clé en main.**
    ///
    /// Un seul appel suffit à obtenir : le relief (`GET /graph`), l'identité
    /// canonique (convention `_uuid`/`_data` **puis** propriétés plates), le
    /// plan IAM matérialisé, l'application des politiques `auth:` (armée par
    /// défaut) et, si le store est inscriptible, le **bail d'écriture**.
    ///
    /// ```rust,ignore
    /// let store = ObrainStore::open("./data/app.obrain")?;
    /// let host = ServerBuilder::new()
    ///     .with_link_service(ObrainLinkService::new(store.clone()))
    ///     .with_obrain_store(store)   // ← tout le reste suit
    ///     .build_host()?;
    /// ```
    ///
    /// C'est un alias sémantique de [`ServerBuilder::with_graph_store`] : il ne
    /// fait rien de plus, mais il dit l'intention. Si le bail ne peut pas être
    /// pris (un autre écrivain le détient), la construction **échoue** : voir
    /// [`ServerBuilder::without_write_fencing`] pour l'assumer explicitement.
    #[cfg(feature = "obrain")]
    pub fn with_obrain_store(self, store: Arc<crate::storage::ObrainStore>) -> Self {
        self.with_graph_store(store)
    }

    /// **N'arme pas** le bail d'écriture sur le store obrain.
    ///
    /// Par défaut, attacher un store inscriptible prend un bail exclusif : un
    /// second écrivain (dans un autre processus **ou dans celui-ci**) est
    /// refusé, au lieu de corrompre le WAL en silence. Désarmer cet appel rend
    /// l'arbitrage à l'exploitant — à ne faire que sciemment.
    ///
    /// Un store ouvert en **lecture seule** n'arme jamais de bail : il n'écrit
    /// pas, donc il n'a rien à arbitrer.
    #[cfg(feature = "obrain")]
    pub fn without_write_fencing(mut self) -> Self {
        self.fence_writes = false;
        tracing::warn!(
            "write fencing DISARMED — a second writer on this store will not be refused"
        );
        self
    }

    /// **Arme l'application des politiques d'autorisation** — c'est le défaut.
    ///
    /// Chaque requête vers une route d'entité connue est confrontée à la
    /// politique déclarée de l'opération, et refusée en **403** si elle ne la
    /// satisfait pas. Deux garanties : **fail-closed** (une politique illisible
    /// refuse, jamais ne laisse passer) et **jamais d'invention** (une route
    /// inconnue est *abstenue*, pas dotée d'une politique imaginaire).
    ///
    /// ⚠️ La politique par défaut d'une entité est `"authenticated"` : une
    /// entité sans configuration explicite **exige un contexte authentifié**.
    /// C'est voulu — un framework qui laisse passer par défaut n'est pas sûr
    /// par défaut.
    pub fn with_authorization(mut self) -> Self {
        self.authorize = true;
        self
    }

    /// **Déclare le repli multi-sites** : plusieurs domaines, plusieurs contenus.
    ///
    /// Monté **avant** les couches d'exposition, donc **couvert** par elles
    /// (CORS, garde de domaine, authentification, politiques) — c'est tout
    /// l'intérêt de passer par ici plutôt que de composer `attach` soi-même.
    ///
    /// ```rust,ignore
    /// let sites = FrontendSites::new()
    ///     .site("www.exemple.fr", Frontend::disk("dist/www"))
    ///     .site("explorer.exemple.fr", Frontend::disk("web/explorer"))
    ///     .trust_forwarded_host(true);
    /// ServerBuilder::new().with_frontend_sites(sites)
    /// ```
    #[cfg(feature = "frontend")]
    pub fn with_frontend_sites(mut self, sites: crate::server::frontend::FrontendSites) -> Self {
        self.frontend_sites = Some(std::sync::Arc::new(sites));
        self
    }

    /// **Charge la configuration d'exposition** (`config/server.yaml`) : CORS,
    /// domaines servis, délégation de proxy.
    ///
    /// ```yaml
    /// cors:
    ///   enabled: true
    ///   origins: ["https://app.exemple.fr"]
    ///   credentials: true
    /// domains:
    ///   allowed: ["app.exemple.fr"]           # tout autre Host est refusé
    ///   trust_forwarded_host: true            # derrière Caddy
    ///   tenants: { "app.exemple.fr": "<uuid>" }
    /// proxy:
    ///   caddy: { enabled: true, upstream: "127.0.0.1:3000" }
    /// ```
    pub fn with_server_config_file(mut self, path: &str) -> Result<Self> {
        self.security = crate::config::ServerSecurityConfig::from_file(path)?;
        Ok(self)
    }

    /// **Fournit la configuration d'exposition** par programme.
    pub fn with_server_security(mut self, security: crate::config::ServerSecurityConfig) -> Self {
        self.security = security;
        self
    }

    /// **CORS** : qui a le droit d'appeler depuis un navigateur.
    ///
    /// Sans cet appel (ou sans `cors.enabled` dans le fichier), **aucune** couche
    /// CORS n'est posée : le framework ne décide pas à la place de l'application.
    pub fn with_cors(mut self, cors: crate::config::CorsConfig) -> Self {
        self.security.cors = cors;
        self
    }

    /// **Domaines servis** : tout autre `Host` est refusé, et un domaine peut
    /// désigner le locataire.
    pub fn with_domains(mut self, domains: crate::config::DomainConfig) -> Self {
        self.security.domains = domains;
        self
    }

    /// **Délègue la configuration du proxy à Caddy** : le framework en rend un
    /// `Caddyfile` (`ServerHost::caddyfile`), l'exploitant l'applique.
    pub fn delegate_to_caddy(mut self, caddy: crate::config::CaddyConfig) -> Self {
        self.security.proxy.caddy = caddy;
        self
    }

    /// **Mode observation** : l'application des politiques décide, journalise et
    /// le publie dans la réponse, mais **ne bloque rien**.
    ///
    /// # Pourquoi c'est le bon premier pas sur un projet existant
    ///
    /// Passer de « aucune politique appliquée » à « tout est appliqué » casse une
    /// production : les politiques déclarées n'ont jamais été vérifiées contre le
    /// trafic réel. Ici, on les observe :
    ///
    /// * chaque refus calculé part dans les journaux (`autorisation : refus OBSERVÉ`) ;
    /// * la réponse porte `x-this-rs-authz: observed-deny`, plus la politique et la
    ///   portée en clair (`x-this-rs-authz-policy`, `x-this-rs-authz-scope`) — un
    ///   reverse-proxy peut donc les compter ;
    /// * quand les journaux sont propres, on arme avec `with_authorization()`.
    pub fn authorization_report_only(mut self) -> Self {
        self.authorize = true;
        self.authorize_report_only = true;
        tracing::warn!(
            "authorization in REPORT-ONLY mode — denials are computed, logged and \
             published, but requests are NOT blocked"
        );
        self
    }

    /// **Fournit le mappage d'identité** de l'application, pour les politiques
    /// `perm:` (IAM matérialisé).
    ///
    /// Le mappage par défaut est le modèle de this-rs (`User`/`Owner` →
    /// `(user_id, tenant_id)`, `Service` → `(service_name, tenant_id)`, `Admin` →
    /// `(admin_id, "global")`). Un projet plus ancien nomme ses identités
    /// autrement : il fournit sa fonction, et l'IAM natif s'applique à **ses**
    /// principaux et à **ses** portées — sans réécrire son authentification.
    ///
    /// ```rust,ignore
    /// let host = ServerBuilder::new()
    ///     .with_obrain_store(store)
    ///     .with_principal_mapping(|ctx| match ctx {
    ///         AuthContext::User { user_id, tenant_id, .. } =>
    ///             Some((format!("account:{user_id}"), format!("org:{tenant_id}"))),
    ///         _ => None,
    ///     })
    ///     .build_host()?;
    /// ```
    pub fn with_principal_mapping<F>(mut self, mapping: F) -> Self
    where
        F: Fn(&crate::core::auth::AuthContext) -> Option<(String, String)> + Send + Sync + 'static,
    {
        self.principal_mapping = Some(Arc::new(mapping));
        self
    }

    /// **Désarme** l'application des politiques — dégradation délibérée.
    ///
    /// À n'utiliser que pour un service réellement public, ou pour reproduire
    /// l'ancien comportement d'accès libre. Une fois désarmées, les sections
    /// `auth:` sont **décoratives** : lues, jamais appliquées. L'appel est
    /// journalisé en avertissement pour que la décision reste visible en
    /// exploitation.
    pub fn without_authorization(mut self) -> Self {
        self.authorize = false;
        tracing::warn!("authorization DISARMED — declared `auth:` policies will NOT be enforced");
        self
    }

    /// Set how entities are identified, quel que soit le moteur qui les porte.
    ///
    /// Sans appel explicite, un hôte auquel un store obrain est attaché
    /// auto-câble [`IdentityRegistry::obrain_default`](crate::core::identity::IdentityRegistry::obrain_default) — convention `_uuid`/`_data`
    /// **puis** propriétés plates. C'est ce qui permet de servir, par la même
    /// route, une base écrite par this-rs et une base écrite par un autre outil.
    pub fn with_identity_registry(
        mut self,
        registry: crate::core::identity::IdentityRegistry,
    ) -> Self {
        self.identity = Some(registry);
        self
    }

    /// Add custom routes to the server
    ///
    /// Use this to add routes that don't fit the CRUD pattern, such as:
    /// - Authentication endpoints (/login, /logout)
    /// - OAuth flows (/oauth/token, /oauth/callback)
    /// - Webhooks (/webhooks/stripe)
    /// - Custom business logic endpoints
    ///
    /// # Example
    ///
    /// ```ignore
    /// use axum::{Router, routing::{post, get}, Json};
    /// use serde_json::json;
    ///
    /// let auth_routes = Router::new()
    ///     .route("/login", post(login_handler))
    ///     .route("/logout", post(logout_handler))
    ///     .route("/oauth/token", post(oauth_token_handler));
    ///
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .with_custom_routes(auth_routes)
    ///     .register_module(module)?
    ///     .build()?;
    /// ```
    pub fn with_custom_routes(mut self, routes: Router) -> Self {
        self.custom_routes.push(routes);
        self
    }

    /// Enable the event bus for real-time notifications
    ///
    /// When enabled, REST/GraphQL handlers will publish events for mutations,
    /// and real-time exposures (WebSocket, SSE) can subscribe to receive them.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Buffer size for the broadcast channel (recommended: 1024)
    ///
    /// # Example
    ///
    /// ```ignore
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .with_event_bus(1024)
    ///     .register_module(module)?
    ///     .build_host()?;
    /// ```
    pub fn with_event_bus(mut self, capacity: usize) -> Self {
        self.event_bus = Some(EventBus::new(capacity));
        self
    }

    /// Enable the event bus with a default capacity of 1024
    ///
    /// Convenience method equivalent to `.with_event_bus(1024)`.
    pub fn with_default_event_bus(mut self) -> Self {
        self.event_bus = Some(EventBus::new(1024));
        self
    }

    /// Provide a pre-built sink registry (overrides auto-wiring from config)
    ///
    /// Use this when you need full control over which sinks are registered.
    /// If not provided and `sinks` is present in config, sinks are auto-wired.
    pub fn with_sink_registry(mut self, registry: SinkRegistry) -> Self {
        self.sink_registry = Some(registry);
        self
    }

    /// Provide a pre-built notification store
    ///
    /// If not provided, one is auto-created when InApp sinks are configured.
    pub fn with_notification_store(mut self, store: Arc<NotificationStore>) -> Self {
        self.notification_store = Some(store);
        self
    }

    /// Create and attach a default notification store
    ///
    /// Convenience method equivalent to `.with_notification_store(Arc::new(NotificationStore::new()))`.
    /// Attache le store d'état d'interface (routes `/ui/state/{portee}`).
    ///
    /// `Arc` partagé : le même store peut être lu par l'hôte et écrit par
    /// l'appelant, sans recopie ni seconde autorité.
    pub fn with_ui_state(mut self, store: Arc<crate::ui::state::UiStateStore>) -> Self {
        self.ui_state = Some(store);
        self
    }

    pub fn with_default_notification_store(mut self) -> Self {
        self.notification_store = Some(Arc::new(NotificationStore::new()));
        self
    }

    /// Provide a pre-built device token store
    ///
    /// If not provided, one is auto-created when sinks are configured.
    pub fn with_device_token_store(mut self, store: Arc<DeviceTokenStore>) -> Self {
        self.device_token_store = Some(store);
        self
    }

    /// Provide a pre-built notification preferences store
    ///
    /// If not provided, one is auto-created when sinks are configured.
    pub fn with_preferences_store(mut self, store: Arc<NotificationPreferencesStore>) -> Self {
        self.preferences_store = Some(store);
        self
    }

    /// Load auth configuration from a YAML file
    ///
    /// This merges the auth config into the server's LinksConfig during build.
    /// Keeps auth config separate from links.yaml for cleaner project structure.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .with_auth_config_file(concat!(env!("CARGO_MANIFEST_DIR"), "/config/auth.yaml"))?
    ///     .register_module(module)?
    ///     .build_host()?;
    /// ```
    pub fn with_auth_config_file(mut self, path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read auth config from '{}': {}", path, e))?;
        let config: crate::config::auth::AuthConfig = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse auth config from '{}': {}", path, e))?;
        self.auth_config = Some(config);
        Ok(self)
    }

    /// Set auth configuration directly
    ///
    /// Use this when you have an `AuthConfig` already constructed in code.
    pub fn with_auth_config(mut self, config: crate::config::auth::AuthConfig) -> Self {
        self.auth_config = Some(config);
        self
    }

    /// Register a custom auth resolver from a struct implementing `AuthResolver`
    ///
    /// The resolver can be referenced from YAML config with the `resolver:` prefix.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use this_rs::core::auth::{AuthResolver, AuthContext};
    ///
    /// struct IsGroupMember { db: Arc<DbPool> }
    ///
    /// impl AuthResolver for IsGroupMember {
    ///     fn check(&self, ctx: &AuthContext, resource_id: Option<&Uuid>) -> bool {
    ///         // ... check group membership
    ///         true
    ///     }
    /// }
    ///
    /// ServerBuilder::new()
    ///     .with_auth_resolver_impl("is_group_member", IsGroupMember { db })
    ///     .build_host()?;
    /// ```
    ///
    /// Then in YAML:
    /// ```yaml
    /// auth:
    ///   create: "resolver:is_group_member"
    /// ```
    pub fn with_auth_resolver_impl(
        mut self,
        name: impl Into<String>,
        resolver: impl AuthResolver + 'static,
    ) -> Self {
        self.resolver_registry.register(name, Arc::new(resolver));
        self
    }

    /// Register a custom auth resolver from a closure
    ///
    /// Convenience method for simple resolvers that don't need a dedicated struct.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// ServerBuilder::new()
    ///     .with_auth_resolver("is_manager", |ctx: &AuthContext, _res: Option<&Uuid>| {
    ///         ctx.has_role("manager")
    ///     })
    ///     .with_auth_resolver("is_delegated", |ctx: &AuthContext, resource_id: Option<&Uuid>| {
    ///         // Check JWT claims for delegation
    ///         let Some(user_id) = ctx.user_id() else { return false };
    ///         let Some(_res_id) = resource_id else { return false };
    ///         // Check delegation...
    ///         true
    ///     })
    ///     .build_host()?;
    /// ```
    ///
    /// Then in YAML:
    /// ```yaml
    /// auth:
    ///   update: "resolver:is_manager"
    ///   delete: "owner_or_resolver:is_delegated"
    /// ```
    pub fn with_auth_resolver<F>(mut self, name: impl Into<String>, func: F) -> Self
    where
        F: Fn(&crate::core::auth::AuthContext, Option<&uuid::Uuid>) -> bool + Send + Sync + 'static,
    {
        let name_str: String = name.into();
        let resolver = FnResolver::new(func, format!("Custom resolver: {}", name_str));
        self.resolver_registry
            .register(name_str, Arc::new(resolver));
        self
    }

    /// Register a module
    ///
    /// This will:
    /// 1. Load the module's configuration
    /// 2. Register all entities from the module
    /// 3. Store the module for entity fetching
    pub fn register_module(mut self, module: impl Module + 'static) -> Result<Self> {
        let module = Arc::new(module);

        // Load the module's configuration
        let config = module.links_config()?;
        self.configs.push(config);

        // Entities are registered in `build_host`, AFTER dependency resolution, in
        // topological order: the result no longer depends on registration order, and a
        // module rejected by resolution never pollutes the `EntityRegistry`.
        self.modules.push(module);

        Ok(self)
    }

    /// Descriptors of the registered modules, checked, resolved, and the module order in
    /// which entities must be registered (legacy modules keep their position; modules with
    /// a descriptor are placed in topological order in their slots).
    fn resolve_descriptors(&self) -> Result<(crate::descriptor::DescriptorSet, Vec<usize>)> {
        use crate::descriptor::framework::codes;
        use crate::descriptor::{
            CheckProfile, ContractError, CustomRegistry, DescriptorSet, naming,
        };

        let mut described: Vec<(usize, crate::descriptor::ModuleDescriptor)> = Vec::new();
        for (i, module) in self.modules.iter().enumerate() {
            let Some(mut d) = module.descriptor() else {
                continue;
            };
            d.normalize();
            d.check(&CustomRegistry::default(), CheckProfile::Compiled)
                .map_err(|issues| {
                    anyhow::Error::new(crate::descriptor::check::issues_to_error(&issues))
                })?;
            // Identity declared once: the trait methods must agree with the descriptor.
            let mut mismatches = Vec::new();
            if naming::to_snake_identifier(module.name()) != d.name {
                mismatches.push(format!("name `{}` vs `{}`", module.name(), d.name));
            }
            if module.version() != d.version {
                mismatches.push(format!("version `{}` vs `{}`", module.version(), d.version));
            }
            let types: std::collections::BTreeSet<&str> =
                module.entity_types().into_iter().collect();
            for e in &d.entities {
                if !types.contains(e.name.as_str()) {
                    mismatches.push(format!("entity `{}` missing from entity_types()", e.name));
                }
            }
            if !mismatches.is_empty() {
                return Err(anyhow::Error::new(
                    ContractError::framework(codes::MODULE_IDENTITY_MISMATCH)
                        .with_param("module", &d.name)
                        .with_param("what", mismatches.join("; ")),
                ));
            }
            described.push((i, d));
        }

        let mut order: Vec<usize> = (0..self.modules.len()).collect();
        let mut set = DescriptorSet::new();
        if described.is_empty() {
            return Ok((set, order));
        }
        let refs: Vec<&crate::descriptor::ModuleDescriptor> =
            described.iter().map(|(_, d)| d).collect();
        let resolution = crate::descriptor::resolve::resolve(&refs, &[]).map_err(|errs| {
            let text = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::Error::new(errs.into_iter().next().expect("non-empty")).context(text)
        })?;
        let slots: Vec<usize> = described.iter().map(|(i, _)| *i).collect();
        for (slot, name) in slots.iter().zip(resolution.order.iter()) {
            let (idx, _) = described
                .iter()
                .find(|(_, d)| &d.name == name)
                .expect("resolved name");
            order[*slot] = *idx;
        }
        for name in &resolution.order {
            let (_, d) = described
                .iter()
                .find(|(_, d)| &d.name == name)
                .expect("resolved name");
            set.insert(d.clone()).map_err(anyhow::Error::new)?;
        }
        Ok((set, order))
    }

    /// Checks on the `links.yaml` of DESCRIBED modules (legacy modules keep the
    /// permissive behaviour):
    /// - every link end is an entity of the module itself or of a module it requires
    ///   (transitively): `this.module.reference_unknown` otherwise (the same rule
    ///   `resolve` applies to the descriptor's own links);
    /// - their configurations merge without conflict (`LinksConfig::merge_strict`):
    ///   `this.module.config_conflict` instead of last-wins.
    fn check_described_configs(
        &self,
        set: &crate::descriptor::DescriptorSet,
        described_idx: &std::collections::BTreeSet<usize>,
    ) -> Result<()> {
        use crate::descriptor::ContractError;
        use crate::descriptor::framework::codes;
        for &i in described_idx {
            let module = &self.modules[i];
            let name = crate::descriptor::naming::to_snake_identifier(module.name());
            let mut visible: std::collections::BTreeSet<String> = module
                .entity_types()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let mut stack = vec![name.clone()];
            let mut seen = std::collections::BTreeSet::new();
            while let Some(m) = stack.pop() {
                if !seen.insert(m.clone()) {
                    continue;
                }
                if let Some(d) = set.module(&m) {
                    visible.extend(d.entities.iter().map(|e| e.name.clone()));
                    stack.extend(d.requires.iter().map(|r| r.name.clone()));
                }
            }
            for l in &self.configs[i].links {
                for end in [&l.source_type, &l.target_type] {
                    if !visible.contains(end) {
                        return Err(anyhow::Error::new(
                            ContractError::framework(codes::MODULE_REFERENCE_UNKNOWN)
                                .with_param("module", &name)
                                .with_param("entity", end),
                        ));
                    }
                }
            }
        }
        let configs: Vec<LinksConfig> = described_idx
            .iter()
            .map(|&i| self.configs[i].clone())
            .collect();
        if configs.len() > 1 {
            LinksConfig::merge_strict(configs).map_err(|errs| {
                let text = errs
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                anyhow::Error::new(errs.into_iter().next().expect("non-empty")).context(text)
            })?;
        }
        Ok(())
    }

    /// Every entity with a descriptor has ONE plural: `EntityDesc.plural` ==
    /// `EntityDescriptor::plural` == `EntityConfig::plural` (when each exists).
    fn check_plurals(
        set: &crate::descriptor::DescriptorSet,
        registry: &EntityRegistry,
        config: &LinksConfig,
    ) -> Result<()> {
        use crate::descriptor::ContractError;
        use crate::descriptor::framework::codes;
        let registered: HashMap<String, String> = registry.entity_plurals().into_iter().collect();
        for m in set.modules() {
            for e in &m.entities {
                let mut sources = vec![format!("descriptor={}", e.plural)];
                let mut diverge = false;
                if let Some(p) = registered.get(&e.name) {
                    diverge |= p != &e.plural;
                    sources.push(format!("EntityDescriptor={p}"));
                }
                if let Some(c) = config.entities.iter().find(|c| c.singular == e.name) {
                    diverge |= c.plural != e.plural;
                    sources.push(format!("links.yaml={}", c.plural));
                }
                if diverge {
                    return Err(anyhow::Error::new(
                        ContractError::framework(codes::MODULE_PLURAL_MISMATCH)
                            .with_param("entity", &e.name)
                            .with_param("sources", sources.join(", ")),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Build the transport-agnostic host
    ///
    /// This generates a `ServerHost` that can be used with any exposure type
    /// (REST, GraphQL, gRPC, etc.).
    ///
    /// # Returns
    ///
    /// Returns a `ServerHost` containing all framework state.
    pub fn build_host(mut self) -> Result<ServerHost> {
        // Merge all configs
        let mut merged_config = self.merge_configs()?;

        // Inject auth config from with_auth_config_file() if set
        if let Some(auth_config) = self.auth_config.take() {
            merged_config.auth = Some(auth_config);
        }

        // Le service de liens : celui fourni, sinon — quand un store obrain est
        // branché — le service obrain. **C'est ce qui rend le branchement d'un
        // store réellement « clé en main »** : sans ce repli, attacher obrain
        // échouait sur « LinkService is required », une exigence qui n'a rien à
        // voir avec ce que l'appelant demandait.
        #[cfg(feature = "obrain")]
        let link_service = match self.link_service.take() {
            Some(service) => service,
            None => match self.graph_store.as_ref() {
                Some(store) => {
                    tracing::info!(
                        "aucun LinkService fourni : service de liens obrain attaché par défaut \
                         (le store est déjà là)"
                    );
                    Arc::new(crate::storage::ObrainLinkService::new(Arc::clone(store)))
                        as Arc<dyn LinkService>
                }
                None => {
                    return Err(anyhow::anyhow!(
                        "LinkService is required. Call .with_link_service() — ou branchez un \
                         store obrain avec .with_obrain_store()"
                    ));
                }
            },
        };

        #[cfg(not(feature = "obrain"))]
        let link_service = self
            .link_service
            .take()
            .ok_or_else(|| anyhow::anyhow!("LinkService is required. Call .with_link_service()"))?;

        // Descriptors: checked, identity-checked, resolved (topological, semver, cycles,
        // duplicates). Without any descriptor this is a no-op (historical behaviour).
        let (descriptor_set, order) = self.resolve_descriptors()?;

        let described_idx: std::collections::BTreeSet<usize> = (0..self.modules.len())
            .filter(|&i| {
                descriptor_set
                    .module(&crate::descriptor::naming::to_snake_identifier(
                        self.modules[i].name(),
                    ))
                    .is_some()
            })
            .collect();
        self.check_described_configs(&descriptor_set, &described_idx)?;

        // Entities registered in resolved order (see `register_module`), one module at a
        // time: an entity of a DESCRIBED module (or colliding with one) is registered with
        // `try_register` — a duplicate entity or plural is an error, not a silent
        // last-wins. Between legacy modules the historical last-wins is kept (warned).
        let mut described_types: std::collections::BTreeSet<String> = Default::default();
        for &i in &order {
            let mut local = EntityRegistry::new();
            self.modules[i].register_entities(&mut local);
            let is_described = described_idx.contains(&i);
            for d in local.drain() {
                let t = d.entity_type().to_string();
                let collides_with_described = described_types.contains(&t)
                    || self
                        .entity_registry
                        .entity_type_for_plural(d.plural())
                        .is_some_and(|owner| described_types.contains(owner));
                if is_described || collides_with_described {
                    self.entity_registry.try_register(d).map_err(|e| {
                        anyhow::Error::new(e.with_param("module", self.modules[i].name()))
                    })?;
                    if is_described {
                        described_types.insert(t);
                    }
                } else {
                    if self.entity_registry.contains(&t) {
                        tracing::warn!(
                            entity_type = %t,
                            module = self.modules[i].name(),
                            "entity registered by several modules: the last one wins (legacy \
                             behaviour; declare a descriptor to make it an error)"
                        );
                    }
                    self.entity_registry.register(d);
                }
            }
        }
        Self::check_plurals(&descriptor_set, &self.entity_registry, &merged_config)?;

        // Build entity fetchers / creators maps. A collision involving a module WITH a
        // descriptor is an error (fail-closed); between legacy modules the historical
        // last-wins is kept, with a warning.
        let described: std::collections::BTreeSet<String> = descriptor_set
            .modules()
            .flat_map(|m| m.entities.iter().map(|e| e.name.clone()))
            .collect();
        let mut fetchers_map: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        let mut creators_map: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        for &i in &order {
            let module = &self.modules[i];
            for entity_type in module.entity_types() {
                if let Some(fetcher) = module.get_entity_fetcher(entity_type)
                    && fetchers_map
                        .insert(entity_type.to_string(), fetcher)
                        .is_some()
                {
                    duplicate_provider(&described, entity_type, module.name())?;
                }
                if let Some(creator) = module.get_entity_creator(entity_type)
                    && creators_map
                        .insert(entity_type.to_string(), creator)
                        .is_some()
                {
                    duplicate_provider(&described, entity_type, module.name())?;
                }
            }
        }

        // Build the host
        let mut host = ServerHost::from_builder_components(
            link_service,
            merged_config,
            self.entity_registry,
            fetchers_map,
            creators_map,
        )?;
        if !descriptor_set.is_empty() {
            host = host.with_descriptors(descriptor_set);
        }

        // Attach event bus if configured
        if let Some(event_bus) = self.event_bus.take() {
            host = host.with_event_bus(event_bus);
        }

        // Store d'état d'interface : attaché à l'hôte, exactement comme le bus
        // d'événements. Sa présence est ce qui décide du montage de
        // `/ui/state/{portee}` — absent, la route n'existe pas.
        if let Some(ui) = self.ui_state.take() {
            host.ui_state = Some(ui);
        }

        // Attach resolver registry if any resolvers were registered
        if !self.resolver_registry.is_empty() {
            let names = self.resolver_registry.names();
            tracing::info!(
                resolvers = ?names,
                "Auth resolver registry attached ({} resolvers)",
                names.len()
            );
            host = host.with_resolver_registry(self.resolver_registry);
        }

        // Transférer l'armement de l'autorisation (faux par défaut).
        // Transfert dans les DEUX sens : sans le `else`, un
        // `without_authorization()` était silencieusement annulé par le défaut
        // `true` posé à la construction de l'hôte — un service qui se croyait
        // ouvert refusait ses clients.
        host.authorize = self.authorize;
        host.authorize_report_only = self.authorize_report_only;
        // **Validée au démarrage** : une origine CORS impossible ou un locataire
        // illisible doit faire échouer la construction, pas le premier appel.
        self.security.cors.validate()?;
        self.security.domains.validate()?;
        host.security = Arc::new(self.security.clone());
        #[cfg(feature = "frontend")]
        {
            host.frontend_sites = self.frontend_sites.clone();
        }
        host.principal_mapping = self.principal_mapping.clone();

        // Attach the obrain graph handle if one was provided.
        //
        // Volontairement sans auto-câblage depuis un chemin de config : ouvrir
        // un store obrain est une décision de l'application (mode, WAL,
        // lecture seule), pas un effet de bord du chargement de `this.yaml`.
        #[cfg(feature = "obrain")]
        if let Some(store) = self.graph_store.take() {
            let substrate = store.is_substrate();
            let read_only = store.is_read_only();

            // Le bail, armé par défaut dès que le store est inscriptible.
            //
            // **Fail-closed** : si le bail ne peut pas être pris, on refuse de
            // construire. Continuer sans bail serait exactement le trou qu'il
            // comble — un second écrivain corromprait le WAL en silence.
            if self.fence_writes && !read_only {
                let shard = store.shard_id()?;
                match &shard.store {
                    crate::storage::writer_lease::StoreId::Dir(path) => {
                        let authority =
                            crate::storage::writer_lease::FileAuthority::new(path.clone());
                        match store.enable_write_fencing_with(&authority) {
                            Ok(epoch) => tracing::info!(
                                epoch,
                                shard = %shard.key(),
                                "write fencing armed by default (second writer will be refused)"
                            ),
                            Err(e) => {
                                return Err(anyhow::anyhow!(
                                    "write fencing refused on {}: {e}. Un autre écrivain détient                                      le bail (autre processus, ou ce processus). Utiliser                                      `.without_write_fencing()` pour l'assumer explicitement.",
                                    shard.key()
                                ));
                            }
                        }
                    }
                    crate::storage::writer_lease::StoreId::Volatile(_) => {
                        tracing::debug!("store volatil : pas de bail de fichier possible");
                    }
                }
            } else if read_only {
                tracing::info!("store ouvert en lecture seule : aucun bail armé (rien à arbitrer)");
            }

            tracing::info!(
                substrate,
                read_only,
                "obrain attached — relief, identity, IAM and authorization follow                  (GET /graph, GET /graph/identity)"
            );
            host = host.with_graph_store(store);
        }

        // Identité : un registre explicite prime ; sinon, dès qu'un store obrain
        // est attaché, on auto-câble le registre obrain (convention PUIS
        // propriétés plates). Sans store, on ne présume rien : la convention
        // seule, c'est-à-dire le comportement historique.
        if let Some(registry) = self.identity.take() {
            host = host.with_identity_registry(registry);
            tracing::info!("identity registry attached explicitly");
        } else {
            #[cfg(feature = "obrain")]
            if host.graph_store().is_some() {
                host = host.with_identity_registry(
                    crate::core::identity::IdentityRegistry::obrain_default(),
                );
                tracing::info!(
                    "identity registry auto-wired (obrain_default: _uuid/_data then flat properties)"
                );
            }
        }

        // Auto-wire auth provider from config
        // Clone the auth config to avoid borrow conflicts with `host`
        let auth_config_owned = host.config.auth.clone();
        if let Some(ref auth_config) = auth_config_owned {
            use crate::config::auth::AuthProviderType;
            match auth_config.provider {
                AuthProviderType::None => {
                    // No auth provider — use default NoAuthProvider behavior
                }
                AuthProviderType::Wami => {
                    #[cfg(feature = "wami")]
                    {
                        use crate::config::auth::AuthMode;
                        use crate::core::auth::wami_provider::WamiAuthProvider;
                        match WamiAuthProvider::from_config(auth_config) {
                            Ok(provider) => {
                                host = host.with_auth_provider(Arc::new(provider));
                                tracing::info!("WAMI auth provider auto-wired");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to create WamiAuthProvider: {}. Continuing without auth.",
                                    e
                                );
                            }
                        }

                        // Auto-wire STS for embedded/bootstrap modes
                        if let Some(ref wami_config) = auth_config.wami {
                            let needs_sts = matches!(
                                wami_config.mode,
                                AuthMode::Embedded | AuthMode::Bootstrap
                            );
                            if needs_sts {
                                match Self::build_sts_state(wami_config) {
                                    Ok(sts_state) => {
                                        host = host.with_sts_state(sts_state);
                                        tracing::info!(
                                            "STS service auto-wired (mode: {:?})",
                                            wami_config.mode
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to create STS service: {}. Auth endpoints won't be available.",
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    }
                    #[cfg(not(feature = "wami"))]
                    {
                        tracing::warn!(
                            "Auth provider set to 'wami' but feature 'wami' is not enabled. Ignoring."
                        );
                    }
                }
                AuthProviderType::Oidc => {
                    tracing::warn!("OIDC auth provider not yet implemented. Ignoring.");
                }
            }
        }

        // Auto-wire event pipeline from config (sinks section)
        let has_sinks = host.config.sinks.as_ref().is_some_and(|s| !s.is_empty());

        if has_sinks || self.sink_registry.is_some() {
            // Build or use provided stores
            let notification_store = self
                .notification_store
                .take()
                .unwrap_or_else(|| Arc::new(NotificationStore::new()));
            let preferences_store = self
                .preferences_store
                .take()
                .unwrap_or_else(|| Arc::new(NotificationPreferencesStore::new()));
            let device_token_store = self
                .device_token_store
                .take()
                .unwrap_or_else(|| Arc::new(DeviceTokenStore::new()));

            // Build or use provided sink registry
            let sink_registry = if let Some(registry) = self.sink_registry.take() {
                registry
            } else if let Some(ref sink_configs) = host.config.sinks {
                let factory = SinkFactory::with_stores(
                    notification_store.clone(),
                    preferences_store.clone(),
                    device_token_store.clone(),
                );
                factory.build_registry(sink_configs)
            } else {
                SinkRegistry::new()
            };

            // Attach everything to the host
            host = host
                .with_notification_store(notification_store)
                .with_preferences_store(preferences_store)
                .with_device_token_store(device_token_store)
                .with_sink_registry(sink_registry);

            tracing::info!("event pipeline auto-wired from config");
        }

        // Auto-wire event log if events section is present
        if host.config.events.is_some() {
            let event_log = Arc::new(crate::events::InMemoryEventLog::new());
            host = host.with_event_log(event_log);
            tracing::info!("event log auto-wired (InMemoryEventLog)");
        }

        // Runtime module loading, created LAST: it captures the final catalog (base of
        // every composition), the event bus, the resolvers and the authorization flags.
        if let Some(setup) = self.module_runtime.take() {
            use crate::server::module_runtime::{DefaultModuleLoader, HostParts, ModuleRuntime};
            let loader = setup.loader.unwrap_or_else(|| {
                Arc::new(DefaultModuleLoader::new().with_event_bus(host.event_bus.clone()))
            });
            let runtime = ModuleRuntime::new(
                HostParts {
                    catalog: host.catalog.clone(),
                    link_service: host.link_service.clone(),
                    event_bus: host.event_bus.clone(),
                    resolvers: host.resolver_registry.clone(),
                    authorize: host.authorize,
                    report_only: host.authorize_report_only,
                    validation_registry: host.validation_registry.clone(),
                    locales: host.locales.clone(),
                },
                loader,
                setup.store,
                setup.options,
                setup.offers,
            );
            host.module_runtime = Some(runtime);
            tracing::info!("module runtime enabled (runtime load/unload, admin interface)");
        }

        Ok(host)
    }

    /// Build the final REST router
    ///
    /// This generates:
    /// - CRUD routes for all registered entities
    /// - Link routes (bidirectional)
    /// - Introspection routes
    ///
    /// Note: This is a convenience method that builds the host and immediately
    /// exposes it via REST. For other exposure types, use `build_host_arc()`.
    pub fn build(mut self) -> Result<Router> {
        let custom_routes = std::mem::take(&mut self.custom_routes);
        let host = Arc::new(self.build_host()?);
        RestExposure::build_router(host, custom_routes)
    }

    /// Merge all configurations from registered modules
    fn merge_configs(&self) -> Result<LinksConfig> {
        Ok(LinksConfig::merge(self.configs.clone()))
    }

    /// Build a combined REST + gRPC router
    ///
    /// This is a convenience method that builds both REST and gRPC routers
    /// from the registered modules and merges them safely into a single router.
    ///
    /// Internally, it uses [`GrpcExposure::build_router_no_fallback`](super::GrpcExposure::build_router_no_fallback) for the
    /// gRPC side (no fallback) and [`RestExposure::build_router`] for REST
    /// (with its nested link path fallback), then merges them via
    /// [`combine_rest_and_grpc`](super::router::combine_rest_and_grpc).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let app = ServerBuilder::new()
    ///     .with_link_service(InMemoryLinkService::new())
    ///     .register_module(MyModule)?
    ///     .build_with_grpc()?;
    ///
    /// let listener = TcpListener::bind("127.0.0.1:3000").await?;
    /// axum::serve(listener, app).await?;
    /// ```
    #[cfg(feature = "grpc")]
    pub fn build_with_grpc(mut self) -> Result<Router> {
        use super::exposure::grpc::GrpcExposure;
        use super::router::combine_rest_and_grpc;

        let custom_routes = std::mem::take(&mut self.custom_routes);
        let host = Arc::new(self.build_host()?);

        let rest_router = RestExposure::build_router(host.clone(), custom_routes)?;
        // **Fail-closed** : ce raccourci montait REST (politiques appliquées) et
        // gRPC (aucune politique) dans le MÊME routeur, sans un mot. Un client
        // gRPC atteignait donc les entités protégées sans leur politique. Il
        // refuse maintenant, et l'application qui accepte le risque le dit.
        let grpc_router = GrpcExposure::build_router_no_fallback(host)?;

        Ok(combine_rest_and_grpc(rest_router, grpc_router))
    }

    /// Comme [`ServerBuilder::build_with_grpc`], en **acceptant explicitement**
    /// que gRPC ne soit pas soumis aux politiques `auth:`.
    ///
    /// # Ce que vous acceptez
    ///
    /// Le routeur REST applique vos politiques ; le routeur gRPC, non — ses
    /// opérations ne sont pas résolues par route. Un client gRPC atteint donc
    /// les entités que le REST refuse. À n'utiliser que si vous appliquez le
    /// contrôle ailleurs (authentification réseau, résolveurs) ou si vos entités
    /// sont publiques pour qui atteint le port.
    #[cfg(feature = "grpc")]
    pub fn build_with_grpc_unenforced(mut self) -> Result<Router> {
        use super::exposure::grpc::GrpcExposure;
        use super::router::combine_rest_and_grpc;

        let custom_routes = std::mem::take(&mut self.custom_routes);
        let host = Arc::new(self.build_host()?);

        let rest_router = RestExposure::build_router(host.clone(), custom_routes)?;
        let grpc_router = GrpcExposure::build_router_no_fallback_unenforced(host)?;

        Ok(combine_rest_and_grpc(rest_router, grpc_router))
    }

    /// Serve the application with graceful shutdown
    ///
    /// This will:
    /// - Bind to the provided address
    /// - Start serving requests
    /// - Handle SIGTERM and SIGINT (Ctrl+C) for graceful shutdown
    ///
    /// # Example
    ///
    /// ```ignore
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .register_module(module)?
    ///     .serve("127.0.0.1:3000").await?;
    /// ```
    pub async fn serve(self, addr: &str) -> Result<()> {
        let app = self.build()?;
        let listener = TcpListener::bind(addr).await?;

        tracing::info!("Server listening on {}", addr);

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        tracing::info!("Server shutdown complete");
        Ok(())
    }

    /// Serve the application with REST + gRPC and graceful shutdown
    ///
    /// This is the gRPC equivalent of [`serve`](Self::serve). It builds a combined
    /// REST+gRPC router and serves it on the given address.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ServerBuilder::new()
    ///     .with_link_service(service)
    ///     .register_module(module)?
    ///     .serve_with_grpc("127.0.0.1:3000").await?;
    /// ```
    #[cfg(feature = "grpc")]
    pub async fn serve_with_grpc(self, addr: &str) -> Result<()> {
        let app = self.build_with_grpc()?;
        let listener = TcpListener::bind(addr).await?;

        tracing::info!("Server listening on {} (REST + gRPC)", addr);

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        tracing::info!("Server shutdown complete");
        Ok(())
    }

    /// Build the STS state from WAMI config (embedded/bootstrap modes)
    #[cfg(feature = "wami")]
    fn build_sts_state(
        wami_config: &crate::config::auth::WamiConfig,
    ) -> anyhow::Result<Arc<crate::server::exposure::rest::auth_routes::StsState>> {
        use crate::config::auth::AuthMode;
        use crate::core::auth::sts::StsService;
        use crate::server::exposure::rest::auth_routes::StsState;

        let (private_pem, public_pem) = match wami_config.mode {
            AuthMode::Bootstrap => {
                // Auto-generate a key pair for development
                use ed25519_dalek::SigningKey;
                use ed25519_dalek::pkcs8::EncodePublicKey;
                use ed25519_dalek::pkcs8::{EncodePrivateKey, spki::der::pem::LineEnding};

                // Generate a random 32-byte secret using UUIDs (no rand dependency needed)
                let u1 = uuid::Uuid::new_v4();
                let u2 = uuid::Uuid::new_v4();
                let mut secret = [0u8; 32];
                secret[..16].copy_from_slice(u1.as_bytes());
                secret[16..].copy_from_slice(u2.as_bytes());
                let signing_key = SigningKey::from_bytes(&secret);
                let verifying_key = signing_key.verifying_key();

                let priv_pem = signing_key
                    .to_pkcs8_pem(LineEnding::LF)
                    .map_err(|e| anyhow::anyhow!("Failed to encode bootstrap private key: {}", e))?
                    .to_string();
                let pub_pem = verifying_key
                    .to_public_key_pem(LineEnding::LF)
                    .map_err(|e| anyhow::anyhow!("Failed to encode bootstrap public key: {}", e))?;

                tracing::info!("Bootstrap mode: auto-generated Ed25519 key pair");
                (priv_pem, pub_pem)
            }
            AuthMode::Embedded => {
                let priv_key = wami_config
                    .private_key
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Embedded mode requires 'private_key' in wami config")
                    })?
                    .clone();
                let pub_key = wami_config
                    .public_key
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Embedded mode requires 'public_key' in wami config")
                    })?
                    .clone();
                (priv_key, pub_key)
            }
            AuthMode::Sts => {
                anyhow::bail!("STS mode does not need embedded token issuance");
            }
        };

        let issuer = wami_config
            .issuer
            .clone()
            .unwrap_or_else(|| "this-rs".to_string());

        let mut sts = StsService::new(&private_pem, issuer)?;
        sts.set_access_ttl_secs(wami_config.token_ttl_secs);
        sts.set_refresh_ttl_secs(wami_config.refresh_ttl_secs);

        let decoding_key = jsonwebtoken::DecodingKey::from_ed_pem(public_pem.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to create DecodingKey: {}", e))?;

        Ok(Arc::new(StsState {
            sts: Arc::new(sts),
            public_key_pem: public_pem,
            decoding_key,
        }))
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Wait for shutdown signal (SIGTERM or Ctrl+C)
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C signal, initiating graceful shutdown...");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM signal, initiating graceful shutdown...");
        },
    }
}

/// Two modules provide the same entity type. Error when one of them is described
/// (fail-closed); historical last-wins with a warning between legacy modules.
fn duplicate_provider(
    described: &std::collections::BTreeSet<String>,
    entity_type: &str,
    module: &str,
) -> Result<()> {
    if described.contains(entity_type) {
        return Err(anyhow::Error::new(
            crate::descriptor::ContractError::framework(
                crate::descriptor::framework::codes::MODULE_DUPLICATE_ENTITY,
            )
            .with_param("entity", entity_type)
            .with_param("modules", module),
        ));
    }
    tracing::warn!(
        entity_type,
        module,
        "entity provided by several modules: the last registered wins (legacy behaviour)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::LinkDefinition;
    use crate::core::module::Module;
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use std::sync::Arc;

    // ── Mock Module for testing ──────────────────────────────────────────

    /// A minimal Module implementation for builder tests
    struct StubModule {
        name: &'static str,
        entity_types: Vec<&'static str>,
        config: LinksConfig,
    }

    impl StubModule {
        fn single_entity() -> Self {
            Self {
                name: "stub",
                entity_types: vec!["order"],
                config: LinksConfig {
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
                },
            }
        }

        fn with_link() -> Self {
            Self {
                name: "linked_stub",
                entity_types: vec!["user", "car"],
                config: LinksConfig {
                    entities: vec![
                        EntityConfig {
                            singular: "user".to_string(),
                            plural: "users".to_string(),
                            auth: EntityAuthConfig::default(),
                        },
                        EntityConfig {
                            singular: "car".to_string(),
                            plural: "cars".to_string(),
                            auth: EntityAuthConfig::default(),
                        },
                    ],
                    links: vec![LinkDefinition {
                        link_type: "owner".to_string(),
                        source_type: "user".to_string(),
                        target_type: "car".to_string(),
                        forward_route_name: "cars-owned".to_string(),
                        reverse_route_name: "users-owners".to_string(),
                        description: Some("User owns a car".to_string()),
                        required_fields: None,
                        auth: None,
                    }],
                    validation_rules: None,
                    events: None,
                    sinks: None,
                    auth: None,
                },
            }
        }
    }

    impl Module for StubModule {
        fn name(&self) -> &str {
            self.name
        }

        fn entity_types(&self) -> Vec<&str> {
            self.entity_types.clone()
        }

        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            Ok(self.config.clone())
        }

        fn register_entities(&self, _registry: &mut EntityRegistry) {
            // No entity descriptors in stub
        }

        fn get_entity_fetcher(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityFetcher>> {
            None
        }

        fn get_entity_creator(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityCreator>> {
            None
        }
    }

    /// A module whose links_config() returns an error
    struct FailingModule;

    impl Module for FailingModule {
        fn name(&self) -> &str {
            "failing"
        }

        fn entity_types(&self) -> Vec<&str> {
            vec![]
        }

        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            Err(anyhow::anyhow!("config load failed"))
        }

        fn register_entities(&self, _registry: &mut EntityRegistry) {}

        fn get_entity_fetcher(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityFetcher>> {
            None
        }

        fn get_entity_creator(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityCreator>> {
            None
        }
    }

    // ── Constructor tests ────────────────────────────────────────────────

    #[test]
    fn test_new_creates_empty_builder() {
        let builder = ServerBuilder::new();
        assert!(builder.link_service.is_none());
        assert!(builder.configs.is_empty());
        assert!(builder.modules.is_empty());
        assert!(builder.custom_routes.is_empty());
        assert!(builder.event_bus.is_none());
    }

    #[test]
    fn test_default_is_same_as_new() {
        let builder = ServerBuilder::default();
        assert!(builder.link_service.is_none());
        assert!(builder.configs.is_empty());
        assert!(builder.modules.is_empty());
        assert!(builder.custom_routes.is_empty());
        assert!(builder.event_bus.is_none());
    }

    // ── with_link_service ────────────────────────────────────────────────

    #[test]
    fn test_with_link_service_sets_service() {
        let builder = ServerBuilder::new().with_link_service(InMemoryLinkService::new());
        assert!(builder.link_service.is_some());
    }

    // ── with_event_bus ───────────────────────────────────────────────────

    #[test]
    fn test_with_event_bus_sets_bus() {
        let builder = ServerBuilder::new().with_event_bus(1024);
        assert!(builder.event_bus.is_some());
    }

    // ── with_custom_routes ───────────────────────────────────────────────

    #[test]
    fn test_with_custom_routes_appends_router() {
        let builder = ServerBuilder::new()
            .with_custom_routes(Router::new())
            .with_custom_routes(Router::new());
        assert_eq!(builder.custom_routes.len(), 2);
    }

    // ── register_module ──────────────────────────────────────────────────

    #[test]
    fn test_register_module_stores_config_and_module() {
        let builder = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("register_module should succeed");
        assert_eq!(builder.configs.len(), 1);
        assert_eq!(builder.modules.len(), 1);
    }

    #[test]
    fn test_register_multiple_modules() {
        let builder = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("first module should register")
            .register_module(StubModule::with_link())
            .expect("second module should register");
        assert_eq!(builder.configs.len(), 2);
        assert_eq!(builder.modules.len(), 2);
    }

    #[test]
    fn test_register_module_failing_config_returns_error() {
        let result = ServerBuilder::new().register_module(FailingModule);
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().expect("should be Err"));
        assert!(
            err_msg.contains("config load failed"),
            "error should contain cause: {}",
            err_msg
        );
    }

    // ── build_host ───────────────────────────────────────────────────────

    #[test]
    fn test_build_host_without_link_service_fails() {
        let result = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host();
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().expect("should be Err"));
        assert!(
            err_msg.contains("LinkService is required"),
            "error should mention LinkService: {}",
            err_msg
        );
    }

    #[test]
    fn test_build_host_single_module() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        assert_eq!(host.config.entities.len(), 1);
        assert_eq!(host.config.entities[0].singular, "order");
        assert!(host.event_bus.is_none());
    }

    #[test]
    fn test_build_host_multi_module_merges_configs() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(StubModule::single_entity())
            .expect("register first should succeed")
            .register_module(StubModule::with_link())
            .expect("register second should succeed")
            .build_host()
            .expect("build_host should succeed");

        // Merged config should contain entities from both modules
        let entity_names: Vec<&str> = host
            .config
            .entities
            .iter()
            .map(|e| e.singular.as_str())
            .collect();
        assert!(entity_names.contains(&"order"), "should contain order");
        assert!(entity_names.contains(&"user"), "should contain user");
        assert!(entity_names.contains(&"car"), "should contain car");
    }

    #[test]
    fn test_build_host_with_event_bus_attaches_bus() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_event_bus(16)
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        assert!(host.event_bus().is_some());
    }

    #[test]
    fn test_build_host_no_modules_empty_config() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .build_host()
            .expect("build_host with no modules should succeed");

        assert!(host.config.entities.is_empty());
        assert!(host.config.links.is_empty());
    }

    // ── build (REST router) ──────────────────────────────────────────────

    #[test]
    fn test_build_produces_router() {
        let router = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build()
            .expect("build should produce a Router");

        // We cannot inspect the Router deeply, but it should not panic
        let _ = router;
    }

    #[test]
    fn test_build_without_link_service_fails() {
        let result = ServerBuilder::new()
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_with_custom_routes() {
        use axum::routing::get;

        let custom = Router::new().route("/custom", get(|| async { "ok" }));
        let router = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_custom_routes(custom)
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build()
            .expect("build should succeed with custom routes");

        let _ = router;
    }

    // ── Fluent chaining ──────────────────────────────────────────────────

    #[test]
    fn test_fluent_chaining_full_pipeline() {
        let result = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_event_bus(256)
            .with_custom_routes(Router::new())
            .register_module(StubModule::with_link())
            .expect("register should succeed")
            .build();
        assert!(result.is_ok(), "full fluent pipeline should succeed");
    }

    // ── Auto-wiring tests ──────────────────────────────────────────────

    /// A module whose config includes sinks
    struct StubModuleWithSinks;

    impl Module for StubModuleWithSinks {
        fn name(&self) -> &str {
            "with_sinks"
        }

        fn entity_types(&self) -> Vec<&str> {
            vec!["user"]
        }

        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            use crate::config::events::EventsConfig;
            use crate::config::sinks::{SinkConfig, SinkType};

            Ok(LinksConfig {
                entities: vec![EntityConfig {
                    singular: "user".to_string(),
                    plural: "users".to_string(),
                    auth: EntityAuthConfig::default(),
                }],
                links: vec![],
                validation_rules: None,
                events: Some(EventsConfig::default()),
                sinks: Some(vec![SinkConfig {
                    name: "in-app-notif".to_string(),
                    sink_type: SinkType::InApp,
                    config: Default::default(),
                }]),
                auth: None,
            })
        }

        fn register_entities(&self, _registry: &mut EntityRegistry) {}

        fn get_entity_fetcher(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityFetcher>> {
            None
        }

        fn get_entity_creator(
            &self,
            _entity_type: &str,
        ) -> Option<Arc<dyn crate::core::EntityCreator>> {
            None
        }
    }

    #[test]
    fn test_auto_wire_sinks_from_config() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_event_bus(16)
            .register_module(StubModuleWithSinks)
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        // Auto-wired: InApp sink registered
        assert!(host.sink_registry().is_some());
        let registry = host.sink_registry().unwrap();
        assert!(registry.get("in-app-notif").is_some());
        assert_eq!(registry.len(), 1);

        // Auto-wired: stores created
        assert!(host.notification_store().is_some());
        assert!(host.device_token_store().is_some());
        assert!(host.preferences_store().is_some());

        // Auto-wired: event log created (events section present)
        assert!(host.event_log().is_some());
    }

    #[test]
    fn test_no_auto_wire_without_sinks_config() {
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        // No sinks in config → no auto-wiring
        assert!(host.sink_registry().is_none());
        assert!(host.notification_store().is_none());
        assert!(host.device_token_store().is_none());
        assert!(host.preferences_store().is_none());
        assert!(host.event_log().is_none());
    }

    #[test]
    fn test_manual_sink_registry_overrides_auto_wire() {
        let manual_registry = SinkRegistry::new();

        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_sink_registry(manual_registry)
            .register_module(StubModuleWithSinks)
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        // Manual registry used instead of auto-wired → empty, no InApp
        let registry = host.sink_registry().unwrap();
        assert!(registry.is_empty());
        // InApp from config NOT auto-created because we provided a manual registry
        assert!(registry.get("in-app-notif").is_none());
    }

    #[test]
    fn test_manual_stores_used_in_auto_wire() {
        let custom_store = Arc::new(crate::events::sinks::in_app::NotificationStore::new());
        let store_clone = custom_store.clone();

        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_notification_store(custom_store)
            .register_module(StubModuleWithSinks)
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        // The custom store should be the one on the host
        assert!(Arc::ptr_eq(
            host.notification_store().unwrap(),
            &store_clone
        ));
    }

    #[test]
    fn test_retro_compatible_no_sinks_no_events() {
        // Exact same test as before — nothing changes
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .with_event_bus(16)
            .register_module(StubModule::single_entity())
            .expect("register should succeed")
            .build_host()
            .expect("build_host should succeed");

        assert!(host.event_bus().is_some());
        assert!(host.sink_registry().is_none());
        assert!(host.event_log().is_none());
        assert_eq!(host.config.entities.len(), 1);
    }
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::module::Module;
    use crate::descriptor::{EntityDesc, ModuleDescriptor, ModuleReq};
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;

    /// Module whose descriptor declares one entity and optional requirements.
    struct Described {
        name: &'static str,
        entity: &'static str,
        yaml_plural: String,
        requires: Vec<&'static str>,
    }

    impl Module for Described {
        fn name(&self) -> &str {
            self.name
        }
        fn entity_types(&self) -> Vec<&str> {
            vec![self.entity]
        }
        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            Ok(LinksConfig {
                entities: vec![EntityConfig {
                    singular: self.entity.to_string(),
                    plural: self.yaml_plural.clone(),
                    auth: EntityAuthConfig::default(),
                }],
                links: vec![],
                validation_rules: None,
                events: None,
                sinks: None,
                auth: None,
            })
        }
        fn register_entities(&self, _registry: &mut EntityRegistry) {}
        fn descriptor(&self) -> Option<ModuleDescriptor> {
            let mut d = ModuleDescriptor::new(self.name, "1.0.0");
            d.entities.push(EntityDesc::new(self.entity));
            d.requires = self
                .requires
                .iter()
                .map(|r| ModuleReq {
                    name: r.to_string(),
                    version: "^1".into(),
                    optional: false,
                })
                .collect();
            Some(d)
        }
    }

    fn m(name: &'static str, entity: &'static str, requires: Vec<&'static str>) -> Described {
        Described {
            name,
            entity,
            yaml_plural: format!("{entity}s"),
            requires,
        }
    }

    #[test]
    fn build_host_resolve_independent_of_registration_order() {
        // `billing` requires `users` but is registered FIRST.
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(m("billing", "invoice", vec!["users"]))
            .unwrap()
            .register_module(m("users", "user", vec![]))
            .unwrap()
            .build_host()
            .expect("resolution does not depend on registration order");
        let set = host.descriptors();
        assert!(set.entity("invoice").is_some() && set.entity("user").is_some());
    }

    #[test]
    fn build_host_fails_on_missing_dependency() {
        let err = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(m("billing", "invoice", vec!["users"]))
            .unwrap()
            .build_host()
            .err()
            .expect("missing dependency");
        let c = crate::core::errors::from_anyhow(&err);
        assert_eq!(c.code.as_str(), "this.module.dependency_missing");
    }

    #[test]
    fn plural_drift_fails_at_startup() {
        let mut bad = m("billing", "invoice", vec![]);
        bad.yaml_plural = "invoicez".into();
        let err = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(bad)
            .unwrap()
            .build_host()
            .err()
            .expect("plural drift");
        assert_eq!(
            crate::core::errors::from_anyhow(&err).code.as_str(),
            "this.module.plural_mismatch"
        );
    }

    #[test]
    fn identity_mismatch_between_trait_and_descriptor_fails() {
        struct Liar;
        impl Module for Liar {
            fn name(&self) -> &str {
                "billing"
            }
            fn entity_types(&self) -> Vec<&str> {
                vec![]
            }
            fn links_config(&self) -> anyhow::Result<LinksConfig> {
                Ok(LinksConfig {
                    entities: vec![],
                    links: vec![],
                    validation_rules: None,
                    events: None,
                    sinks: None,
                    auth: None,
                })
            }
            fn register_entities(&self, _registry: &mut EntityRegistry) {}
            fn descriptor(&self) -> Option<ModuleDescriptor> {
                let mut d = ModuleDescriptor::new("billing", "1.0.0");
                d.entities.push(EntityDesc::new("invoice"));
                Some(d)
            }
        }
        let err = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(Liar)
            .unwrap()
            .build_host()
            .err()
            .expect("identity mismatch");
        assert_eq!(
            crate::core::errors::from_anyhow(&err).code.as_str(),
            "this.module.identity_mismatch"
        );
    }

    // ── modules-runtime stream: registration and links of described modules ──

    struct Routes(&'static str, &'static str);
    impl crate::server::entity_registry::EntityDescriptor for Routes {
        fn entity_type(&self) -> &str {
            self.0
        }
        fn plural(&self) -> &str {
            self.1
        }
        fn build_routes(&self) -> axum::Router {
            axum::Router::new()
        }
    }

    /// Module that REGISTERS its entity (legacy or described).
    struct Registering {
        name: &'static str,
        entity: &'static str,
        described: bool,
        links: Vec<(&'static str, &'static str)>,
    }
    impl Module for Registering {
        fn name(&self) -> &str {
            self.name
        }
        fn entity_types(&self) -> Vec<&str> {
            vec![self.entity]
        }
        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            let links = self
                .links
                .iter()
                .map(|(s, t)| crate::core::LinkDefinition {
                    link_type: "owner".into(),
                    source_type: s.to_string(),
                    target_type: t.to_string(),
                    forward_route_name: format!("{t}s"),
                    reverse_route_name: format!("{s}s"),
                    description: None,
                    required_fields: None,
                    auth: None,
                })
                .collect();
            Ok(LinksConfig {
                entities: vec![],
                links,
                validation_rules: None,
                events: None,
                sinks: None,
                auth: None,
            })
        }
        fn register_entities(&self, registry: &mut EntityRegistry) {
            registry.register(Box::new(Routes(self.entity, "invoices")));
        }
        fn descriptor(&self) -> Option<ModuleDescriptor> {
            self.described.then(|| {
                let mut d = ModuleDescriptor::new(self.name, "1.0.0");
                d.entities.push(EntityDesc::new(self.entity));
                d
            })
        }
    }

    fn code_of(r: anyhow::Result<crate::server::host::ServerHost>) -> String {
        let err = r.err().expect("build must fail");
        crate::core::errors::from_anyhow(&err)
            .code
            .as_str()
            .to_string()
    }

    #[test]
    fn duplicate_entity_across_modules_fails() {
        let legacy = |name| Registering {
            name,
            entity: "invoice",
            described: false,
            links: vec![],
        };
        let described = |name| Registering {
            name,
            entity: "invoice",
            described: true,
            links: vec![],
        };
        // Described vs described: refused by resolution.
        let r = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(described("billing"))
            .unwrap()
            .register_module(described("accounting"))
            .unwrap()
            .build_host();
        assert_eq!(code_of(r), "this.module.duplicate_entity");
        // Legacy vs described, in both registration orders: refused by `try_register`.
        for legacy_first in [true, false] {
            let mut b = ServerBuilder::new().with_link_service(InMemoryLinkService::new());
            if legacy_first {
                b = b.register_module(legacy("old")).unwrap();
                b = b.register_module(described("billing")).unwrap();
            } else {
                b = b.register_module(described("billing")).unwrap();
                b = b.register_module(legacy("old")).unwrap();
            }
            assert_eq!(code_of(b.build_host()), "this.module.duplicate_entity");
        }
        // Legacy vs legacy: historical last-wins, unchanged.
        let host = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(legacy("a"))
            .unwrap()
            .register_module(legacy("b"))
            .unwrap()
            .build_host()
            .expect("legacy last-wins kept");
        assert_eq!(host.entity_types(), vec!["invoice"]);
    }

    #[test]
    fn link_to_entity_of_undeclared_module_fails() {
        let billing = Registering {
            name: "billing",
            entity: "invoice",
            described: true,
            links: vec![("user", "invoice")],
        };
        let r = ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(m("users", "user", vec![]))
            .unwrap()
            .register_module(billing)
            .unwrap()
            .build_host();
        assert_eq!(code_of(r), "this.module.reference_unknown");
    }
}
