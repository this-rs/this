//! `ModuleLoader`: turn a source (compiled module, manifest, stored descriptor or plugin)
//! into the interface the host publishes in its catalog.

use super::json_entity::JsonEntityService;
use crate::config::LinksConfig;
use crate::core::events::EventBus;
use crate::core::module::{EntityCreator, EntityFetcher, Module};
use crate::descriptor::framework::codes;
use crate::descriptor::{
    CheckProfile, CompiledEntity, ContractError, CustomRegistry, EntityDesc, ModuleDescriptor,
    ModuleState,
};
use crate::server::entity_registry::EntityRegistry;
use crate::storage::vfs::ModuleStore;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Where a module comes from.
///
/// # Extension point: WASM / WAMI plugins
///
/// There is no stable Rust ABI, so this lot does NOT load native code at runtime, and
/// `wasmtime` is not a dependency of this-rs: no WASM module is executed here. The
/// extension point is [`ModuleSource::Plugin`] + [`DefaultModuleLoader::with_plugin`]: a
/// crate that embeds a WASM runtime (or talks to a WAMI host) implements
/// [`ModuleLoader`] for its `kind` (`"wasm"`, `"wami"`…), reads `location`, and returns
/// a [`ModuleInterface`] (descriptor + fetchers/creators backed by the guest + routes).
/// The runtime treats it exactly like any other source: same `check()`, same
/// resolution, same catalog publication, same drain on unload. An unregistered `kind`
/// is refused with `this.not_implemented` (fail-closed).
#[derive(Clone)]
pub enum ModuleSource {
    /// A module compiled into the binary.
    Compiled(Arc<dyn Module>),
    /// A declarative module read from a `ModuleStore` (checked with `CheckProfile::Stored`).
    Stored {
        store: Arc<dyn ModuleStore>,
        name: String,
        /// `None` = highest stored version.
        version: Option<String>,
    },
    /// A declarative module given inline as a JSON manifest (= a `ModuleDescriptor`),
    /// checked with `CheckProfile::Stored` like a stored one.
    Manifest(String),
    /// A module produced by a plugin loader registered for `kind` (WASM/WAMI extension
    /// point, see the type documentation).
    Plugin { kind: String, location: String },
}

impl std::fmt::Debug for ModuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleSource::Compiled(m) => write!(f, "Compiled({})", m.name()),
            ModuleSource::Stored { name, version, .. } => {
                write!(
                    f,
                    "Stored({name}@{})",
                    version.as_deref().unwrap_or("latest")
                )
            }
            ModuleSource::Manifest(_) => f.write_str("Manifest"),
            ModuleSource::Plugin { kind, location } => write!(f, "Plugin({kind}:{location})"),
        }
    }
}

/// What a loaded module exposes to the host. Published through `HostCatalog` (fetchers,
/// creators, descriptor, links) and the dispatch layer (routes), never through a fallback.
#[derive(Clone)]
pub struct ModuleInterface {
    pub descriptor: Arc<ModuleDescriptor>,
    pub fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    /// Raw creators; the runtime wraps them (`ValidatingCreator`) when building the snapshot.
    pub creators: HashMap<String, Arc<dyn EntityCreator>>,
    /// CRUD routes of the module's entities (`/{plural}`, `/{plural}/{id}`).
    pub routes: Option<axum::Router>,
    /// Links configuration contributed by the module (entities, links, policies), merged
    /// strictly into the host's at load time.
    pub links_config: LinksConfig,
}

/// Observable status of a module.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ModuleStatus {
    pub name: String,
    pub version: String,
    #[serde(flatten)]
    pub state: ModuleState,
    pub requires: Vec<String>,
    /// Entities (singular names) the module serves.
    pub entities: Vec<String>,
    /// `true` for a module compiled into the host at build time (cannot be unloaded).
    pub builtin: bool,
}

/// Read and check a module, build its interface. Does NOT activate it.
#[async_trait]
pub trait ModuleLoader: Send + Sync {
    async fn load(&self, source: &ModuleSource) -> Result<ModuleInterface, ContractError>;
}

/// The loader shipped with this-rs: compiled modules, manifests, stored modules, and
/// plugin loaders registered by `kind`.
#[derive(Clone)]
pub struct DefaultModuleLoader {
    custom: CustomRegistry,
    allow_public: bool,
    event_bus: Option<Arc<EventBus>>,
    plugins: HashMap<String, Arc<dyn ModuleLoader>>,
}

impl Default for DefaultModuleLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultModuleLoader {
    pub fn new() -> Self {
        Self {
            custom: CustomRegistry::default(),
            allow_public: false,
            event_bus: None,
            plugins: HashMap::new(),
        }
    }

    /// Registry used to check and validate declarative modules (`Custom` rules).
    pub fn with_custom_registry(mut self, custom: CustomRegistry) -> Self {
        self.custom = custom;
        self
    }

    /// Host option `allow_public` of `CheckProfile::Stored` (default: `public` refused in
    /// stored/manifest modules).
    pub fn allow_public(mut self, allow: bool) -> Self {
        self.allow_public = allow;
        self
    }

    /// Event bus the declarative entities publish their `EntityEvent`s on.
    pub fn with_event_bus(mut self, bus: Option<Arc<EventBus>>) -> Self {
        self.event_bus = bus;
        self
    }

    /// Register the loader of a plugin `kind` (WASM/WAMI extension point).
    pub fn with_plugin(mut self, kind: impl Into<String>, loader: Arc<dyn ModuleLoader>) -> Self {
        self.plugins.insert(kind.into(), loader);
        self
    }

    fn checked(
        &self,
        mut d: ModuleDescriptor,
        profile: CheckProfile,
    ) -> Result<ModuleDescriptor, ContractError> {
        d.normalize();
        d.check(&self.custom, profile)
            .map_err(|issues| crate::descriptor::check::issues_to_error(&issues))?;
        Ok(d)
    }

    /// Interface of a declarative descriptor: one `JsonEntityService` per entity.
    fn declarative(&self, d: ModuleDescriptor) -> Result<ModuleInterface, ContractError> {
        let d = self.checked(
            d,
            CheckProfile::Stored {
                allow_public: self.allow_public,
            },
        )?;
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        let mut router = axum::Router::new();
        for e in &d.entities {
            let compiled = Arc::new(CompiledEntity::new(&d.name, e.clone())?);
            let svc = Arc::new(JsonEntityService::new(
                compiled,
                self.custom.clone(),
                self.event_bus.clone(),
            ));
            router = router.merge(svc.routes());
            fetchers.insert(e.name.clone(), svc.clone());
            creators.insert(e.name.clone(), svc);
        }
        Ok(ModuleInterface {
            links_config: LinksConfig::from_descriptor(&d),
            descriptor: Arc::new(d),
            fetchers,
            creators,
            routes: Some(router),
        })
    }

    /// Interface of a compiled module. Without `Module::descriptor()`, a minimal
    /// descriptor is derived from the trait (name, version, entity types and the plurals
    /// of its `EntityDescriptor`s): the runtime needs one to resolve and to dispatch.
    fn compiled(&self, module: &Arc<dyn Module>) -> Result<ModuleInterface, ContractError> {
        let mut registry = EntityRegistry::new();
        module.register_entities(&mut registry);
        let plurals: HashMap<String, String> = registry.entity_plurals().into_iter().collect();
        let config = module.links_config().map_err(|e| {
            ContractError::framework(codes::DESCRIPTOR_INVALID)
                .with_param("issues", format!("links_config: {e}"))
        })?;
        let declared_descriptor = module.descriptor();
        let derived = declared_descriptor.is_none();
        let descriptor = match declared_descriptor {
            Some(d) => self.checked(d, CheckProfile::Compiled)?,
            None => {
                let mut d = ModuleDescriptor::new(
                    crate::descriptor::naming::to_snake_identifier(module.name()),
                    module.version(),
                );
                for t in module.entity_types() {
                    let mut e = EntityDesc::new(t);
                    if let Some(p) = plurals.get(t) {
                        e.plural = p.clone();
                    }
                    d.entities.push(e);
                }
                self.checked(d, CheckProfile::Compiled)?
            }
        };
        // Identity declared once (same rule as `ServerBuilder::build_host`, plus the
        // reverse direction): name, version and entity types of the trait must match the
        // descriptor. An extra `entity_types()` entry would bring a fetcher/creator the
        // descriptor does not own; a different name would make `offer_module` list the
        // module under one name and load it under another.
        let mut mismatches = Vec::new();
        if crate::descriptor::naming::to_snake_identifier(module.name()) != descriptor.name {
            mismatches.push(format!("name `{}` vs `{}`", module.name(), descriptor.name));
        }
        if module.version() != descriptor.version {
            mismatches.push(format!(
                "version `{}` vs `{}`",
                module.version(),
                descriptor.version
            ));
        }
        let types: std::collections::BTreeSet<&str> = module.entity_types().into_iter().collect();
        let declared: std::collections::BTreeSet<&str> = descriptor
            .entities
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        for t in types.difference(&declared) {
            mismatches.push(format!("entity_types() has `{t}`, the descriptor does not"));
        }
        for t in declared.difference(&types) {
            mismatches.push(format!("descriptor has `{t}`, entity_types() does not"));
        }
        if !mismatches.is_empty() {
            return Err(ContractError::framework(codes::MODULE_IDENTITY_MISMATCH)
                .with_param("module", &descriptor.name)
                .with_param("what", mismatches.join("; ")));
        }
        // One owner per plural: descriptor, `EntityDescriptor` and links.yaml must agree.
        for e in &descriptor.entities {
            let registered = plurals.get(&e.name);
            let yaml = config.entities.iter().find(|c| c.singular == e.name);
            if registered.is_some_and(|p| p != &e.plural)
                || yaml.is_some_and(|c| c.plural != e.plural)
            {
                return Err(ContractError::framework(codes::MODULE_PLURAL_MISMATCH)
                    .with_param("entity", &e.name)
                    .with_param(
                        "sources",
                        format!(
                            "descriptor={}, EntityDescriptor={:?}, links.yaml={:?}",
                            e.plural,
                            registered,
                            yaml.map(|c| &c.plural)
                        ),
                    ));
            }
        }
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        for t in module.entity_types() {
            if let Some(f) = module.get_entity_fetcher(t) {
                fetchers.insert(t.to_string(), f);
            }
            if let Some(c) = module.get_entity_creator(t) {
                creators.insert(t.to_string(), c);
            }
        }
        let mut links_config = config.clone().overlay_descriptor(&descriptor);
        if derived {
            // A DERIVED descriptor declares no policy: overlaying it would replace the
            // module's own YAML `auth:` (e.g. `create: role:admin`) with the default
            // `authenticated` (fail-open, found in review). Its YAML entries stay
            // authoritative; the overlay only adds the entities YAML does not list.
            for e in links_config.entities.iter_mut() {
                if let Some(yaml) = config.entities.iter().find(|c| c.singular == e.singular) {
                    *e = yaml.clone();
                }
            }
        }
        Ok(ModuleInterface {
            links_config,
            descriptor: Arc::new(descriptor),
            fetchers,
            creators,
            routes: Some(registry.build_routes()),
        })
    }
}

#[async_trait]
impl ModuleLoader for DefaultModuleLoader {
    async fn load(&self, source: &ModuleSource) -> Result<ModuleInterface, ContractError> {
        match source {
            ModuleSource::Compiled(m) => self.compiled(m),
            ModuleSource::Manifest(json) => self.declarative(ModuleDescriptor::from_json(json)?),
            ModuleSource::Stored {
                store,
                name,
                version,
            } => self.declarative(store.get(name, version.as_deref()).await?),
            ModuleSource::Plugin { kind, location } => match self.plugins.get(kind) {
                Some(loader) => loader.load(source).await,
                None => Err(ContractError::framework(codes::NOT_IMPLEMENTED)
                    .with_param("what", format!("module plugin `{kind}` ({location})"))),
            },
        }
    }
}
