//! `ModuleRuntime`: load, replace and unload modules on a LIVE host.
//!
//! # Mechanism
//!
//! - One runtime per host, created by `ServerBuilder::with_module_runtime()`. Every
//!   change is serialized by one async mutex (`LiveCatalog::publish` is last-writer-wins
//!   on the whole snapshot, so publishers must not race).
//! - A change never edits a snapshot in place: the next `HostCatalog` is COMPOSED from the
//!   host's base snapshot (what `build_host` produced) plus the interface of every active
//!   runtime module, then published with one atomic store. A failed composition
//!   (conflict, duplicate) publishes nothing.
//! - The dispatch table (plural → module router, link routes of runtime modules,
//!   draining set, descriptors) is rebuilt from the same composition and swapped
//!   atomically too (`ArcSwap`). Requests read it once (see `dispatch.rs`).
//! - Lifecycle per module: `Discovered → Resolved → Active → Draining → Unloaded`, with
//!   `Failed` on a refused load. Every transition goes through `ModuleState::transition`
//!   (illegal ones are errors, never silent).
//! - Each load / unload / replace / failure publishes a `FrameworkEvent::Module` on the
//!   host's event bus.

use super::dispatch::{DispatchTable, Gate};
use super::loader::{ModuleInterface, ModuleLoader, ModuleSource, ModuleStatus};
use crate::config::LinksConfig;
use crate::core::auth::AuthResolverRegistry;
use crate::core::events::{EventBus, FrameworkEvent, ModuleEvent};
use crate::core::service::LinkService;
use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, DescriptorSet, ModuleDescriptor, ModuleState, PolicyExpr};
use crate::links::handlers::AppState;
use crate::links::registry::LinkRouteRegistry;
use crate::server::catalog::{HostCatalog, LiveCatalog};
use crate::storage::vfs::ModuleStore;
use arc_swap::ArcSwap;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Options of the runtime (host-level decisions).
#[derive(Clone)]
pub struct RuntimeOptions {
    /// `Draining` grace delay before streams are cancelled (default 30 s,
    /// `DEFAULT_DRAIN_GRACE_MS`).
    pub drain_grace: Duration,
    /// Policy of the admin interface (default `admin_only`).
    pub admin_policy: PolicyExpr,
    /// Path prefix of the admin interface (default `/admin/modules`).
    pub admin_prefix: String,
    /// `hot_load.enabled`: load / replace / unload and the admin interface are refused with
    /// `this.module.hot_load_disabled` unless this is set explicitly (default `false`: closed).
    pub hot_load_enabled: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            drain_grace: Duration::from_millis(
                crate::descriptor::lifecycle::DEFAULT_DRAIN_GRACE_MS,
            ),
            admin_policy: PolicyExpr::AdminOnly,
            admin_prefix: "/admin/modules".into(),
            hot_load_enabled: false,
        }
    }
}

/// Everything the runtime needs from the host, captured once at build time.
pub struct HostParts {
    pub catalog: Arc<LiveCatalog>,
    pub link_service: Arc<dyn LinkService>,
    pub event_bus: Option<Arc<EventBus>>,
    pub resolvers: Arc<AuthResolverRegistry>,
    /// `ServerHost::authorize` (policies of runtime modules enforced by the dispatch layer).
    pub authorize: bool,
    pub report_only: bool,
    /// `ServerHost::validation_registry`: creators and REST routes of runtime modules are
    /// validated by the same engine as the host's (`ValidatingCreator`, validation layer).
    pub validation_registry: crate::core::validation::SharedRegistry,
    /// `ServerHost::locales`: bundles of a stored module are added on load, removed on
    /// unload, replaced on replace.
    pub locales: crate::core::i18n::LocaleCatalog,
}

struct Record {
    descriptor: Arc<ModuleDescriptor>,
    state: ModuleState,
    builtin: bool,
    iface: Option<ModuleInterface>,
    gate: Arc<Gate>,
}

#[derive(Default)]
struct Inner {
    records: BTreeMap<String, Record>,
    /// Sources offered for loading by name (admin `POST {"module": name}`).
    offers: BTreeMap<String, ModuleSource>,
}

/// The runtime of one host.
pub struct ModuleRuntime {
    parts: HostParts,
    /// Snapshot produced by `build_host`: the base every composition starts from.
    base: HostCatalog,
    loader: Arc<dyn ModuleLoader>,
    store: Option<Arc<dyn ModuleStore>>,
    pub(crate) options: RuntimeOptions,
    /// Serializes changes (load / unload / replace). Held across the loader and the drain.
    ops: tokio::sync::Mutex<()>,
    /// State, locked briefly and NEVER across an `.await` (status stays readable while a
    /// module drains).
    inner: std::sync::Mutex<Inner>,
    pub(crate) table: ArcSwap<DispatchTable>,
    /// Test fault injection: the Nth next `compose` fails (0 = off).
    #[cfg(test)]
    pub(crate) fail_compose_at: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for ModuleRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleRuntime")
            .field("admin_prefix", &self.options.admin_prefix)
            .finish_non_exhaustive()
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Several resolution errors → one error (the first), the others' codes in `also`.
fn one_error(mut errs: Vec<ContractError>) -> ContractError {
    let first = errs.remove(0);
    if errs.is_empty() {
        first
    } else {
        let also: Vec<String> = errs
            .iter()
            .map(|e| format!("{} {:?}", e.code.as_str(), e.params))
            .collect();
        first.with_param("also", also)
    }
}

impl ModuleRuntime {
    /// Runtime over a host's parts. Modules with a descriptor already in the catalog
    /// (built into the host) are recorded as `Active` and `builtin`.
    pub fn new(
        parts: HostParts,
        loader: Arc<dyn ModuleLoader>,
        store: Option<Arc<dyn ModuleStore>>,
        options: RuntimeOptions,
        offers: Vec<(String, ModuleSource)>,
    ) -> Arc<Self> {
        let base = (*parts.catalog.load()).clone();
        let mut inner = Inner::default();
        inner.offers.extend(offers);
        for d in base.descriptors.modules() {
            inner.records.insert(
                d.name.clone(),
                Record {
                    descriptor: d.clone(),
                    state: ModuleState::Active,
                    builtin: true,
                    iface: None,
                    gate: Arc::new(Gate::new()),
                },
            );
        }
        let table = DispatchTable::empty(base.descriptors.clone());
        Arc::new(Self {
            parts,
            base,
            loader,
            store,
            options,
            ops: tokio::sync::Mutex::new(()),
            inner: std::sync::Mutex::new(inner),
            table: ArcSwap::from_pointee(table),
            #[cfg(test)]
            fail_compose_at: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Hot loading is closed unless the host enabled it (`hot_load.enabled`).
    pub(crate) fn ensure_hot_load(&self, operation: &str) -> Result<(), ContractError> {
        if self.options.hot_load_enabled {
            Ok(())
        } else {
            Err(ContractError::framework(codes::MODULE_HOT_LOAD_DISABLED)
                .with_param("operation", operation))
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn parts(&self) -> &HostParts {
        &self.parts
    }

    pub(crate) fn store(&self) -> Option<&Arc<dyn ModuleStore>> {
        self.store.as_ref()
    }

    fn emit(&self, e: ModuleEvent) {
        if let Some(bus) = &self.parts.event_bus {
            bus.publish(FrameworkEvent::Module(e));
        }
    }

    /// Offer a source for loading by name (listed as `Discovered`, loadable from the
    /// admin interface with `{"module": "<name>"}`).
    pub async fn offer(&self, name: impl Into<String>, source: ModuleSource) {
        self.lock().offers.insert(name.into(), source);
    }

    /// Status of every known module (active, draining, unloaded, failed, offered).
    pub async fn list(&self) -> Vec<ModuleStatus> {
        let inner = self.lock();
        let mut out: Vec<ModuleStatus> = inner
            .records
            .iter()
            .map(|(name, r)| Self::status_of(name, r))
            .collect();
        for name in inner.offers.keys() {
            if !inner.records.contains_key(name) {
                out.push(ModuleStatus {
                    name: name.clone(),
                    version: String::new(),
                    state: ModuleState::Discovered,
                    requires: vec![],
                    entities: vec![],
                    builtin: false,
                });
            }
        }
        out
    }

    /// Status of one module.
    pub async fn status(&self, name: &str) -> Result<ModuleStatus, ContractError> {
        self.list()
            .await
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| {
                ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", name)
            })
    }

    fn status_of(name: &str, r: &Record) -> ModuleStatus {
        ModuleStatus {
            name: name.to_string(),
            version: r.descriptor.version.clone(),
            state: r.state.clone(),
            requires: r
                .descriptor
                .requires
                .iter()
                .map(|q| format!("{} {}", q.name, q.version))
                .collect(),
            entities: r
                .descriptor
                .entities
                .iter()
                .map(|e| e.name.clone())
                .collect(),
            builtin: r.builtin,
        }
    }

    /// Load an offered source by name.
    pub async fn load_offered(&self, name: &str) -> Result<ModuleStatus, ContractError> {
        let source = self.lock().offers.get(name).cloned();
        match source {
            Some(s) => self.load(s).await,
            None => {
                Err(ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", name))
            }
        }
    }

    /// Load and activate a module: read and check (loader), resolve against the active
    /// modules (dependencies, semver, cycles, duplicates, references), compose and
    /// publish the next catalog and dispatch table. Fail-closed: on any error nothing is
    /// published and the module is recorded `Failed`.
    pub async fn load(&self, source: ModuleSource) -> Result<ModuleStatus, ContractError> {
        self.ensure_hot_load("load")?;
        let _op = self.ops.lock().await;
        let iface = match self.loader.load(&source).await {
            Ok(i) => i,
            Err(e) => {
                self.emit(ModuleEvent::Failed {
                    module: format!("{source:?}"),
                    code: e.code.as_str().to_string(),
                });
                return Err(e);
            }
        };
        // Bundles of a stored module, checked BEFORE anything is published (fail-closed: a
        // bundle the catalog would refuse refuses the load).
        let bundles = match Self::bundles_of(&source, &iface.descriptor.name).await {
            Ok(b) => b,
            Err(e) => {
                self.emit(ModuleEvent::Failed {
                    module: format!("{source:?}"),
                    code: e.code.as_str().to_string(),
                });
                return Err(e);
            }
        };
        let name = iface.descriptor.name.clone();
        let descriptor = iface.descriptor.clone();
        let mut inner = self.lock();
        match self.activate(&mut inner, iface) {
            Ok(status) => {
                self.publish_bundles(&status.name, bundles);
                Ok(status)
            }
            Err(e) => {
                let failed = ModuleState::Failed {
                    code: e.code.clone(),
                    at: now(),
                };
                // Never overwrite a live record (duplicate load of an active module).
                let live = inner.records.get(&name).is_some_and(|r| {
                    matches!(r.state, ModuleState::Active | ModuleState::Draining)
                });
                if !live {
                    inner.records.insert(
                        name.clone(),
                        Record {
                            descriptor,
                            state: failed,
                            builtin: false,
                            iface: None,
                            gate: Arc::new(Gate::new()),
                        },
                    );
                }
                self.emit(ModuleEvent::Failed {
                    module: name,
                    code: e.code.as_str().to_string(),
                });
                Err(e)
            }
        }
    }

    /// Message bundles of `source` (stored modules only; other sources have none yet:
    /// `Module` has no bundle hook). Refused, nothing published, when one belongs to
    /// another module or would be refused by the catalog (`I18nCatalog::bundle_issues`).
    async fn bundles_of(
        source: &ModuleSource,
        module: &str,
    ) -> Result<Vec<crate::descriptor::MessageBundle>, ContractError> {
        let Some(stored) = crate::storage::vfs::read_source(source).await? else {
            return Ok(Vec::new());
        };
        for b in &stored.bundles {
            let issues = crate::descriptor::I18nCatalog::bundle_issues(b);
            if b.module != module || !issues.is_empty() {
                return Err(ContractError::framework(codes::DESCRIPTOR_INVALID)
                    .with_param("module", module)
                    .with_param(
                        "reason",
                        format!(
                            "bundle {}/{} refused: {issues:?}",
                            b.module,
                            b.locale.as_str()
                        ),
                    ));
            }
        }
        Ok(stored.bundles)
    }

    /// Replace the bundles of `module` in the host catalog (already checked by
    /// `bundles_of`, so `add_bundles` cannot refuse them).
    fn publish_bundles(&self, module: &str, bundles: Vec<crate::descriptor::MessageBundle>) {
        self.parts.locales.remove_module(module);
        if bundles.is_empty() {
            return;
        }
        if let Err(e) = self.parts.locales.add_bundles(bundles) {
            tracing::error!(%module, code = %e.code.as_str(), "module bundles refused after check");
        }
    }

    fn active_descriptors(inner: &Inner, except: Option<&str>) -> Vec<Arc<ModuleDescriptor>> {
        inner
            .records
            .iter()
            .filter(|(n, r)| {
                Some(n.as_str()) != except
                    && matches!(r.state, ModuleState::Active | ModuleState::Draining)
            })
            .map(|(_, r)| r.descriptor.clone())
            .collect()
    }

    fn activate(
        &self,
        inner: &mut Inner,
        iface: ModuleInterface,
    ) -> Result<ModuleStatus, ContractError> {
        let d = iface.descriptor.clone();
        let active = Self::active_descriptors(inner, None);
        let refs: Vec<&ModuleDescriptor> = active.iter().map(|a| a.as_ref()).collect();
        crate::descriptor::resolve::resolve(&[d.as_ref()], &refs).map_err(one_error)?;

        let mut state = match inner.records.get(&d.name) {
            // Reload after unload / failure: Unloaded|Failed -> Discovered.
            Some(r) => {
                let mut s = r.state.clone();
                s.transition(ModuleState::Discovered)?;
                s
            }
            None => ModuleState::Discovered,
        };
        state.transition(ModuleState::Resolved)?;

        // Compose with the candidate as active; nothing is published on error.
        let gate = Arc::new(Gate::new());
        let mut candidate = Record {
            descriptor: d.clone(),
            state: ModuleState::Active,
            builtin: false,
            iface: Some(iface),
            gate,
        };
        let previous = inner.records.remove(&d.name);
        inner.records.insert(d.name.clone(), candidate);
        let composed = self.compose(inner);
        let (catalog, table) = match composed {
            Ok(x) => x,
            Err(e) => {
                let mut c = inner.records.remove(&d.name).expect("just inserted");
                if let Some(p) = previous {
                    inner.records.insert(d.name.clone(), p);
                } else {
                    c.state = ModuleState::Failed {
                        code: e.code.clone(),
                        at: now(),
                    };
                    c.iface = None;
                    inner.records.insert(d.name.clone(), c);
                }
                return Err(e);
            }
        };
        state.transition(ModuleState::Active)?;
        candidate = inner.records.remove(&d.name).expect("just inserted");
        candidate.state = state;
        inner.records.insert(d.name.clone(), candidate);

        let published = self.parts.catalog.publish(catalog);
        self.table.store(Arc::new(table));
        tracing::info!(module = %d.name, version = %d.version, generation = published.generation, "module loaded");
        self.emit(ModuleEvent::Loaded {
            module: d.name.clone(),
            version: d.version.clone(),
            generation: published.generation,
        });
        Ok(Self::status_of(&d.name, &inner.records[&d.name]))
    }

    /// Unload a module: refused while an active module requires it
    /// (`this.module.has_dependents`) and for modules built into the host. The module
    /// goes `Draining` (new requests: 503 `this.module.unavailable`), in-flight requests
    /// get up to `drain_grace` to finish, then streams are cancelled, the next catalog is
    /// published without it, and it is `Unloaded`.
    pub async fn unload(&self, name: &str) -> Result<ModuleStatus, ContractError> {
        self.ensure_hot_load("unload")?;
        let _op = self.ops.lock().await;
        let gate = self.begin_drain(name)?;
        if !gate.wait_idle(self.options.drain_grace).await {
            tracing::warn!(module = %name, "drain grace elapsed: cancelling in-flight streams");
        }
        gate.cancel();

        let mut inner = self.lock();
        let r = inner.records.get_mut(name).expect("checked");
        r.state.transition(ModuleState::Unloaded)?;
        let saved_iface = r.iface.take();
        let (catalog, table) = match self.compose(&inner) {
            Ok(c) => c,
            Err(e) => {
                // Compose failed: the module must stay served, coherently. Back to `Active`
                // with its interface and a fresh gate (the old one was cancelled), and the
                // dispatch table recomposed WITHOUT the draining state.
                let r = inner.records.get_mut(name).expect("checked");
                r.state = ModuleState::Active;
                r.iface = saved_iface;
                r.gate = Arc::new(Gate::new());
                if let Ok((_, table)) = self.compose(&inner) {
                    self.table.store(Arc::new(table));
                }
                self.emit(ModuleEvent::Failed {
                    module: name.to_string(),
                    code: e.code.as_str().to_string(),
                });
                return Err(e);
            }
        };
        let published = self.parts.catalog.publish(catalog);
        self.table.store(Arc::new(table));
        self.parts.locales.remove_module(name);
        let r = &inner.records[name];
        tracing::info!(module = %name, generation = published.generation, "module unloaded");
        self.emit(ModuleEvent::Unloaded {
            module: name.to_string(),
            version: r.descriptor.version.clone(),
            generation: published.generation,
        });
        Ok(Self::status_of(name, r))
    }

    /// Checks of `unload`, then `Active -> Draining` and the draining dispatch table.
    fn begin_drain(&self, name: &str) -> Result<Arc<Gate>, ContractError> {
        let mut inner = self.lock();
        let record = inner.records.get(name).ok_or_else(|| {
            ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", name)
        })?;
        if record.builtin {
            return Err(ContractError::framework(codes::NOT_IMPLEMENTED).with_param(
                "what",
                format!("unload of `{name}`, built into the host (rebuild the host instead)"),
            ));
        }
        if record.state != ModuleState::Active {
            let mut s = record.state.clone();
            s.transition(ModuleState::Draining)?;
        }
        let active = Self::active_descriptors(&inner, Some(name));
        let refs: Vec<&ModuleDescriptor> = active.iter().map(|a| a.as_ref()).collect();
        let deps = crate::descriptor::resolve::dependents(name, &refs);
        if !deps.is_empty() {
            return Err(ContractError::framework(codes::MODULE_HAS_DEPENDENTS)
                .with_param("module", name)
                .with_param("dependents", deps.join(", ")));
        }

        // Draining: dispatch answers 503 for this module from now on.
        let gate = {
            let r = inner.records.get_mut(name).expect("checked");
            r.state.transition(ModuleState::Draining)?;
            r.gate.clone()
        };
        match self.compose(&inner) {
            Ok((_, table)) => {
                self.table.store(Arc::new(table));
                Ok(gate)
            }
            Err(e) => {
                inner.records.get_mut(name).expect("checked").state = ModuleState::Active;
                Err(e)
            }
        }
    }

    /// Replace an active module by another version, in ONE publication: every active
    /// module that requires it must accept the new version (`VersionReq`), and the new
    /// version must itself resolve. In-flight requests finish on the old version (they
    /// hold its router); new ones see the new version.
    pub async fn replace(&self, source: ModuleSource) -> Result<ModuleStatus, ContractError> {
        self.ensure_hot_load("replace")?;
        let _op = self.ops.lock().await;
        let label = format!("{source:?}");
        let result = self.replace_locked(source).await;
        if let Err(e) = &result {
            // Same observability as a refused load: every refused change is an event.
            self.emit(ModuleEvent::Failed {
                module: label,
                code: e.code.as_str().to_string(),
            });
        }
        result
    }

    async fn replace_locked(&self, source: ModuleSource) -> Result<ModuleStatus, ContractError> {
        let iface = self.loader.load(&source).await?;
        let bundles = Self::bundles_of(&source, &iface.descriptor.name).await?;
        let mut inner = self.lock();
        let d = iface.descriptor.clone();
        let Some(old) = inner.records.get(&d.name) else {
            return Err(
                ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", &d.name)
            );
        };
        if old.builtin || old.state != ModuleState::Active {
            return Err(
                ContractError::framework(codes::MODULE_UNAVAILABLE).with_param("module", &d.name)
            );
        }
        // `resolve` checks the `requires` of EVERY module it sees, active ones included:
        // a dependent whose `VersionReq` rejects the new version is reported as
        // `this.module.dependency_version {module: <dependent>}`.
        let others = Self::active_descriptors(&inner, Some(&d.name));
        let refs: Vec<&ModuleDescriptor> = others.iter().map(|a| a.as_ref()).collect();
        crate::descriptor::resolve::resolve(&[d.as_ref()], &refs).map_err(one_error)?;

        let old = inner.records.remove(&d.name).expect("checked");
        let from = old.descriptor.version.clone();
        inner.records.insert(
            d.name.clone(),
            Record {
                descriptor: d.clone(),
                state: ModuleState::Active,
                builtin: false,
                iface: Some(iface),
                gate: Arc::new(Gate::new()),
            },
        );
        let (catalog, table) = match self.compose(&inner) {
            Ok(x) => x,
            Err(e) => {
                inner.records.insert(d.name.clone(), old);
                return Err(e);
            }
        };
        let published = self.parts.catalog.publish(catalog);
        self.table.store(Arc::new(table));
        self.publish_bundles(&d.name, bundles);
        // The OLD version drains in the background: requests already in flight finish
        // on it (they hold its router), and after the grace delay its streams are
        // cancelled. Without this, a stream opened on the old version would stay open
        // forever on code that is no longer published (found in review).
        let old_gate = old.gate.clone();
        let grace = self.options.drain_grace;
        let module = d.name.clone();
        tokio::spawn(async move {
            if !old_gate.wait_idle(grace).await {
                tracing::warn!(%module, "replace: grace elapsed, cancelling the old version's streams");
            }
            old_gate.cancel();
        });
        self.emit(ModuleEvent::Replaced {
            module: d.name.clone(),
            from,
            to: d.version.clone(),
            generation: published.generation,
        });
        Ok(Self::status_of(&d.name, &inner.records[&d.name]))
    }

    /// A runtime module's REST router behind the descriptor validation layer. The dispatch
    /// layer sits OUTSIDE the host's validation layer (see `rest/mod.rs`), so without this a
    /// hot-loaded compiled module's create/update would reach its store unchecked.
    fn validated_routes(&self, router: axum::Router) -> axum::Router {
        let catalog = self.parts.catalog.clone();
        let state = crate::core::validation::ValidationLayerState::new(
            Arc::new(move || catalog.load().descriptors.clone()),
            self.parts.validation_registry.clone(),
        );
        router.layer(axum::middleware::from_fn_with_state(
            state,
            crate::core::validation::validation_middleware,
        ))
    }

    /// Next catalog and dispatch table: base snapshot + every Active/Draining runtime
    /// module. Duplicates and configuration conflicts are errors (no last-wins).
    fn compose(&self, inner: &Inner) -> Result<(HostCatalog, DispatchTable), ContractError> {
        #[cfg(test)]
        if self
            .fail_compose_at
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
            && self
                .fail_compose_at
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
                == 1
        {
            return Err(ContractError::framework(codes::INTERNAL).with_param("reason", "injected"));
        }
        let mut descriptors: DescriptorSet = (*self.base.descriptors).clone();
        let mut fetchers = (*self.base.fetchers).clone();
        let mut creators = (*self.base.creators).clone();
        let mut plurals = (*self.base.plurals).clone();
        let mut configs: Vec<LinksConfig> = vec![(*self.base.links_config).clone()];
        let mut table = DispatchTable::default();
        let mut runtime_links = Vec::new();

        for (name, r) in &inner.records {
            if !r.builtin && r.state == ModuleState::Unloaded {
                for e in &r.descriptor.entities {
                    table.tombstones.insert(e.plural.clone(), name.clone());
                }
            }
            if r.builtin || !matches!(r.state, ModuleState::Active | ModuleState::Draining) {
                continue;
            }
            let Some(iface) = &r.iface else { continue };
            descriptors.insert((*iface.descriptor).clone())?;
            for e in &iface.descriptor.entities {
                if fetchers.contains_key(&e.name)
                    || creators.contains_key(&e.name)
                    || plurals.iter().any(|(t, _)| t == &e.name)
                {
                    return Err(ContractError::framework(codes::MODULE_DUPLICATE_ENTITY)
                        .with_param("entity", &e.name)
                        .with_param("modules", format!("host, {name}")));
                }
                if plurals.iter().any(|(_, p)| p == &e.plural) {
                    return Err(ContractError::framework(codes::MODULE_DUPLICATE_PLURAL)
                        .with_param("plural", &e.plural));
                }
                plurals.push((e.name.clone(), e.plural.clone()));
                if let Some(router) = &iface.routes {
                    table.add_entity(
                        &e.plural,
                        name,
                        self.validated_routes(router.clone()),
                        r.gate.clone(),
                    );
                }
            }
            // A module only contributes fetchers / creators for the entities its
            // descriptor declares (all checked above). Anything else would silently
            // REPLACE a host or other module's fetcher (last-wins), whatever loader
            // produced the interface (plugin loaders included).
            for k in iface.fetchers.keys().chain(iface.creators.keys()) {
                if !iface.descriptor.entities.iter().any(|e| &e.name == k) {
                    return Err(ContractError::framework(codes::MODULE_IDENTITY_MISMATCH)
                        .with_param("module", name)
                        .with_param(
                            "what",
                            format!("fetcher/creator for `{k}`, an entity the descriptor does not declare"),
                        ));
                }
            }
            for (k, v) in &iface.fetchers {
                fetchers.insert(k.clone(), v.clone());
            }
            // Creators validated by the engine (GraphQL, gRPC and link handlers reach them
            // through the catalog). The interface keeps the RAW creators, so a later
            // composition never wraps twice.
            let mut own = DescriptorSet::new();
            own.insert((*iface.descriptor).clone())?;
            let validated = crate::core::validation::wrap_creators(
                &iface.creators,
                &own,
                &self.parts.validation_registry,
            );
            for (k, v) in validated {
                creators.insert(k, v);
            }
            configs.push(iface.links_config.clone());
            runtime_links.extend(
                iface
                    .links_config
                    .links
                    .iter()
                    .map(|l| (name.clone(), l.clone())),
            );
            if r.state == ModuleState::Draining {
                table.draining.insert(name.clone());
            }
        }
        let links_config = Arc::new(LinksConfig::merge_strict(configs).map_err(one_error)?);
        let fetchers = Arc::new(fetchers);
        let creators = Arc::new(creators);
        let plural_of: HashMap<&str, &str> = plurals
            .iter()
            .map(|(e, p)| (e.as_str(), p.as_str()))
            .collect();
        let mut link_routes = HashSet::new();
        for (module, l) in &runtime_links {
            if let Some(p) = plural_of.get(l.source_type.as_str()) {
                link_routes.insert((p.to_string(), l.forward_route_name.clone(), module.clone()));
            }
            if let Some(p) = plural_of.get(l.target_type.as_str()) {
                link_routes.insert((p.to_string(), l.reverse_route_name.clone(), module.clone()));
            }
        }
        let descriptors = Arc::new(descriptors);
        if !table.is_empty() || !link_routes.is_empty() {
            table.links = Some(crate::server::router::build_link_routes(AppState {
                link_service: self.parts.link_service.clone(),
                config: links_config.clone(),
                registry: Arc::new(LinkRouteRegistry::new(links_config.clone())),
                entity_fetchers: fetchers.clone(),
                entity_creators: creators.clone(),
                event_bus: self.parts.event_bus.clone(),
            }));
        }
        for (plural, route, module) in link_routes {
            let gate = inner.records.get(&module).map(|r| r.gate.clone());
            table
                .link_routes
                .insert((plural, route), (module, gate.unwrap_or_default()));
        }
        table
            .tombstones
            .retain(|p, _| !plurals.iter().any(|(_, q)| q == p));
        table.descriptors = descriptors.clone();
        table.links_config = Some(links_config.clone());
        table.plurals = Arc::new(plurals.clone());
        let catalog = HostCatalog {
            descriptors,
            fetchers,
            creators,
            links_config,
            plurals: Arc::new(plurals),
            generation: 0,
        };
        Ok((catalog, table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{
        EntityDesc, FieldDesc, FieldType, ModuleDescriptor, Operation, PolicyExpr,
    };
    use crate::server::RestExposure;
    use crate::server::builder::ServerBuilder;
    use crate::storage::in_memory::InMemoryLinkService;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    fn billing() -> ModuleDescriptor {
        let mut d = ModuleDescriptor::new("billing", "1.0.0");
        let mut e = EntityDesc::new("invoice");
        e.fields.push(FieldDesc::new("amount", FieldType::F64));
        for op in Operation::CRUD {
            e.permissions.ops.insert(op, PolicyExpr::Authenticated);
        }
        d.entities.push(e);
        d.normalize();
        d
    }

    async fn get(app: &axum::Router, uri: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn unload_compose_failure_keeps_module_consistent() {
        let host = std::sync::Arc::new(
            ServerBuilder::new()
                .with_link_service(InMemoryLinkService::new())
                .with_module_runtime()
                .with_hot_load_enabled(true)
                .without_authorization()
                .build_host()
                .expect("host"),
        );
        let app = RestExposure::build_router(host.clone(), vec![]).expect("router");
        let rt = host.module_runtime.clone().expect("runtime");
        rt.load(ModuleSource::Manifest(billing().to_json_pretty()))
            .await
            .expect("load");
        assert_eq!(get(&app, "/invoices").await, StatusCode::OK);
        let generation = host.catalog.load().generation;

        // 1st compose = draining table, 2nd = the unloaded catalog: make the 2nd fail.
        rt.fail_compose_at
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let err = rt.unload("billing").await.unwrap_err();
        assert_eq!(err.code.as_str(), "this.internal");

        // Coherent: still Active, still in the catalog (nothing published), still served
        // (not 503 draining, not tombstoned 404), stream gate usable again.
        let st = rt.status("billing").await.unwrap();
        assert_eq!(st.state, ModuleState::Active);
        assert_eq!(host.catalog.load().generation, generation);
        assert!(host.catalog.load().entity_of_plural("invoices").is_some());
        assert_eq!(get(&app, "/invoices").await, StatusCode::OK);

        // And the module can still be unloaded normally afterwards.
        let st = rt.unload("billing").await.expect("unload retry");
        assert_eq!(st.state, ModuleState::Unloaded);
        assert_eq!(get(&app, "/invoices").await, StatusCode::NOT_FOUND);
    }
}
