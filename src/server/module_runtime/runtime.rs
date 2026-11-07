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
use crate::core::module::Module;
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
    /// Trait object of a hot-loaded compiled module, kept for `stop` / `health`. `None` for
    /// built-in and declarative modules (there is nothing to call).
    lifecycle: Option<Arc<dyn Module>>,
}

#[derive(Default)]
struct Inner {
    records: BTreeMap<String, Record>,
    /// Sources offered for loading by name (admin `POST {"module": name}`).
    offers: BTreeMap<String, ModuleSource>,
}

/// One module as the host serves it: its identity, state AND its whole descriptor, views
/// and actions included. This is what the host reads to draw what a module declares.
///
/// Built under the same lock as the catalog composition, so the descriptor listed here is
/// the one the dispatch table and the catalog serve (no window where a replaced module
/// shows its old views).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServedDescriptor {
    pub name: String,
    pub version: String,
    #[serde(flatten)]
    pub state: ModuleState,
    /// `true` for a module compiled into the host at build time.
    pub builtin: bool,
    /// The full descriptor (`views` and `actions` included).
    #[serde(serialize_with = "serialize_descriptor")]
    pub descriptor: Arc<ModuleDescriptor>,
}

fn serialize_descriptor<S: serde::Serializer>(
    d: &Arc<ModuleDescriptor>,
    s: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(d.as_ref(), s)
}

/// A module the runtime has loaded (name and exact version): what a host persists to
/// reload its modules at the next start (see [`ModuleRuntime::restore`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoadedModule {
    pub name: String,
    pub version: String,
}

/// A module `restore` could not reload. Reported, never fatal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RestoreFailure {
    pub name: String,
    pub version: String,
    /// `this.module.*` code of the refusal (e.g. `this.module.not_found` when the module
    /// disappeared from the store).
    pub code: String,
}

/// Outcome of [`ModuleRuntime::restore`].
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct RestoreReport {
    /// Reloaded from the store.
    pub restored: Vec<LoadedModule>,
    /// Already served at the requested version: nothing to do.
    pub already_active: Vec<LoadedModule>,
    /// Could not be reloaded (gone from the store, refused, dependency unsatisfied).
    pub missing: Vec<RestoreFailure>,
}

/// Owns a hot-loaded module between `init()` and the publication of the catalog.
///
/// # Why this exists
///
/// `stop()` is the compensation for an activation that never became visible. The
/// handle used to live only in the module's `Record`, and a refused load replaced
/// that `Record` with `lifecycle: None` — so the handle was dropped and nobody
/// could stop what had already started. `start()` may have spawned tasks, opened
/// connections or registered resources; a `replace` refused on a version check
/// left all of it running on code published nowhere.
///
/// Until [`ActivationGuard::disarm`] is called — and it is called only once the
/// catalog **and** the dispatch table are published — dropping the guard calls
/// `stop()`. That covers, with one mechanism:
/// - `init()` succeeded but `start()` failed;
/// - `activate` / `compose` refused the module;
/// - every early `return Err` in `replace_locked` (unknown module, not newer,
///   incompatible proto tags, an unsatisfied dependent).
///
/// The failure of `stop()` itself is logged, never propagated: it happens while
/// unwinding to an error the caller already has, and a compensation that panics
/// would replace a clean refusal with a crash.
struct ActivationGuard {
    name: String,
    module: Arc<dyn Module>,
    armed: bool,
}

impl ActivationGuard {
    fn new(name: String, module: Arc<dyn Module>) -> Self {
        Self {
            name,
            module,
            armed: false,
        }
    }

    /// Run `init()` then `start()`.
    ///
    /// The guard arms only once `init()` succeeded: stopping a module whose
    /// initialisation failed would ask it to release resources it never acquired.
    fn run(&mut self) -> Result<(), anyhow::Error> {
        self.module.init()?;
        self.armed = true;
        self.module.start()
    }

    /// The module is published and served: its handle now lives in the Record,
    /// which owns the `stop()` call from here on.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ActivationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(e) = self.module.stop() {
            tracing::warn!(
                module = %self.name,
                error = %e,
                "activation refused: the stop hook failed (ignored)"
            );
        } else {
            tracing::info!(
                module = %self.name,
                "activation refused after start: the module was stopped"
            );
        }
    }
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
                    lifecycle: None,
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

    /// The source offered under `name`, if any.
    pub(crate) fn offered(&self, name: &str) -> Option<ModuleSource> {
        self.lock().offers.get(name).cloned()
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

    /// Health of one module. `Ok(())` = healthy. A module with no lifecycle handle
    /// (built-in, declarative, or not loaded) reports healthy: nothing to probe.
    pub async fn health(&self, name: &str) -> Result<(), ContractError> {
        let lifecycle = {
            let inner = self.lock();
            let record = inner.records.get(name).ok_or_else(|| {
                ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", name)
            })?;
            if !matches!(record.state, ModuleState::Active | ModuleState::Draining) {
                return Err(
                    ContractError::framework(codes::MODULE_UNAVAILABLE).with_param("module", name)
                );
            }
            record.lifecycle.clone()
        };
        match lifecycle {
            Some(m) => m.health().map_err(|e| {
                ContractError::framework(codes::MODULE_UNAVAILABLE)
                    .with_param("module", name)
                    .with_param("reason", e.to_string())
            }),
            None => Ok(()),
        }
    }

    /// Every module the host currently SERVES (`Active` or `Draining`, built-in ones
    /// included), each with its full descriptor: entities, links, **views and actions**.
    /// Sorted by name. This is the integration point of the interface: the host reads it
    /// to draw what each module declares.
    pub async fn descriptors(&self) -> Vec<ServedDescriptor> {
        let inner = self.lock();
        inner
            .records
            .iter()
            .filter(|(_, r)| matches!(r.state, ModuleState::Active | ModuleState::Draining))
            .map(|(name, r)| Self::served_of(name, r))
            .collect()
    }

    /// The served descriptor of one module. `this.module.not_found` if unknown,
    /// `this.module.unavailable` if known but not served (offered, unloaded, failed).
    pub async fn descriptor(&self, name: &str) -> Result<ServedDescriptor, ContractError> {
        let inner = self.lock();
        let r = inner
            .records
            .get(name)
            .ok_or_else(|| Self::not_found(name))?;
        if !matches!(r.state, ModuleState::Active | ModuleState::Draining) {
            return Err(
                ContractError::framework(codes::MODULE_UNAVAILABLE).with_param("module", name)
            );
        }
        Ok(Self::served_of(name, r))
    }

    fn served_of(name: &str, r: &Record) -> ServedDescriptor {
        ServedDescriptor {
            name: name.to_string(),
            version: r.descriptor.version.clone(),
            state: r.state.clone(),
            builtin: r.builtin,
            descriptor: r.descriptor.clone(),
        }
    }

    fn not_found(name: &str) -> ContractError {
        ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", name)
    }

    /// The hot-loaded modules (name, exact version) currently served: the list a host
    /// persists to reload its modules at the next start. Built-in modules are not listed
    /// (the host rebuilds them).
    pub async fn loaded(&self) -> Vec<LoadedModule> {
        let inner = self.lock();
        inner
            .records
            .iter()
            .filter(|(_, r)| {
                !r.builtin && matches!(r.state, ModuleState::Active | ModuleState::Draining)
            })
            .map(|(name, r)| LoadedModule {
                name: name.clone(),
                version: r.descriptor.version.clone(),
            })
            .collect()
    }

    /// Reload `wanted` from the attached `ModuleStore` (restart path).
    ///
    /// A module gone from the store, refused by the checks, or whose dependency cannot be
    /// satisfied is REPORTED in [`RestoreReport::missing`] and logged: it never aborts the
    /// others nor the start of the host. Entries are tried in passes so that a module is
    /// loaded after the modules it requires, whatever the order of `wanted`; a pass that
    /// loads nothing ends the attempt (bounded: at most `wanted.len()` passes).
    pub async fn restore(&self, wanted: &[LoadedModule]) -> RestoreReport {
        let mut report = RestoreReport::default();
        let Some(store) = self.store.clone() else {
            for w in wanted {
                report.missing.push(RestoreFailure {
                    name: w.name.clone(),
                    version: w.version.clone(),
                    code: codes::MODULE_UNAVAILABLE.to_string(),
                });
            }
            return report;
        };
        let mut pending: Vec<&LoadedModule> = Vec::new();
        for w in wanted {
            let same = self.lock().records.get(&w.name).is_some_and(|r| {
                matches!(r.state, ModuleState::Active | ModuleState::Draining)
                    && r.descriptor.version == w.version
            });
            if same {
                report.already_active.push(w.clone());
            } else {
                pending.push(w);
            }
        }
        let mut last_code: HashMap<String, String> = HashMap::new();
        for _ in 0..wanted.len().max(1) {
            if pending.is_empty() {
                break;
            }
            let mut still = Vec::new();
            let mut progress = false;
            for w in pending {
                let source = ModuleSource::Stored {
                    store: store.clone(),
                    name: w.name.clone(),
                    version: Some(w.version.clone()),
                };
                match self.load(source).await {
                    Ok(_) => {
                        progress = true;
                        report.restored.push(w.clone());
                    }
                    Err(e) => {
                        last_code.insert(w.name.clone(), e.code.as_str().to_string());
                        let retry = e.code.as_str() == codes::MODULE_DEPENDENCY_MISSING
                            || e.code.as_str() == codes::MODULE_DEPENDENCY_VERSION;
                        if retry {
                            still.push(w);
                        } else {
                            report.missing.push(RestoreFailure {
                                name: w.name.clone(),
                                version: w.version.clone(),
                                code: e.code.as_str().to_string(),
                            });
                        }
                    }
                }
            }
            pending = still;
            if !progress {
                break;
            }
        }
        for w in pending {
            report.missing.push(RestoreFailure {
                name: w.name.clone(),
                version: w.version.clone(),
                code: last_code
                    .get(&w.name)
                    .cloned()
                    .unwrap_or_else(|| codes::MODULE_UNAVAILABLE.to_string()),
            });
        }
        for m in &report.missing {
            tracing::warn!(module = %m.name, version = %m.version, code = %m.code, "restore: module not reloaded");
        }
        report
    }

    /// Validate a load or replace WITHOUT publishing: check the descriptor, resolve it
    /// against the active modules, and (for a module already active) run proto-tag
    /// `check_compat`. Nothing is recorded, published or served -- a sandbox for a module
    /// change. The returned status is the one the module WOULD get (state `Resolved`).
    pub async fn dry_run(&self, source: ModuleSource) -> Result<ModuleStatus, ContractError> {
        let iface = self.loader.load(&source).await?;
        let d = iface.descriptor.clone();
        let inner = self.lock();
        let others = Self::active_descriptors(&inner, Some(&d.name));
        let refs: Vec<&ModuleDescriptor> = others.iter().map(|a| a.as_ref()).collect();
        crate::descriptor::resolve::resolve(&[d.as_ref()], &refs).map_err(one_error)?;
        if let Some(old) = inner.records.get(&d.name)
            && matches!(old.state, ModuleState::Active | ModuleState::Draining)
        {
            // Same rules as `replace`: forward-only, then proto-tag compatibility.
            if let (Some(new_v), Some(old_v)) = (d.semver(), old.descriptor.semver())
                && new_v <= old_v
            {
                return Err(ContractError::framework(codes::MODULE_VERSION_NOT_NEWER)
                    .with_param("module", d.name.clone())
                    .with_param("from", old.descriptor.version.clone())
                    .with_param("to", d.version.clone()));
            }
            let issues =
                crate::descriptor::compat::check_compat(old.descriptor.as_ref(), d.as_ref());
            if !issues.is_empty() {
                return Err(crate::descriptor::check::issues_to_error(&issues));
            }
        }
        Ok(ModuleStatus {
            name: d.name.clone(),
            version: d.version.clone(),
            state: ModuleState::Resolved,
            requires: d
                .requires
                .iter()
                .map(|q| format!("{} {}", q.name, q.version))
                .collect(),
            entities: d.entities.iter().map(|e| e.name.clone()).collect(),
            builtin: false,
        })
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

        // Descriptor-only validation, BEFORE any side effect. A module whose
        // `requires` cannot be satisfied must not be started only to be refused.
        // `compose` conflicts (duplicate plural, ...) still need the candidate in
        // place, so they stay inside `activate` — the guard covers that case.
        // The guard MUST be released before `refuse_load` runs: `self.inner` is a
        // `std::sync::Mutex`, which is not reentrant, and `refuse_load` takes it.
        // Calling it from inside this block deadlocked the whole host — caught by
        // the CI, whose test job hung instead of failing.
        let resolve_error = {
            let inner = self.lock();
            // The module itself is NOT excluded: loading an already-active module
            // must report `duplicate_module`, exactly as it did when `resolve` ran
            // inside `activate` on the full active set. Excluding it turned that
            // into an `illegal_transition` further down — a worse message for the
            // same situation.
            // Explicit transition check first: the refusal names BOTH versions and the
            // way out (`replace`), whichever version is requested.
            let live = inner
                .records
                .get(&name)
                .filter(|r| matches!(r.state, ModuleState::Active | ModuleState::Draining));
            if let Some(r) = live {
                Some(vec![
                    ContractError::framework(codes::MODULE_DUPLICATE_MODULE)
                        .with_param("module", name.clone())
                        .with_param("loaded_version", r.descriptor.version.clone())
                        .with_param("requested_version", iface.descriptor.version.clone())
                        .with_param(
                            "hint",
                            "use replace to move a loaded module to another version",
                        ),
                ])
            } else {
                let others = Self::active_descriptors(&inner, None);
                let refs: Vec<&ModuleDescriptor> = others.iter().map(|a| a.as_ref()).collect();
                crate::descriptor::resolve::resolve(&[iface.descriptor.as_ref()], &refs).err()
            }
        };
        if let Some(issues) = resolve_error {
            let err = one_error(issues);
            self.refuse_load(&name, &iface.descriptor, &err);
            return Err(err);
        }

        // Lifecycle of a hot-loaded compiled module: `init` then `start`, before it is
        // served. A failure refuses the load (the module is recorded `Failed`).
        let lifecycle = match &source {
            ModuleSource::Compiled(m) => Some(m.clone()),
            _ => None,
        };
        // From `init()` to the publication of the catalog, the guard owns the
        // module: anything that refuses the activation drops it, and dropping an
        // armed guard calls `stop()`.
        let mut guard = lifecycle
            .as_ref()
            .map(|m| ActivationGuard::new(name.clone(), m.clone()));
        if let Some(g) = guard.as_mut()
            && let Err(e) = g.run()
        {
            let err = ContractError::framework(codes::MODULE_UNAVAILABLE)
                .with_param("module", name.clone())
                .with_param("reason", e.to_string());
            self.refuse_load(&name, &iface.descriptor, &err);
            return Err(err);
        }
        let descriptor = iface.descriptor.clone();
        let mut inner = self.lock();
        match self.activate(&mut inner, iface, lifecycle) {
            Ok(status) => {
                // Published and served: the Record owns `stop()` from now on.
                if let Some(g) = guard.as_mut() {
                    g.disarm();
                }
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
                            lifecycle: None,
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

    /// Record a refused load: `Failed` unless the module is live (never overwrite
    /// an active record — a duplicate load of a live module must not erase it),
    /// then emit the failure event.
    ///
    /// One place, because the refusal paths multiplied: `init`/`start` failing,
    /// a dependency that cannot be satisfied, and the activation itself.
    fn refuse_load(&self, name: &str, descriptor: &Arc<ModuleDescriptor>, err: &ContractError) {
        {
            let mut inner = self.lock();
            let live = inner
                .records
                .get(name)
                .is_some_and(|r| matches!(r.state, ModuleState::Active | ModuleState::Draining));
            if !live {
                inner.records.insert(
                    name.to_string(),
                    Record {
                        descriptor: descriptor.clone(),
                        state: ModuleState::Failed {
                            code: err.code.clone(),
                            at: now(),
                        },
                        builtin: false,
                        iface: None,
                        gate: Arc::new(Gate::new()),
                        lifecycle: None,
                    },
                );
            }
        }
        self.emit(ModuleEvent::Failed {
            module: name.to_string(),
            code: err.code.as_str().to_string(),
        });
    }

    /// Message bundles of `source`: a compiled module's own bundles (the `Module::bundles`
    /// hook the host also reads at build time), or a stored module's bundles. Stored
    /// bundles are refused - nothing published - when one belongs to another module or
    /// would be refused by the catalog (`I18nCatalog::bundle_issues`).
    async fn bundles_of(
        source: &ModuleSource,
        module: &str,
    ) -> Result<Vec<crate::descriptor::MessageBundle>, ContractError> {
        if let ModuleSource::Compiled(m) = source {
            return Ok(m.bundles());
        }
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
        lifecycle: Option<Arc<dyn Module>>,
    ) -> Result<ModuleStatus, ContractError> {
        let d = iface.descriptor.clone();
        // No `resolve` here: `load` runs it before `init()`/`start()`, so a module
        // whose dependencies cannot be satisfied is never started. What is left
        // for this function is what needs the candidate in place: state
        // transitions and composition.

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
        let candidate = Record {
            descriptor: d.clone(),
            state: ModuleState::Active,
            builtin: false,
            iface: Some(iface),
            gate,
            lifecycle,
        };
        let previous = inner.records.remove(&d.name);
        inner.records.insert(d.name.clone(), candidate);
        let composed = self.compose(inner);
        let (catalog, table) = match composed {
            Ok(x) => x,
            Err(e) => {
                let candidate = inner.records.remove(&d.name);
                if let Some(p) = previous {
                    inner.records.insert(d.name.clone(), p);
                } else if let Some(mut c) = candidate {
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
        if let Some(c) = inner.records.get_mut(&d.name) {
            c.state = state;
        }

        let published = self.parts.catalog.publish(catalog);
        self.table.store(Arc::new(table));
        tracing::info!(module = %d.name, version = %d.version, generation = published.generation, "module loaded");
        self.emit(ModuleEvent::Loaded {
            module: d.name.clone(),
            version: d.version.clone(),
            generation: published.generation,
        });
        let r = inner
            .records
            .get(&d.name)
            .ok_or_else(|| Self::not_found(&d.name))?;
        Ok(Self::status_of(&d.name, r))
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
        // Cancel-safety: if the caller drops this future while the module drains (e.g.
        // the admin client disconnects), `DrainGuard` puts it back to `Active` instead of
        // leaving it stuck in `Draining` (503) with no legal transition out.
        let mut guard = DrainGuard::new(self, name);
        if !gate.wait_idle(self.options.drain_grace).await {
            tracing::warn!(module = %name, "drain grace elapsed: cancelling in-flight streams");
        }
        gate.cancel();
        guard.disarm();

        let mut inner = self.lock();
        let r = inner
            .records
            .get_mut(name)
            .ok_or_else(|| Self::not_found(name))?;
        r.state.transition(ModuleState::Unloaded)?;
        let saved_iface = r.iface.take();
        let (catalog, table) = match self.compose(&inner) {
            Ok(c) => c,
            Err(e) => {
                // Compose failed: the module must stay served, coherently. Back to `Active`
                // with its interface and a fresh gate (the old one was cancelled), and the
                // dispatch table recomposed WITHOUT the draining state.
                if let Some(r) = inner.records.get_mut(name) {
                    r.state = ModuleState::Active;
                    r.iface = saved_iface;
                    r.gate = Arc::new(Gate::new());
                }
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
        let r = inner
            .records
            .get(name)
            .ok_or_else(|| Self::not_found(name))?;
        let version = r.descriptor.version.clone();
        let lifecycle = r.lifecycle.clone();
        let status = Self::status_of(name, r);
        // Release the state lock before the `stop` hook (never hold the sync mutex across
        // an await).
        drop(inner);
        if let Some(m) = lifecycle
            && let Err(e) = m.stop()
        {
            tracing::warn!(module = %name, error = %e, "stop hook failed (ignored)");
        }
        tracing::info!(module = %name, generation = published.generation, "module unloaded");
        self.emit(ModuleEvent::Unloaded {
            module: name.to_string(),
            version,
            generation: published.generation,
        });
        Ok(status)
    }

    /// Revert a module left `Draining` by a cancelled `unload` back to `Active`
    /// (`Draining -> Active` is the documented "unload cancelled" transition), and
    /// republish the dispatch table without the draining set. No-op unless the module is
    /// exactly `Draining`.
    fn revert_drain(&self, name: &str) {
        let mut inner = self.lock();
        let Some(r) = inner.records.get_mut(name) else {
            return;
        };
        if r.state != ModuleState::Draining {
            return;
        }
        r.state = ModuleState::Active;
        r.gate = Arc::new(Gate::new());
        if let Ok((_, table)) = self.compose(&inner) {
            self.table.store(Arc::new(table));
        }
        tracing::warn!(module = %name, "unload cancelled mid-drain: module back to Active");
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
            let r = inner
                .records
                .get_mut(name)
                .ok_or_else(|| Self::not_found(name))?;
            r.state.transition(ModuleState::Draining)?;
            r.gate.clone()
        };
        match self.compose(&inner) {
            Ok((_, table)) => {
                self.table.store(Arc::new(table));
                Ok(gate)
            }
            Err(e) => {
                if let Some(r) = inner.records.get_mut(name) {
                    r.state = ModuleState::Active;
                }
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
        // Lifecycle of the NEW version: init then start, before the swap is published.
        let lifecycle = match &source {
            ModuleSource::Compiled(m) => Some(m.clone()),
            _ => None,
        };
        let d = iface.descriptor.clone();

        // Every validation of a replacement reads only the descriptor and the
        // published records: none needs the new version to have been started.
        // Running them first means a refused replacement never starts a version
        // that will not be served. `self.ops` is held for the whole operation, so
        // what is read here is still true after `start()`.
        {
            let inner = self.lock();
            let Some(old) = inner.records.get(&d.name) else {
                return Err(
                    ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", &d.name)
                );
            };
            if old.builtin || old.state != ModuleState::Active {
                return Err(ContractError::framework(codes::MODULE_UNAVAILABLE)
                    .with_param("module", &d.name));
            }
            // A replacement must move forward: no downgrade, no same version.
            if let (Some(new_v), Some(old_v)) = (d.semver(), old.descriptor.semver())
                && new_v <= old_v
            {
                return Err(ContractError::framework(codes::MODULE_VERSION_NOT_NEWER)
                    .with_param("module", d.name.clone())
                    .with_param("from", old.descriptor.version.clone())
                    .with_param("to", d.version.clone()));
            }
            // Proto tags: a tag published once is never moved nor reused.
            let issues =
                crate::descriptor::compat::check_compat(old.descriptor.as_ref(), d.as_ref());
            if !issues.is_empty() {
                return Err(crate::descriptor::check::issues_to_error(&issues));
            }
            // `resolve` checks the `requires` of EVERY module it sees, active ones
            // included: a dependent whose `VersionReq` rejects the new version is
            // reported as `this.module.dependency_version {module: <dependent>}`.
            let others = Self::active_descriptors(&inner, Some(&d.name));
            let refs: Vec<&ModuleDescriptor> = others.iter().map(|a| a.as_ref()).collect();
            crate::descriptor::resolve::resolve(&[d.as_ref()], &refs).map_err(one_error)?;
        }

        // The guard still matters: `compose` can refuse AFTER `start()`, and that
        // path needs the compensation.
        let mut guard = lifecycle
            .as_ref()
            .map(|m| ActivationGuard::new(iface.descriptor.name.clone(), m.clone()));
        if let Some(g) = guard.as_mut()
            && let Err(e) = g.run()
        {
            return Err(ContractError::framework(codes::MODULE_UNAVAILABLE)
                .with_param("module", iface.descriptor.name.clone())
                .with_param("reason", e.to_string()));
        }
        let mut inner = self.lock();
        // The validations above ran before `start()`; `self.ops` is still held, so
        // the record is the same one. Only the swap is left.
        let Some(old) = inner.records.remove(&d.name) else {
            return Err(Self::not_found(&d.name));
        };
        let from = old.descriptor.version.clone();
        let old_lifecycle = old.lifecycle.clone();
        inner.records.insert(
            d.name.clone(),
            Record {
                descriptor: d.clone(),
                state: ModuleState::Active,
                builtin: false,
                iface: Some(iface),
                gate: Arc::new(Gate::new()),
                lifecycle,
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
        // Published and served: the new version's Record owns `stop()` from now on.
        if let Some(g) = guard.as_mut() {
            g.disarm();
        }
        // `stop()` runs NOW, not after the drain: the drain grace defaults to 30
        // seconds (`DEFAULT_DRAIN_GRACE_MS`), and deferring a module's cleanup by
        // half a minute to let a few requests finish is the wrong trade. What the
        // drain protects is the old ROUTER and its streams, not the module's own
        // resources — so the contract on `Module::stop` says exactly that: do not
        // release what an in-flight request may still need.
        if let Some(m) = old_lifecycle
            && let Err(e) = m.stop()
        {
            tracing::warn!(module = %d.name, error = %e, "replace: old version stop hook failed (ignored)");
        }
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
        let r = inner
            .records
            .get(&d.name)
            .ok_or_else(|| Self::not_found(&d.name))?;
        Ok(Self::status_of(&d.name, r))
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
                entity_creators: crate::server::exposure::guard::publishing_creators(
                    &creators,
                    self.parts.event_bus.as_ref(),
                ),
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

/// Reverts a module to `Active` if an `unload` future is dropped while it drains (e.g.
/// the admin client disconnects). Armed the moment the module enters `Draining`, disarmed
/// once the unload completes. Without it, a cancelled unload leaves the module stuck in
/// `Draining` (503) with no legal transition out.
struct DrainGuard<'a> {
    rt: &'a ModuleRuntime,
    name: String,
    armed: bool,
}

impl<'a> DrainGuard<'a> {
    fn new(rt: &'a ModuleRuntime, name: &str) -> Self {
        Self {
            rt,
            name: name.to_string(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DrainGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.rt.revert_drain(&self.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::core::module::Module;
    use crate::descriptor::{
        EntityDesc, FieldDesc, FieldType, Locale, MessageBundle, MessageKey, ModuleDescriptor,
        Operation, PolicyExpr,
    };
    use crate::server::RestExposure;
    use crate::server::builder::ServerBuilder;
    use crate::server::entity_registry::EntityRegistry;
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

    struct Bundled;

    impl Module for Bundled {
        fn name(&self) -> &str {
            "bundled"
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
        fn register_entities(&self, _: &mut EntityRegistry) {}
        fn bundles(&self) -> Vec<MessageBundle> {
            let mut b = MessageBundle {
                module: "bundled".into(),
                locale: Locale::new("en"),
                ..Default::default()
            };
            b.messages
                .insert(MessageKey::new("hello"), "bonjour".into());
            vec![b]
        }
    }

    #[tokio::test]
    async fn compiled_module_bundles_are_returned_by_bundles_of() {
        let source = ModuleSource::Compiled(Arc::new(Bundled));
        let bundles = ModuleRuntime::bundles_of(&source, "bundled")
            .await
            .expect("bundles");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].module, "bundled");
        assert_eq!(bundles[0].get(&MessageKey::new("hello")), Some("bonjour"));
    }

    // ── lifecycle, atomicity, drain, restart, served descriptors ───────────────

    fn billing_v(version: &str) -> ModuleDescriptor {
        let mut d = billing();
        d.version = version.into();
        d
    }

    /// `billing` plus one declared view reading `invoice`.
    fn billing_with_view(version: &str) -> ModuleDescriptor {
        let mut d = billing_v(version);
        let view: crate::descriptor::ViewDesc = serde_json::from_value(serde_json::json!({
            "name": "invoices", "surface": "main", "title": format!("Invoices {version}"),
            "root": {"type": "leaf", "brick": {"type": "list", "entity": "invoice", "label": "amount"}}
        }))
        .expect("view json");
        d.views.push(view);
        d.normalize();
        d
    }

    fn manifest_of(d: &ModuleDescriptor) -> ModuleSource {
        ModuleSource::Manifest(d.to_json_pretty())
    }

    fn open_host(builder: ServerBuilder) -> (Arc<crate::server::host::ServerHost>, axum::Router) {
        let host = Arc::new(
            builder
                .with_link_service(InMemoryLinkService::new())
                .with_module_runtime_options(RuntimeOptions {
                    hot_load_enabled: true,
                    admin_policy: PolicyExpr::Public,
                    drain_grace: Duration::from_millis(2000),
                    ..Default::default()
                })
                .without_authorization()
                .build_host()
                .expect("host"),
        );
        let app = RestExposure::build_router(host.clone(), vec![]).expect("router");
        (host, app)
    }

    async fn call_json(
        app: &axum::Router,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .expect("request");
        let res = app.clone().oneshot(req).await.expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, v)
    }

    #[tokio::test]
    async fn served_descriptors_carry_views_and_follow_load_replace_unload() {
        let (host, _app) = open_host(ServerBuilder::new());
        let rt = host.module_runtime.clone().expect("runtime");
        assert!(rt.descriptors().await.is_empty());

        rt.load(manifest_of(&billing_with_view("1.0.0")))
            .await
            .expect("load");
        let served = rt.descriptors().await;
        assert_eq!(served.len(), 1);
        assert_eq!(served[0].version, "1.0.0");
        assert_eq!(served[0].descriptor.views.len(), 1, "views are served");
        assert_eq!(
            served[0].descriptor.views[0].title.as_deref(),
            Some("Invoices 1.0.0")
        );

        rt.replace(manifest_of(&billing_with_view("1.1.0")))
            .await
            .expect("replace");
        let d = rt.descriptor("billing").await.expect("served");
        assert_eq!(
            (d.version.as_str(), d.descriptor.views[0].title.as_deref()),
            ("1.1.0", Some("Invoices 1.1.0")),
            "the replaced module shows its NEW views"
        );

        rt.unload("billing").await.expect("unload");
        assert!(rt.descriptors().await.is_empty());
        assert_eq!(
            rt.descriptor("billing").await.unwrap_err().code.as_str(),
            "this.module.unavailable"
        );
        assert_eq!(
            rt.descriptor("ghost").await.unwrap_err().code.as_str(),
            "this.module.not_found"
        );
    }

    #[tokio::test]
    async fn admin_serves_descriptors_dry_run_replace_and_health() {
        let (host, app) = open_host(ServerBuilder::new());
        let rt = host.module_runtime.clone().expect("runtime");
        let v1 = serde_json::to_value(billing_with_view("1.0.0")).expect("json");
        let v2 = serde_json::to_value(billing_with_view("1.1.0")).expect("json");

        let (s, _) = call_json(
            &app,
            Method::POST,
            "/admin/modules",
            Some(serde_json::json!({"manifest": v1})),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);

        let (s, v) = call_json(&app, Method::GET, "/admin/modules/_descriptors", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v[0]["name"], "billing");
        assert_eq!(v[0]["descriptor"]["views"][0]["name"], "invoices");
        assert_eq!(v[0]["state"], "active");

        let (s, v) = call_json(
            &app,
            Method::GET,
            "/admin/modules/_descriptors/billing",
            None,
        )
        .await;
        assert_eq!((s, v["version"].clone()), (StatusCode::OK, "1.0.0".into()));
        let (s, v) = call_json(&app, Method::GET, "/admin/modules/_descriptors/ghost", None).await;
        assert_eq!(
            (s, v["code"].clone()),
            (StatusCode::NOT_FOUND, "this.module.not_found".into())
        );

        let (s, _) = call_json(&app, Method::GET, "/admin/modules/_health/billing", None).await;
        assert_eq!(s, StatusCode::OK);

        // dry_run publishes nothing.
        let (s, v) = call_json(
            &app,
            Method::POST,
            "/admin/modules/_dry_run",
            Some(serde_json::json!({"manifest": v2.clone()})),
        )
        .await;
        assert_eq!((s, v["state"].clone()), (StatusCode::OK, "resolved".into()));
        assert_eq!(rt.descriptor("billing").await.expect("d").version, "1.0.0");

        // PUT names another module than the source: refused, nothing changes.
        let (s, v) = call_json(
            &app,
            Method::PUT,
            "/admin/modules/crm",
            Some(serde_json::json!({"manifest": v2.clone()})),
        )
        .await;
        assert_eq!(
            (s, v["code"].clone()),
            (StatusCode::BAD_REQUEST, "this.transport.bad_request".into())
        );
        assert_eq!(rt.descriptor("billing").await.expect("d").version, "1.0.0");

        let (s, v) = call_json(
            &app,
            Method::PUT,
            "/admin/modules/billing",
            Some(serde_json::json!({"manifest": v2})),
        )
        .await;
        assert_eq!((s, v["version"].clone()), (StatusCode::OK, "1.1.0".into()));
        assert_eq!(rt.descriptor("billing").await.expect("d").version, "1.1.0");

        // An unknown module id is not a module name.
        let (s, v) = call_json(&app, Method::GET, "/admin/modules/_health/ghost", None).await;
        assert_eq!(
            (s, v["code"].clone()),
            (StatusCode::NOT_FOUND, "this.module.not_found".into())
        );
    }

    #[tokio::test]
    async fn admin_closed_by_default_says_why() {
        // Default options: admin_only, hot loading closed.
        let host = Arc::new(
            ServerBuilder::new()
                .with_link_service(InMemoryLinkService::new())
                .with_module_runtime()
                .without_authorization()
                .build_host()
                .expect("host"),
        );
        let app = RestExposure::build_router(host, vec![]).expect("router");
        let body = serde_json::json!({"manifest": serde_json::to_value(billing()).expect("json")});
        let (s, v) = call_json(
            &app,
            Method::PUT,
            "/admin/modules/billing",
            Some(body.clone()),
        )
        .await;
        // Anonymous caller: refused by the admin policy, and the body says which one.
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
        assert!(
            v.to_string().contains("admin_only") || v.to_string().contains("AdminOnly"),
            "refusal explains the policy: {v}"
        );
    }

    #[tokio::test]
    async fn hot_load_closed_refuses_replace_and_dry_run_with_the_operation() {
        let host = Arc::new(
            ServerBuilder::new()
                .with_link_service(InMemoryLinkService::new())
                .with_module_runtime_options(RuntimeOptions {
                    admin_policy: PolicyExpr::Public,
                    ..Default::default()
                })
                .without_authorization()
                .build_host()
                .expect("host"),
        );
        let app = RestExposure::build_router(host, vec![]).expect("router");
        let body = serde_json::json!({"manifest": serde_json::to_value(billing()).expect("json")});
        for (m, uri, op) in [
            (Method::PUT, "/admin/modules/billing", "replace"),
            (Method::POST, "/admin/modules/_dry_run", "dry_run"),
        ] {
            let (s, v) = call_json(&app, m, uri, Some(body.clone())).await;
            assert_eq!(v["code"], "this.module.hot_load_disabled", "{op}: {v}");
            assert!(v.to_string().contains(op), "names the operation: {v}");
            assert_ne!(s, StatusCode::OK);
        }
        // Reading stays possible.
        let (s, _) = call_json(&app, Method::GET, "/admin/modules/_descriptors", None).await;
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_lifecycle_transitions_are_refused_with_named_codes() {
        let (host, _app) = open_host(ServerBuilder::new());
        let rt = host.module_runtime.clone().expect("runtime");
        let code = |e: ContractError| e.code.as_str().to_string();

        // Nothing loaded: unload / replace / health on an absent module.
        assert_eq!(
            code(rt.unload("billing").await.unwrap_err()),
            "this.module.not_found"
        );
        assert_eq!(
            code(
                rt.replace(manifest_of(&billing_v("2.0.0")))
                    .await
                    .unwrap_err()
            ),
            "this.module.not_found",
            "replace of a module never loaded"
        );
        assert_eq!(
            code(rt.health("billing").await.unwrap_err()),
            "this.module.not_found"
        );

        rt.load(manifest_of(&billing_v("1.0.0")))
            .await
            .expect("load");
        // Double load, same and another version: refused, the loaded one is untouched.
        let e = rt.load(manifest_of(&billing_v("1.0.0"))).await.unwrap_err();
        assert_eq!(e.code.as_str(), "this.module.duplicate_module");
        let e = rt.load(manifest_of(&billing_v("2.0.0"))).await.unwrap_err();
        assert_eq!(e.code.as_str(), "this.module.duplicate_module");
        assert!(
            format!("{:?}", e.params).contains("loaded_version"),
            "{:?}",
            e.params
        );
        let st = rt.status("billing").await.expect("status");
        assert_eq!(
            (st.state, st.version.as_str()),
            (ModuleState::Active, "1.0.0")
        );

        rt.unload("billing").await.expect("unload");
        // Unload twice, replace and health of an unloaded module.
        assert_eq!(
            code(rt.unload("billing").await.unwrap_err()),
            "this.module.illegal_transition"
        );
        assert_eq!(
            code(
                rt.replace(manifest_of(&billing_v("2.0.0")))
                    .await
                    .unwrap_err()
            ),
            "this.module.unavailable"
        );
        assert_eq!(
            code(rt.health("billing").await.unwrap_err()),
            "this.module.unavailable"
        );
        // And it can be loaded again (Unloaded -> Discovered -> ... -> Active).
        rt.load(manifest_of(&billing_v("2.0.0")))
            .await
            .expect("reload");
    }

    #[tokio::test]
    async fn failed_load_after_partial_composition_leaves_the_host_unchanged() {
        let (host, app) = open_host(ServerBuilder::new());
        let rt = host.module_runtime.clone().expect("runtime");
        rt.load(manifest_of(&billing_v("1.0.0")))
            .await
            .expect("billing");
        let before = host.catalog.load().generation;

        // `shop` declares two entities; the 2nd one repeats billing's plural. Whichever
        // check catches it, no entity of `shop` may stay registered.
        let mut shop = ModuleDescriptor::new("shop", "1.0.0");
        let mut order = EntityDesc::new("order");
        order.fields.push(FieldDesc::new("amount", FieldType::F64));
        let mut dup = EntityDesc::new("bill");
        dup.plural = "invoices".into();
        for e in [&mut order, &mut dup] {
            for op in Operation::CRUD {
                e.permissions.ops.insert(op, PolicyExpr::Authenticated);
            }
        }
        shop.entities.push(order);
        shop.entities.push(dup);
        let err = rt.load(manifest_of(&shop)).await.unwrap_err();
        assert!(
            err.code.as_str().starts_with("this.module."),
            "{}",
            err.code.as_str()
        );

        assert_eq!(host.catalog.load().generation, before, "nothing published");
        assert!(host.catalog.load().entity_of_plural("orders").is_none());
        assert!(
            get(&app, "/orders").await.is_client_error(),
            "never registered: not served"
        );
        assert!(rt.descriptors().await.iter().all(|d| d.name != "shop"));
        assert_eq!(
            get(&app, "/invoices").await,
            StatusCode::OK,
            "billing still served"
        );
    }

    #[tokio::test]
    async fn load_failing_in_composition_registers_nothing_and_can_be_retried() {
        let (host, app) = open_host(ServerBuilder::new());
        let rt = host.module_runtime.clone().expect("runtime");
        let before = host.catalog.load().generation;

        // The 1st compose of the load fails: after the candidate was put in place.
        rt.fail_compose_at
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let err = rt.load(manifest_of(&billing_v("1.0.0"))).await.unwrap_err();
        assert_eq!(err.code.as_str(), "this.internal");
        assert_eq!(host.catalog.load().generation, before);
        assert!(host.catalog.load().entity_of_plural("invoices").is_none());
        assert!(get(&app, "/invoices").await.is_client_error(), "not served");
        assert!(rt.descriptors().await.is_empty());
        assert!(rt.loaded().await.is_empty());
        let st = rt.status("billing").await.expect("recorded");
        assert!(
            matches!(st.state, ModuleState::Failed { .. }),
            "{:?}",
            st.state
        );

        // Failed -> Discovered -> ... -> Active.
        rt.load(manifest_of(&billing_v("1.0.0")))
            .await
            .expect("retry");
        assert_eq!(get(&app, "/invoices").await, StatusCode::OK);
    }

    /// Loader of a module whose `/invoices` handler waits for `release`.
    struct SlowLoader {
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl ModuleLoader for SlowLoader {
        async fn load(&self, _: &ModuleSource) -> Result<ModuleInterface, ContractError> {
            let release = self.release.clone();
            let routes = axum::Router::new().route(
                "/invoices",
                axum::routing::get(move || {
                    let release = release.clone();
                    async move {
                        release.notified().await;
                        "done"
                    }
                }),
            );
            Ok(ModuleInterface {
                descriptor: Arc::new(billing()),
                fetchers: Default::default(),
                creators: Default::default(),
                routes: Some(routes),
                links_config: LinksConfig {
                    entities: vec![],
                    links: vec![],
                    validation_rules: None,
                    events: None,
                    sinks: None,
                    auth: None,
                },
            })
        }
    }

    #[tokio::test]
    async fn in_flight_request_finishes_within_grace_before_unload_completes() {
        let release = Arc::new(tokio::sync::Notify::new());
        let (host, app) = open_host(ServerBuilder::new().with_module_loader(Arc::new(
            SlowLoader {
                release: release.clone(),
            },
        )));
        let rt = host.module_runtime.clone().expect("runtime");
        rt.load(ModuleSource::Manifest(String::new()))
            .await
            .expect("load");

        let in_flight = {
            let app = app.clone();
            tokio::spawn(async move { call_json(&app, Method::GET, "/invoices", None).await })
        };
        tokio::time::sleep(Duration::from_millis(60)).await;
        let unloading = {
            let rt = rt.clone();
            tokio::spawn(async move { rt.unload("billing").await })
        };
        tokio::time::sleep(Duration::from_millis(60)).await;
        // Draining: new requests are refused cleanly, the module is not gone yet.
        let (s, v) = call_json(&app, Method::GET, "/invoices", None).await;
        assert_eq!(
            (s, v["code"].clone()),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "this.module.unavailable".into()
            )
        );
        assert!(
            !unloading.is_finished(),
            "unload waits for the in-flight request"
        );
        assert_eq!(
            rt.status("billing").await.expect("s").state,
            ModuleState::Draining
        );

        let started = std::time::Instant::now();
        release.notify_waiters();
        let st = tokio::time::timeout(Duration::from_millis(1500), unloading)
            .await
            .expect("unload ends well before the 2 s grace")
            .expect("join")
            .expect("unload");
        assert_eq!(st.state, ModuleState::Unloaded);
        assert!(started.elapsed() < Duration::from_millis(1500));
        // The request that was in flight was served to the end, not cut.
        let (s, v) = in_flight.await.expect("join");
        assert_eq!(
            (s, v),
            (StatusCode::OK, serde_json::Value::String("done".into()))
        );
        assert_eq!(get(&app, "/invoices").await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn restore_reloads_from_store_in_dependency_order_and_reports_missing() {
        use crate::storage::vfs::{InMemoryVfs, ModuleStore, VfsModuleStore};
        let store: Arc<dyn ModuleStore> =
            Arc::new(VfsModuleStore::new(Arc::new(InMemoryVfs::new())));
        let mut crm = ModuleDescriptor::new("crm", "1.0.0");
        crm.requires.push(crate::descriptor::ModuleReq {
            name: "billing".into(),
            version: "^1".into(),
            optional: false,
        });
        let mut customer = EntityDesc::new("customer");
        customer
            .fields
            .push(FieldDesc::new("name", FieldType::String));
        for op in Operation::CRUD {
            customer
                .permissions
                .ops
                .insert(op, PolicyExpr::Authenticated);
        }
        crm.entities.push(customer);
        crm.normalize();
        store
            .put(&billing_v("1.0.0"), &[])
            .await
            .expect("put billing");
        store.put(&crm, &[]).await.expect("put crm");

        let (host, app) = open_host(ServerBuilder::new().with_module_store(store));
        let rt = host.module_runtime.clone().expect("runtime");
        let wanted = vec![
            LoadedModule {
                name: "crm".into(),
                version: "1.0.0".into(),
            },
            LoadedModule {
                name: "ghost".into(),
                version: "1.0.0".into(),
            },
            LoadedModule {
                name: "billing".into(),
                version: "1.0.0".into(),
            },
        ];
        let report = rt.restore(&wanted).await;
        assert_eq!(report.restored.len(), 2, "{report:?}");
        assert_eq!(report.missing.len(), 1, "{report:?}");
        assert_eq!(report.missing[0].name, "ghost");
        assert!(report.missing[0].code.starts_with("this."), "{report:?}");
        assert_eq!(get(&app, "/invoices").await, StatusCode::OK);
        assert_eq!(
            rt.loaded().await,
            vec![
                LoadedModule {
                    name: "billing".into(),
                    version: "1.0.0".into()
                },
                LoadedModule {
                    name: "crm".into(),
                    version: "1.0.0".into()
                },
            ]
        );
        // A second restore is a no-op: everything is already served.
        let again = rt.restore(&wanted[..1]).await;
        assert_eq!((again.restored.len(), again.already_active.len()), (0, 1));
    }

    #[tokio::test]
    async fn restore_without_store_reports_every_module_and_does_not_panic() {
        let (host, _app) = open_host(ServerBuilder::new());
        let rt = host.module_runtime.clone().expect("runtime");
        let report = rt
            .restore(&[LoadedModule {
                name: "billing".into(),
                version: "1.0.0".into(),
            }])
            .await;
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.missing[0].code, "this.module.unavailable");
    }
}
