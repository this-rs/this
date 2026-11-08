//! `WasmModuleLoader`: run a REAL module compiled to wasm and publish it as a normal module.
//!
//! The contract is `docs/modules-wasm-abi.md` (ABI v1): the artifact `program.wasm` exports
//! `memory`, `alloc`, `manifest` and `handle`, takes **no import**, speaks JSON through
//! `(ptr << 32) | len` pairs, and reaches the world only through a closed set of effects.
//!
//! # How a compiled module becomes a normal one
//!
//! 1. the descriptor is read from the [`ModuleStore`] and the code through
//!    [`ModuleStore::code`] (integrity-checked against the manifest);
//! 2. the guest's `manifest()` is run (bounded) and must answer a `ModuleDescriptor`;
//! 3. that descriptor passes the **same** `check(Stored)` as a stored module — a module with
//!    code gets no privilege and cannot widen its own scope;
//! 4. its name and version must be the ones stored (one truth about identity, like
//!    `compiled()` in `loader.rs`);
//! 5. one [`JsonEntityService`] per entity, each with a [`WasmLogic`]: the routes REST /
//!    GraphQL of the module are the ordinary ones, there is no parallel path.
//!
//! # Bounds
//!
//! Every call into the guest is bounded by [`Limits`] (fuel, memory, request and response
//! size) and runs on the blocking pool: a guest that loops is stopped by its fuel, the
//! runtime stays usable, and the next call starts from a fresh instance.

use super::json_entity::JsonEntityService;
use super::loader::{
    ModuleInterface, ModuleLoader, ModuleSource, build_services, check_descriptor,
};
use super::logic::{Effect, EffectSink, EntityLogic, LogLevel, LogicOutcome};
use super::rows::{MemoryRowStores, RowStoreFactory};
use crate::core::events::{EntityEvent, EventBus, FrameworkEvent};
use crate::descriptor::framework::codes;
use crate::descriptor::{
    CheckProfile, ContractError, CustomRegistry, ErrorCode, ErrorKind, FieldViolation,
    ModuleDescriptor,
};
use crate::storage::vfs::ModuleStore;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};
use this_module_harness::call::{Args, CallError, Limits, call_json};
use uuid::Uuid;

/// Rows handed to the guest per call (ABI §8); the rest is cut and `ctx.truncated` says so.
pub const MAX_ROWS: usize = 500;

/// Loads `ModuleSource::Wasm { location }` (`"<name>@<version>"`, or `"<name>"` = latest).
#[derive(Clone)]
pub struct WasmModuleLoader {
    store: Arc<dyn ModuleStore>,
    limits: Limits,
    max_rows: usize,
    custom: CustomRegistry,
    allow_public: bool,
    event_bus: Option<Arc<EventBus>>,
    row_stores: Arc<dyn RowStoreFactory>,
}

impl WasmModuleLoader {
    /// Bounds of the ABI (`Limits::module_abi()`): 50 M fuel, 4 MiB, 1 MiB in, 4 MiB out.
    pub fn new(store: Arc<dyn ModuleStore>) -> Self {
        Self {
            store,
            limits: Limits::module_abi(),
            max_rows: MAX_ROWS,
            custom: CustomRegistry::default(),
            allow_public: false,
            event_bus: None,
            row_stores: Arc::new(MemoryRowStores),
        }
    }

    /// Per-call bounds. Zero fuel or zero memory is refused at call time, never "unbounded".
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_max_rows(mut self, max_rows: usize) -> Self {
        self.max_rows = max_rows;
        self
    }

    pub fn with_custom_registry(mut self, custom: CustomRegistry) -> Self {
        self.custom = custom;
        self
    }

    pub fn allow_public(mut self, allow: bool) -> Self {
        self.allow_public = allow;
        self
    }

    pub fn with_event_bus(mut self, bus: Option<Arc<EventBus>>) -> Self {
        self.event_bus = bus;
        self
    }

    pub fn with_row_stores(mut self, stores: Arc<dyn RowStoreFactory>) -> Self {
        self.row_stores = stores;
        self
    }
}

fn parse_location(location: &str) -> Result<(&str, Option<&str>), ContractError> {
    let bad = || {
        ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED).with_param(
            "what",
            format!("wasm location `{location}`: expected `<name>@<version>` or `<name>`"),
        )
    };
    match location.split_once('@') {
        Some((name, version)) if !name.is_empty() && !version.is_empty() => {
            Ok((name, Some(version)))
        }
        Some(_) => Err(bad()),
        None if !location.is_empty() => Ok((location, None)),
        None => Err(bad()),
    }
}

#[async_trait]
impl ModuleLoader for WasmModuleLoader {
    async fn load(&self, source: &ModuleSource) -> Result<ModuleInterface, ContractError> {
        let location = match source {
            ModuleSource::Wasm { location } => location,
            ModuleSource::Plugin { kind, location } if kind == "wasm" => location,
            other => {
                return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                    .with_param("what", format!("the wasm loader does not load {other:?}")));
            }
        };
        let (name, version) = parse_location(location)?;

        // What the store says this module is (identity) and the code it carries.
        let stored = self.store.get(name, version).await?;
        let wasm = self
            .store
            .code(&stored.name, &stored.version)
            .await?
            .ok_or_else(|| {
                ContractError::framework(codes::MODULE_NOT_FOUND)
                    .with_param("module", format!("{}@{}", stored.name, stored.version))
                    .with_param("reason", "the version carries no program.wasm")
            })?;
        let guest = Guest {
            wasm: Arc::from(wasm),
            limits: self.limits.clone(),
            module: stored.name.clone(),
        };

        // What the guest says it is. Untrusted until it passes the same check.
        let manifest = guest.call("manifest", Vec::new()).await?;
        let text = std::str::from_utf8(&manifest).map_err(|e| {
            ContractError::framework(codes::DESCRIPTOR_PARSE)
                .with_param("reason", format!("manifest() is not UTF-8: {e}"))
        })?;
        let declared = ModuleDescriptor::from_json(text)?;
        let descriptor = check_descriptor(
            &self.custom,
            declared,
            CheckProfile::Stored {
                allow_public: self.allow_public,
            },
        )?;

        // One truth about name and version: the store's.
        let mut mismatches = Vec::new();
        if descriptor.name != stored.name {
            mismatches.push(format!(
                "name `{}` (manifest()) vs `{}` (stored)",
                descriptor.name, stored.name
            ));
        }
        if descriptor.version != stored.version {
            mismatches.push(format!(
                "version `{}` (manifest()) vs `{}` (stored)",
                descriptor.version, stored.version
            ));
        }
        if !mismatches.is_empty() {
            return Err(ContractError::framework(codes::MODULE_IDENTITY_MISMATCH)
                .with_param("module", &stored.name)
                .with_param("what", mismatches.join("; ")));
        }

        let declared_errors: Arc<BTreeMap<String, crate::descriptor::ErrorDesc>> = Arc::new(
            descriptor
                .errors
                .iter()
                .map(|e| (e.code.as_str().to_string(), e.clone()))
                .collect(),
        );
        let sink = Arc::new(ModuleEffects {
            module: descriptor.name.clone(),
            entities: OnceLock::new(),
            bus: self.event_bus.clone(),
        });
        let max_rows = self.max_rows;
        let (interface, services) = build_services(
            descriptor,
            &self.custom,
            &self.event_bus,
            self.row_stores.as_ref(),
            |entity| {
                let logic: Arc<dyn EntityLogic> = Arc::new(WasmLogic {
                    guest: guest.clone(),
                    entity: entity.name.clone(),
                    max_rows,
                    errors: declared_errors.clone(),
                });
                let sink: Arc<dyn EffectSink> = sink.clone();
                Some((logic, sink))
            },
        )?;
        // The sink needs the services, which needed the sink: closed once, here.
        let _ = sink.entities.set(services);
        Ok(interface)
    }
}

/// The compiled artifact and its bounds. Cheap to clone (the bytes are shared).
#[derive(Clone)]
struct Guest {
    wasm: Arc<[u8]>,
    limits: Limits,
    module: String,
}

impl Guest {
    /// One bounded call on the blocking pool, on a fresh instance. A refusal of the engine
    /// (imports, fuel, memory, bad buffer…) becomes a `ContractError`, never a panic.
    async fn call(&self, export: &str, buffers: Vec<Vec<u8>>) -> Result<Vec<u8>, ContractError> {
        let wasm = self.wasm.clone();
        let limits = self.limits.clone();
        let export_name = export.to_string();
        let joined = tokio::task::spawn_blocking(move || {
            let refs: Vec<&[u8]> = buffers.iter().map(Vec::as_slice).collect();
            call_json(&wasm, &export_name, &Args { buffers: refs }, &limits)
        })
        .await;
        match joined {
            Ok(Ok(call)) => Ok(call.response),
            Ok(Err(e)) => Err(self.refused(export, &e)),
            Err(e) => Err(self.refused_text(export, &format!("blocking task failed: {e}"))),
        }
    }

    fn refused(&self, export: &str, error: &CallError) -> ContractError {
        self.refused_text(export, &error.to_string())
    }

    fn refused_text(&self, export: &str, reason: &str) -> ContractError {
        ContractError::framework(codes::MODULE_UNAVAILABLE)
            .with_param("module", &self.module)
            .with_param("reason", format!("{export}: {reason}"))
    }
}

/// What `handle` answers.
#[derive(Deserialize)]
struct GuestResponse {
    ok: bool,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    effects: Vec<Effect>,
    #[serde(default)]
    error: Option<GuestError>,
}

#[derive(Deserialize)]
struct GuestError {
    code: String,
    message: String,
    #[serde(default)]
    field: Option<String>,
}

/// The business logic of one entity, served by the guest's `handle`.
struct WasmLogic {
    guest: Guest,
    entity: String,
    max_rows: usize,
    errors: Arc<BTreeMap<String, crate::descriptor::ErrorDesc>>,
}

impl WasmLogic {
    async fn handle(
        &self,
        op: &str,
        name: Option<&str>,
        id: Option<&str>,
        input: &Value,
        rows: &[Value],
    ) -> Result<LogicOutcome, ContractError> {
        let truncated = rows.len() > self.max_rows;
        let rows = &rows[..rows.len().min(self.max_rows)];
        let mut request = json!({
            "op": op,
            "entity": self.entity,
            "input": input,
            "rows": rows,
            "ctx": {
                "now_ms": chrono::Utc::now().timestamp_millis(),
                "tenant": Value::Null,
                "account": Value::Null,
                "locale": "fr",
                "config": {},
                "truncated": truncated,
            },
        });
        if let Some(name) = name {
            request["name"] = json!(name);
        }
        if let Some(id) = id {
            request["id"] = json!(id);
        }
        let request_bytes = serde_json::to_vec(&request).map_err(|e| {
            self.guest
                .refused_text("handle", &format!("request not serializable: {e}"))
        })?;
        let response = self
            .guest
            .call("handle", vec![op.as_bytes().to_vec(), request_bytes])
            .await?;
        // An unknown effect `kind` fails HERE, for the whole response: nothing is applied.
        let parsed: GuestResponse = serde_json::from_slice(&response).map_err(|e| {
            self.guest
                .refused_text("handle", &format!("response refused by the host: {e}"))
        })?;
        if !parsed.ok {
            return Err(self.guest_error(parsed.error));
        }
        Ok(LogicOutcome {
            output: parsed.output.filter(|v| !v.is_null()),
            effects: parsed.effects,
        })
    }

    /// The module's refusal, as a contract error: kind from its declaration when it declared
    /// the code (`errors`), validation otherwise; the message is the module's own.
    fn guest_error(&self, error: Option<GuestError>) -> ContractError {
        let Some(error) = error else {
            return self
                .guest
                .refused_text("handle", "`ok: false` without an `error`");
        };
        let mut out = match self.errors.get(&error.code) {
            Some(desc) => ContractError::from_desc(desc),
            None => ContractError::new(ErrorCode::new(error.code.clone()), ErrorKind::Validation),
        }
        .with_message(error.message.clone());
        if let Some(field) = error.field {
            let mut violation = FieldViolation::new(field, ErrorCode::new(error.code));
            violation.message = error.message;
            out = out.with_field(violation);
        }
        out
    }
}

#[async_trait]
impl EntityLogic for WasmLogic {
    async fn create(&self, input: &Value) -> Result<LogicOutcome, ContractError> {
        self.handle("create", None, None, input, &[]).await
    }

    async fn update(
        &self,
        id: &str,
        patch: &Value,
        current: &Value,
    ) -> Result<LogicOutcome, ContractError> {
        self.handle(
            "update",
            None,
            Some(id),
            patch,
            std::slice::from_ref(current),
        )
        .await
    }

    async fn delete(&self, id: &str, current: &Value) -> Result<LogicOutcome, ContractError> {
        self.handle(
            "delete",
            None,
            Some(id),
            &json!({ "id": id }),
            std::slice::from_ref(current),
        )
        .await
    }

    async fn get(&self, row: &Value) -> Result<LogicOutcome, ContractError> {
        // La ligne demandée est dans `input` ET dans `rows` : un module qui cherche par
        // identifiant (`req.row(id)`) et un module qui lit `input` doivent tous deux
        // trouver la ligne. Sans ça, un `get` rendait « introuvable » une ligne qui existe
        // — mesuré sur le premier module Rust réel.
        self.handle("get", None, None, row, std::slice::from_ref(row))
            .await
    }

    async fn list(&self, rows: &[Value]) -> Result<LogicOutcome, ContractError> {
        self.handle("list", None, None, &Value::Null, rows).await
    }

    async fn call(
        &self,
        name: &str,
        input: &Value,
        rows: &[Value],
    ) -> Result<Option<LogicOutcome>, ContractError> {
        self.handle("call", Some(name), None, input, rows)
            .await
            .map(Some)
    }
}

/// Applies the effects of a module on its own entities, the bus and the journal.
struct ModuleEffects {
    module: String,
    entities: OnceLock<HashMap<String, Arc<JsonEntityService>>>,
    bus: Option<Arc<EventBus>>,
}

impl ModuleEffects {
    fn entity(&self, kind: &str, name: &str) -> Result<&Arc<JsonEntityService>, ContractError> {
        self.entities
            .get()
            .and_then(|m| m.get(name))
            .ok_or_else(|| {
                refuse(
                    &self.module,
                    format!("effect {kind}: unknown entity `{name}`"),
                )
            })
    }
}

fn refuse(module: &str, reason: String) -> ContractError {
    ContractError::framework(codes::VALIDATION_FAILED)
        .with_param("module", module)
        .with_param("reason", reason)
}

fn parse_id(module: &str, id: &str) -> Result<Uuid, ContractError> {
    Uuid::parse_str(id)
        .map_err(|e| refuse(module, format!("effect delete: invalid id `{id}`: {e}")))
}

#[async_trait]
impl EffectSink for ModuleEffects {
    async fn check(&self, effects: &[Effect]) -> Result<(), ContractError> {
        for effect in effects {
            match effect {
                Effect::Put { entity, body } => {
                    if !body.is_object() {
                        return Err(refuse(
                            &self.module,
                            "effect put: `body` is not an object".into(),
                        ));
                    }
                    self.entity("put", entity)?.precheck_put(body)?;
                }
                Effect::Delete { entity, id } => {
                    self.entity("delete", entity)?;
                    parse_id(&self.module, id)?;
                }
                Effect::Emit { topic, .. } if topic.trim().is_empty() => {
                    return Err(refuse(&self.module, "effect emit: empty `topic`".into()));
                }
                Effect::Emit { .. } | Effect::Log { .. } => {}
            }
        }
        Ok(())
    }

    async fn apply(&self, effects: Vec<Effect>) -> Result<(), ContractError> {
        // Whole list validated first; then applied in order.
        self.check(&effects).await?;
        for effect in effects {
            match effect {
                Effect::Put { entity, body } => {
                    self.entity("put", &entity)?
                        .put_effect(body)
                        .await
                        .map_err(into_contract)?;
                }
                Effect::Delete { entity, id } => {
                    let id = parse_id(&self.module, &id)?;
                    self.entity("delete", &entity)?
                        .delete_effect(&id)
                        .await
                        .map_err(into_contract)?;
                }
                Effect::Emit { topic, payload } => {
                    // The bus has no free-form variant: a topic travels as an `EntityEvent`
                    // whose `entity_type` is the topic (id from `payload.id`, else nil).
                    if let Some(bus) = &self.bus {
                        let entity_id = payload
                            .get("id")
                            .and_then(Value::as_str)
                            .and_then(|s| Uuid::parse_str(s).ok())
                            .unwrap_or_else(Uuid::nil);
                        bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
                            entity_type: topic,
                            entity_id,
                            data: payload,
                        }));
                    }
                }
                Effect::Log { level, message } => match level {
                    LogLevel::Info => tracing::info!(module = %self.module, "{message}"),
                    LogLevel::Warn => tracing::warn!(module = %self.module, "{message}"),
                    LogLevel::Error => tracing::error!(module = %self.module, "{message}"),
                },
            }
        }
        Ok(())
    }
}

fn into_contract(error: anyhow::Error) -> ContractError {
    match error.downcast::<ContractError>() {
        Ok(contract) => contract,
        Err(other) => {
            ContractError::framework(codes::INTERNAL).with_param("reason", other.to_string())
        }
    }
}
