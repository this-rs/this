//! # `hot_module` — a declarative module loaded from a VFS into a *live* host
//!
//! `warehouse` is declared **once**, as a [`ModuleDescriptor`] (see [`warehouse()`]). That
//! descriptor is stored in a [`ModuleStore`] over a VFS — by default a `LocalFsVfs`, i.e.
//! real files in a temporary directory (pass `--memory` for an `InMemoryVfs`); both
//! backends implement the same `Vfs` trait and the same on-store layout.
//!
//! The host is then built **without** the module, and the module is loaded at runtime:
//! `ModuleRuntime` reads it back from the VFS, checks it (`CheckProfile::Stored`), composes
//! the next catalog and publishes it. From that instant the *same* descriptor drives:
//!
//! - **REST**: `/parcels`, `/couriers`, with its validators, its input transforms, its typed
//!   query capabilities (`?filter=&sort=`) and the link route derived from its `LinkDesc`;
//! - **GraphQL**: `type Parcel` / `type Courier`, their queries and mutations, read from the
//!   LIVE catalog — the SDL changes on load, not on rebuild;
//! - **gRPC**: the same entities through `EntityService`, with the same contract codes;
//! - **policies**: one evaluator on the three surfaces (creating a parcel is admin-only,
//!   reading only needs an identity) — no per-surface wiring;
//! - **i18n**: the module bundles (fr/en) enter the host catalog on load.
//!
//! Unloading (`DELETE /admin/modules/warehouse`, or `ModuleRuntime::unload`) removes the
//! routes, the entities, the GraphQL types, the proto message and the bundles: every one of
//! those surfaces then answers `404` / `this.module.not_found` (a tombstone), never a stale
//! route and never an empty 200.
//!
//! ## Run it
//!
//! ```text
//! cargo run --example hot_module --features graphql,grpc
//! ```
//!
//! Options: `--memory` (VFS in RAM instead of a temp directory) and `--serve [addr]`
//! (load the module, then serve REST + GraphQL on `127.0.0.1:3000` until Ctrl-C, so the
//! same routes can be queried with curl).
//!
//! ## Test it
//!
//! ```text
//! cargo test --example hot_module --features graphql,grpc
//! ```
//!
//! - `hot_module_loaded_from_vfs_is_served_then_404_after_unload` — the whole lifecycle on
//!   REST + GraphQL + gRPC, with the module policies armed;
//! - `hot_module_on_disk_vfs_writes_the_documented_layout` — the same lifecycle over
//!   `LocalFsVfs`, checking the files the store writes;
//! - `hot_module_admin_interface_loads_and_unloads_from_the_store` — load/unload driven by
//!   the admin interface, including its `admin_only` policy.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use this::core::auth::{AuthContext, AuthProvider};
use this::core::errors::grpc::contract_error_of;
use this::core::events::{FrameworkEvent, ModuleEvent};
use this::descriptor::{
    Applies, EntityDesc, FieldDesc, FieldType, FilterOp, LinkDesc, Locale, MessageBundle,
    MessageKey, MessageSyntax, ModuleDescriptor, Operation, PolicyExpr, TransformSpec,
    ValidatorSpec,
};
use this::server::builder::ServerBuilder;
use this::server::exposure::GraphQLExposure;
use this::server::exposure::grpc::entity_service::EntityServiceImpl;
use this::server::exposure::grpc::proto::entity_service_server::EntityService;
use this::server::exposure::grpc::proto::{CreateEntityRequest, ListEntitiesRequest};
use this::server::exposure::grpc::proto_generator::ProtoGenerator;
use this::server::exposure::rest::RestExposure;
use this::server::host::ServerHost;
use this::server::module_runtime::ModuleRuntime;
use this::storage::InMemoryLinkService;
use this::storage::vfs::{InMemoryVfs, LocalFsVfs, ModuleStore, Vfs, VfsModuleStore};

// ── the descriptor: the ONLY declaration of the module ──────────────────────

/// `warehouse` 1.0.0, declared once. Every surface below derives from this value; there is
/// no route table, no SDL, no proto and no policy map written by hand anywhere in this file.
pub fn warehouse() -> ModuleDescriptor {
    let mut parcel = EntityDesc::new("parcel");
    parcel.fields = vec![
        FieldDesc::new("reference", FieldType::String)
            // Input transforms run before validation: `  p-1 ` is stored as `P-1`.
            .transform(TransformSpec::Trim, Applies::Always)
            .transform(TransformSpec::Uppercase, Applies::Always)
            .validator(
                ValidatorSpec::StringLength { min: 3, max: 20 },
                Applies::Always,
            )
            // Typed query capabilities: `?filter={"reference":"P-1"}` and `?sort=reference`
            // are accepted, anything else is refused with a stable code.
            .filterable(&[FilterOp::Eq])
            .sortable(),
        FieldDesc::new("weight", FieldType::F64)
            .validator(ValidatorSpec::Positive, Applies::Always)
            .filterable(&[FilterOp::Gt, FilterOp::Eq])
            .sortable(),
        // Declared, optional, not filterable and not sortable.
        FieldDesc::new("note", FieldType::String).optional(),
    ];
    for op in Operation::CRUD {
        parcel.permissions.ops.insert(op, PolicyExpr::Authenticated);
    }
    // Creating a parcel is `AdminOnly`; reading only needs an identity.
    parcel
        .permissions
        .ops
        .insert(Operation::Create, PolicyExpr::AdminOnly);

    let mut courier = EntityDesc::new("courier");
    courier.fields = vec![
        FieldDesc::new("name", FieldType::String)
            .validator(
                ValidatorSpec::StringLength { min: 2, max: 40 },
                Applies::Always,
            )
            .sortable(),
    ];
    for op in Operation::CRUD {
        courier
            .permissions
            .ops
            .insert(op, PolicyExpr::Authenticated);
    }

    let mut m = ModuleDescriptor::new("warehouse", "1.0.0");
    m.entities.push(parcel);
    m.entities.push(courier);
    // The link route `/parcels/{id}/couriers-delivers/{id}` is DERIVED from this line.
    m.links.push(LinkDesc::new("deliver", "parcel", "courier"));
    m.i18n.locales = vec![Locale::new("fr"), Locale::new("en")];
    m.normalize();
    m
}

/// The module's own texts, fr and en. Framework messages (validation, `this.module.*`)
/// come from the framework catalog.
pub fn bundles() -> Vec<MessageBundle> {
    [("fr", "Colis"), ("en", "Parcel")]
        .into_iter()
        .map(|(locale, text)| {
            let mut messages = BTreeMap::new();
            messages.insert(MessageKey::new("warehouse.parcel.title"), text.to_string());
            MessageBundle {
                module: "warehouse".into(),
                locale: Locale::new(locale),
                syntax: MessageSyntax::Simple,
                messages,
            }
        })
        .collect()
}

/// `Authorization: Bearer admin` → admin; `Bearer user` → an identity without roles.
struct Tokens;

#[async_trait]
impl AuthProvider for Tokens {
    async fn extract_context(&self, h: &HeaderMap) -> anyhow::Result<AuthContext> {
        Ok(match h.get("authorization").and_then(|v| v.to_str().ok()) {
            Some("Bearer admin") => AuthContext::Admin {
                admin_id: uuid::Uuid::from_u128(9),
            },
            Some("Bearer user") => AuthContext::User {
                user_id: uuid::Uuid::from_u128(1),
                tenant_id: uuid::Uuid::from_u128(2),
                roles: vec![],
                scopes: Vec::new(),
            },
            _ => AuthContext::Anonymous,
        })
    }
    async fn is_owner(&self, _: &uuid::Uuid, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

// ── the host: built WITHOUT the module ──────────────────────────────────────

/// A running host (REST + GraphQL routers, one shared catalog) over a VFS-backed store.
pub struct Demo {
    pub host: Arc<ServerHost>,
    pub rest: Router,
    pub graphql: Router,
    pub store: Arc<dyn ModuleStore>,
    /// Root directory when the store sits on the local filesystem (`None` in memory).
    pub root: Option<PathBuf>,
}

/// Build the host and the store over `vfs`. Panics only if the host cannot be built at
/// all: no module is registered here, so nothing can be incoherent yet.
pub fn demo(vfs: Arc<dyn Vfs>, root: Option<PathBuf>) -> Demo {
    let store: Arc<dyn ModuleStore> = Arc::new(VfsModuleStore::new(vfs));
    let host = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        // Module events (`Loaded` / `Unloaded`) are what the demo prints.
        .with_event_bus(256)
        // Runtime loading exists… but stays CLOSED until explicitly opened (decision:
        // a host that can load code at runtime says so).
        .with_module_runtime()
        .with_hot_load_enabled(true)
        .with_module_store(store.clone())
        .build_host()
        .expect("host builds")
        .with_auth_provider(Arc::new(Tokens));
    let host = Arc::new(host);
    Demo {
        rest: RestExposure::build_router(host.clone(), vec![]).expect("REST exposure"),
        graphql: GraphQLExposure::build_router(host.clone()).expect("GraphQL exposure"),
        host,
        store,
        root,
    }
}

/// Put the module in the VFS. From here on the store holds it; the host knows nothing.
pub async fn seed(d: &Demo) -> anyhow::Result<()> {
    d.store.put(&warehouse(), &bundles()).await?;
    Ok(())
}

impl Demo {
    pub fn runtime(&self) -> &Arc<ModuleRuntime> {
        self.host
            .module_runtime
            .as_ref()
            .expect("the host was built with_module_runtime()")
    }

    /// One REST call, through the real `RestExposure` router (auth, policy, validation and
    /// localization layers included).
    pub async fn call_rest(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        lang: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("accept-language", lang);
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .expect("request");
        let res = self.rest.clone().oneshot(req).await.expect("response");
        reply_json(res).await
    }

    /// One GraphQL call. The schema is rebuilt from the live catalog on every request.
    pub async fn gql(&self, query: &str, token: &str, lang: &str) -> Value {
        let res = self
            .graphql
            .clone()
            .oneshot(
                Request::post("/graphql")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .header("accept-language", lang)
                    .body(Body::from(json!({ "query": query }).to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        reply_json(res).await.1
    }

    /// The SDL of the moment: `type Parcel` appears with the load, disappears with the unload.
    pub async fn sdl(&self) -> String {
        let res = self
            .graphql
            .clone()
            .oneshot(
                Request::get("/graphql/schema")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).unwrap_or_default()
    }

    /// The proto the host exports for gRPC codegen at this instant.
    pub async fn proto(&self) -> String {
        ProtoGenerator::new(self.host.clone())
            .generate_proto()
            .await
    }

    fn grpc_request<T>(&self, token: &str, lang: &str, msg: T) -> tonic::Request<T> {
        let mut req = tonic::Request::new(msg);
        let md = req.metadata_mut();
        md.insert("authorization", format!("Bearer {token}").parse().unwrap());
        md.insert("accept-language", lang.parse().unwrap());
        req
    }

    /// `EntityService/CreateEntity`: the same entity, the same contract codes as REST.
    pub async fn grpc_create(
        &self,
        entity: &str,
        data: prost_types::Struct,
        token: &str,
    ) -> Result<(), tonic::Status> {
        let req = self.grpc_request(
            token,
            "en",
            CreateEntityRequest {
                entity_type: entity.into(),
                data: Some(data),
            },
        );
        EntityServiceImpl::new(self.host.clone())
            .create_entity(req)
            .await
            .map(|_| ())
    }

    /// `EntityService/ListEntities`, returning the row count.
    pub async fn grpc_list(&self, entity: &str, token: &str) -> Result<usize, tonic::Status> {
        let req = self.grpc_request(
            token,
            "en",
            ListEntitiesRequest {
                entity_type: entity.into(),
                limit: 50,
                offset: 0,
            },
        );
        EntityServiceImpl::new(self.host.clone())
            .list_entities(req)
            .await
            .map(|r| r.into_inner().entities.len())
    }
}

async fn reply_json(res: axum::response::Response) -> (StatusCode, Value) {
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string())),
    )
}

fn code(v: &Value) -> &str {
    v["code"].as_str().unwrap_or("")
}

fn gql_code(v: &Value) -> &str {
    v["errors"][0]["extensions"]["this_code"]
        .as_str()
        .unwrap_or("")
}

fn field_codes(v: &Value) -> Vec<String> {
    v["fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f["code"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn grpc_parcel(reference: &str, weight: f64) -> prost_types::Struct {
    use prost_types::value::Kind;
    let v = |kind| prost_types::Value { kind: Some(kind) };
    prost_types::Struct {
        fields: [
            (
                "reference".to_string(),
                v(Kind::StringValue(reference.into())),
            ),
            ("weight".to_string(), v(Kind::NumberValue(weight))),
        ]
        .into(),
    }
}

fn module_events(
    rx: &mut tokio::sync::broadcast::Receiver<this::core::events::EventEnvelope>,
) -> Vec<FrameworkEvent> {
    let mut out = vec![];
    while let Ok(e) = rx.try_recv() {
        out.push(e.event);
    }
    out
}

/// Directory under the system temp dir, emptied first (best effort).
fn scratch_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("this-hot-module-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

// ── the walkthrough `cargo run --example hot_module` prints ─────────────────

async fn walkthrough(d: &Demo) -> anyhow::Result<()> {
    let en = Locale::new("en");
    let title = MessageKey::new("warehouse.parcel.title");
    let mut rx = d.host.event_bus().expect("event bus").subscribe();

    println!("hot_module — one ModuleDescriptor, stored in a VFS, loaded into a live host");
    println!(
        "           module `warehouse` 1.0.0: 2 entities, 1 link, 3 validators, 2 transforms,"
    );
    println!(
        "           2 locales, 1 admin-only operation — all of it declared in warehouse() and nowhere else"
    );

    println!("\n── 1. the VFS store ─────────────────────────────────────────────────");
    println!("   store.list() = {:?}", d.store.list().await?);
    if let Some(root) = &d.root {
        println!("   files on disk under {}:", root.display());
        for rel in [
            "modules/warehouse/1.0.0/descriptor.json",
            "modules/warehouse/1.0.0/i18n/fr.json",
            "modules/warehouse/1.0.0/i18n/en.json",
            "modules/warehouse/1.0.0/manifest.json",
        ] {
            let ok = root.join(rel).is_file();
            println!("     {} {rel}", if ok { "written " } else { "missing " });
        }
    }

    println!("\n── 2. before the load: nothing of `warehouse` exists ────────────────");
    let (s, v) = d
        .call_rest(Method::GET, "/parcels", "user", "en", None)
        .await;
    println!(
        "   REST    GET  /parcels          -> {s} {} (no /parcels route: fail-closed)",
        code(&v)
    );
    println!(
        "   GraphQL SDL has `type Parcel`  -> {}",
        d.sdl().await.contains("type Parcel")
    );
    let g = d.gql("{ parcels { id } }", "admin", "en").await;
    println!(
        "   GraphQL query {{ parcels }}      -> error {}",
        g["errors"][0]["message"].as_str().unwrap_or("")
    );
    let st = d.grpc_list("parcel", "admin").await.expect_err("unknown");
    println!("   gRPC    ListEntities(parcel)   -> {:?}", st.code());

    println!("\n── 3. the admin interface, before the load ──────────────────────────");
    let (s, list) = d
        .call_rest(Method::GET, "/admin/modules", "admin", "en", None)
        .await;
    println!("   GET     /admin/modules         -> {s} {list}");

    println!("\n── 4. hot load from the VFS, through the admin interface ────────────");
    let (s, status) = d
        .call_rest(
            Method::POST,
            "/admin/modules",
            "admin",
            "en",
            Some(json!({ "store": { "name": "warehouse" } })),
        )
        .await;
    println!(
        "   POST    /admin/modules {{\"store\":{{\"name\":\"warehouse\"}}}} -> {s} state={} entities={}",
        status["state"], status["entities"]
    );
    let st = d.runtime().status("warehouse").await?;
    println!(
        "   ModuleRuntime::status -> {:?} v{} requires={:?}",
        st.state, st.version, st.requires
    );
    for e in module_events(&mut rx) {
        if let FrameworkEvent::Module(ModuleEvent::Loaded {
            module, generation, ..
        }) = e
        {
            println!("   event   Module::Loaded {{ module: {module}, generation: {generation} }}");
        }
    }

    println!("\n── 5. served: REST (validators, transforms, typed query) ────────────");
    let bad = json!({ "reference": "ab", "weight": -1 });
    let (s, v) = d
        .call_rest(Method::POST, "/parcels", "admin", "en", Some(bad))
        .await;
    println!(
        "   POST    /parcels (invalid)     -> {s} {} fields={:?}",
        code(&v),
        field_codes(&v)
    );
    let (s, v) = d
        .call_rest(
            Method::POST,
            "/parcels",
            "user",
            "en",
            Some(json!({ "reference": "p-9", "weight": 1 })),
        )
        .await;
    println!(
        "   POST    /parcels (user)        -> {s} {} (create is admin-only)",
        code(&v)
    );
    let (s, created) = d
        .call_rest(
            Method::POST,
            "/parcels",
            "admin",
            "en",
            Some(json!({ "reference": "  p-1 ", "weight": 4.5 })),
        )
        .await;
    println!(
        "   POST    /parcels (admin)       -> {s} reference={} (trimmed + uppercased)",
        created["reference"]
    );
    let parcel_id = created["id"].as_str().unwrap_or_default().to_string();
    d.call_rest(
        Method::POST,
        "/parcels",
        "admin",
        "en",
        Some(json!({ "reference": "p-2", "weight": 50 })),
    )
    .await;
    let (s, list) = d
        .call_rest(
            Method::GET,
            "/parcels?filter=%7B%22weight%3E%22%3A6%7D&sort=-weight",
            "user",
            "en",
            None,
        )
        .await;
    println!(
        "   GET     /parcels?filter={{\"weight>\":6}}&sort=-weight -> {s} total={} first={}",
        list["pagination"]["total"], list["data"][0]["reference"]
    );
    let (s, v) = d
        .call_rest(Method::GET, "/parcels?sort=note", "user", "en", None)
        .await;
    println!(
        "   GET     /parcels?sort=note     -> {s} {} (the descriptor does not make `note` sortable)",
        code(&v)
    );

    println!("\n── 6. served: GraphQL (schema derived from the LIVE catalog) ────────");
    let sdl = d.sdl().await;
    println!(
        "   SDL     `type Parcel` = {}   `type Courier` = {}",
        sdl.contains("type Parcel"),
        sdl.contains("type Courier")
    );
    println!(
        "   proto   `message Parcel` = {} (exported for gRPC codegen)",
        d.proto().await.contains("message Parcel")
    );
    let g = d
        .gql(
            r#"mutation { createParcel(data: {reference: "P-3", weight: 7.0}) { id } }"#,
            "admin",
            "en",
        )
        .await;
    println!(
        "   mutation createParcel (admin)  -> {}",
        if g["errors"].is_null() {
            "ok".to_string()
        } else {
            gql_code(&g).to_string()
        }
    );
    let g = d
        .gql("{ parcels { reference weight } }", "user", "en")
        .await;
    println!(
        "   query    parcels (user)        -> {} row(s)",
        g["data"]["parcels"].as_array().map(Vec::len).unwrap_or(0)
    );

    println!("\n── 7. served: gRPC (`EntityService`) ────────────────────────────────");
    let r = d
        .grpc_create("parcel", grpc_parcel("P-4", 9.0), "admin")
        .await;
    println!(
        "   CreateEntity(parcel, admin)    -> {}",
        if r.is_ok() { "ok" } else { "refused" }
    );
    let st = d
        .grpc_create("parcel", grpc_parcel("P-5", 9.0), "user")
        .await
        .expect_err("refused");
    println!(
        "   CreateEntity(parcel, user)     -> {:?} {}",
        st.code(),
        contract_error_of(&st)
            .map(|e| e.code.as_str().to_string())
            .unwrap_or_default()
    );
    println!(
        "   ListEntities(parcel, user)     -> {:?} row(s)",
        d.grpc_list("parcel", "user").await
    );

    println!("\n── 8. the link route, derived from `LinkDesc` ───────────────────────");
    let (_, courier) = d
        .call_rest(
            Method::POST,
            "/couriers",
            "admin",
            "en",
            Some(json!({ "name": "Dora" })),
        )
        .await;
    let courier_id = courier["id"].as_str().unwrap_or_default().to_string();
    let (s, _) = d
        .call_rest(
            Method::POST,
            &format!("/parcels/{parcel_id}/couriers-delivers/{courier_id}"),
            "admin",
            "en",
            Some(json!({})),
        )
        .await;
    println!("   POST    /parcels/{{id}}/couriers-delivers/{{id}} -> {s}");
    let (s, links) = d
        .call_rest(
            Method::GET,
            &format!("/parcels/{parcel_id}/couriers-delivers"),
            "user",
            "en",
            None,
        )
        .await;
    println!(
        "   GET     /parcels/{{id}}/couriers-delivers      -> {s} courier linked = {}",
        links.to_string().contains(&courier_id)
    );

    println!("\n── 9. the module bundles, in the host i18n catalog ──────────────────");
    let cat = d.host.locales.load();
    println!(
        "   `warehouse.parcel.title` -> fr={:?} en={:?}",
        cat.lookup(&Locale::new("fr"), &title),
        cat.lookup(&Locale::new("en"), &title)
    );
    drop(cat);

    println!("\n── 10. unload: a tombstone on every surface ─────────────────────────");
    let (s, status) = d
        .call_rest(
            Method::DELETE,
            "/admin/modules/warehouse",
            "admin",
            "en",
            None,
        )
        .await;
    println!(
        "   DELETE  /admin/modules/warehouse -> {s} state={}",
        status["state"]
    );
    for e in module_events(&mut rx) {
        if let FrameworkEvent::Module(ModuleEvent::Unloaded {
            module, generation, ..
        }) = e
        {
            println!(
                "   event   Module::Unloaded {{ module: {module}, generation: {generation} }}"
            );
        }
    }
    let (s, v) = d
        .call_rest(Method::GET, "/parcels", "user", "en", None)
        .await;
    println!("   REST    GET  /parcels          -> {s} {}", code(&v));
    let (s, v) = d
        .call_rest(
            Method::GET,
            &format!("/parcels/{parcel_id}"),
            "user",
            "en",
            None,
        )
        .await;
    println!("   REST    GET  /parcels/{{id}}     -> {s} {}", code(&v));
    println!(
        "   GraphQL SDL has `type Parcel`  -> {}",
        d.sdl().await.contains("type Parcel")
    );
    println!(
        "   proto   `message Parcel`       -> {}",
        d.proto().await.contains("message Parcel")
    );
    let g = d.gql("{ parcels { id } }", "admin", "en").await;
    println!(
        "   GraphQL query {{ parcels }}      -> error {}",
        g["errors"][0]["message"].as_str().unwrap_or("")
    );
    let st = d.grpc_list("parcel", "admin").await.expect_err("unknown");
    println!("   gRPC    ListEntities(parcel)   -> {:?}", st.code());
    println!(
        "   `warehouse.parcel.title` -> {:?} (bundle removed with the module)",
        d.host.locales.load().lookup(&en, &title)
    );
    Ok(())
}

/// `--serve`: load the module, then hand the very same routers to a real HTTP server.
async fn serve(d: &Demo, addr: &str) -> anyhow::Result<()> {
    let (s, status) = d
        .call_rest(
            Method::POST,
            "/admin/modules",
            "admin",
            "en",
            Some(json!({ "store": { "name": "warehouse" } })),
        )
        .await;
    println!("\nloaded for serving: {s} state={}", status["state"]);
    let app = Router::new().merge(d.rest.clone()).merge(d.graphql.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    println!("serving on http://{local}");
    println!(
        "  REST     GET  /parcels   POST /parcels   GET  /admin/modules   DELETE /admin/modules/warehouse"
    );
    println!("  GraphQL  POST /graphql   GET  /graphql/schema   GET /graphql/playground");
    println!("  auth     Authorization: Bearer admin | Bearer user");
    println!("Ctrl-C to stop.");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let memory = args.iter().any(|a| a == "--memory");
    let serve_at = args.iter().position(|a| a == "--serve").map(|i| {
        args.get(i + 1)
            .filter(|a| !a.starts_with("--"))
            .cloned()
            .unwrap_or_else(|| "127.0.0.1:3000".to_string())
    });

    let (vfs, root): (Arc<dyn Vfs>, Option<PathBuf>) = if memory {
        (Arc::new(InMemoryVfs::new()), None)
    } else {
        let root = scratch_dir("demo");
        (Arc::new(LocalFsVfs::open(&root)?), Some(root))
    };
    let d = demo(vfs, root);
    seed(&d).await?;

    match serve_at {
        Some(addr) => serve(&d, &addr).await,
        None => {
            walkthrough(&d).await?;
            println!(
                "\nDone. `cargo test --example hot_module --features graphql,grpc` asserts every line above."
            );
            Ok(())
        }
    }
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use this::descriptor::ModuleState;
    use this::server::module_runtime::ModuleSource;

    /// The whole lifecycle on the three surfaces, with the module policies ARMED:
    /// load from the VFS → served (REST/GraphQL/gRPC) → unload → tombstone everywhere.
    #[tokio::test]
    async fn hot_module_loaded_from_vfs_is_served_then_404_after_unload() {
        let d = demo(Arc::new(InMemoryVfs::new()), None);
        seed(&d).await.expect("stored in the VFS");
        let en = Locale::new("en");
        let title = MessageKey::new("warehouse.parcel.title");
        let mut rx = d.host.event_bus().expect("event bus").subscribe();

        // ── nothing is served before the load ──────────────────────────────
        assert!(
            !d.sdl().await.contains("type Parcel"),
            "SDL before the load"
        );
        let (s, _) = d
            .call_rest(Method::GET, "/parcels", "user", "en", None)
            .await;
        assert!(s.is_client_error(), "REST before the load: {s}");
        assert_eq!(
            d.grpc_list("parcel", "admin").await.map_err(|s| s.code()),
            Err(tonic::Code::NotFound),
            "gRPC before the load"
        );
        assert!(d.host.locales.load().lookup(&en, &title).is_none());

        // ── the load itself reads the module back from the VFS ─────────────
        let status = d
            .runtime()
            .load(ModuleSource::Stored {
                store: d.store.clone(),
                name: "warehouse".into(),
                version: None,
            })
            .await
            .expect("load from the VFS");
        assert_eq!(status.state, ModuleState::Active);
        assert_eq!(status.entities, vec!["parcel", "courier"]);
        assert!(status.requires.is_empty());

        // ── REST: validators, transforms, policies, typed query ────────────
        let (s, created) = d
            .call_rest(
                Method::POST,
                "/parcels",
                "admin",
                "en",
                Some(json!({ "reference": "  p-1 ", "weight": 4.5 })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{created}");
        assert_eq!(created["reference"], "P-1", "the declared transforms ran");
        let id = created["id"].as_str().expect("id").to_string();
        let (s, got) = d
            .call_rest(Method::GET, &format!("/parcels/{id}"), "user", "en", None)
            .await;
        assert_eq!((s, got["weight"].clone()), (StatusCode::OK, json!(4.5)));

        let (s, refused) = d
            .call_rest(
                Method::POST,
                "/parcels",
                "admin",
                "en",
                Some(json!({ "reference": "ab", "weight": -1 })),
            )
            .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        assert_eq!(code(&refused), "this.validation.failed");
        let codes = field_codes(&refused);
        assert!(
            codes.contains(&"this.validation.string_length".to_string()),
            "{codes:?}"
        );
        assert!(
            codes.contains(&"this.validation.positive".to_string()),
            "{codes:?}"
        );

        let (s, forbidden) = d
            .call_rest(
                Method::POST,
                "/parcels",
                "user",
                "en",
                Some(json!({ "reference": "p-2", "weight": 1 })),
            )
            .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{forbidden}");
        assert_eq!(code(&forbidden), "this.auth.forbidden");

        let (s, list) = d
            .call_rest(
                Method::GET,
                "/parcels?filter=%7B%22weight%3E%22%3A2%7D&sort=-weight",
                "user",
                "en",
                None,
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{list}");
        assert_eq!(list["pagination"]["total"], json!(1), "{list}");

        // ── GraphQL: schema and resolvers from the LIVE catalog ────────────
        let sdl = d.sdl().await;
        assert!(
            sdl.contains("type Parcel") && sdl.contains("type Courier"),
            "{sdl}"
        );
        let g = d
            .gql(
                r#"mutation { createParcel(data: {reference: "P-3", weight: 7.0}) { id } }"#,
                "admin",
                "en",
            )
            .await;
        assert!(g["errors"].is_null(), "{g}");
        let g = d.gql("{ parcels { reference } }", "user", "en").await;
        assert_eq!(
            g["data"]["parcels"].as_array().map(Vec::len),
            Some(2),
            "{g}"
        );
        let g = d
            .gql(
                r#"mutation { createParcel(data: {reference: "P-4", weight: 7.0}) { id } }"#,
                "user",
                "en",
            )
            .await;
        assert_eq!(gql_code(&g), "this.auth.forbidden", "{g}");

        // ── gRPC: same entities, same contract codes ───────────────────────
        d.grpc_create("parcel", grpc_parcel("P-5", 3.0), "admin")
            .await
            .expect("gRPC create");
        assert_eq!(d.grpc_list("parcel", "user").await.expect("gRPC list"), 3);
        let st = d
            .grpc_create("parcel", grpc_parcel("P-6", 3.0), "user")
            .await
            .expect_err("refused");
        assert_eq!(st.code(), tonic::Code::PermissionDenied);
        assert_eq!(
            contract_error_of(&st).map(|e| e.code.as_str().to_string()),
            Some("this.auth.forbidden".to_string())
        );

        // ── the module bundle entered the host catalog ─────────────────────
        assert_eq!(d.host.locales.load().lookup(&en, &title), Some("Parcel"));
        let seen = module_events(&mut rx);
        assert!(
            seen.iter().any(|e| matches!(
                e,
                FrameworkEvent::Module(ModuleEvent::Loaded { module, .. }) if module == "warehouse"
            )),
            "{seen:?}"
        );

        // ── unload: nothing of the module survives on any surface ──────────
        let status = d.runtime().unload("warehouse").await.expect("unload");
        assert_eq!(status.state, ModuleState::Unloaded);
        for uri in ["/parcels".to_string(), format!("/parcels/{id}")] {
            let (s, v) = d.call_rest(Method::GET, &uri, "user", "en", None).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{uri}: {v}");
            assert_eq!(code(&v), "this.module.not_found", "{uri}");
        }
        assert!(
            !d.sdl().await.contains("type Parcel"),
            "SDL after the unload"
        );
        assert!(
            !d.proto().await.contains("message Parcel"),
            "proto after the unload"
        );
        assert!(
            d.gql("{ parcels { id } }", "admin", "en").await["errors"].is_array(),
            "GraphQL after the unload"
        );
        assert_eq!(
            d.grpc_list("parcel", "admin").await.map_err(|s| s.code()),
            Err(tonic::Code::NotFound),
            "gRPC after the unload"
        );
        assert!(
            d.host.locales.load().lookup(&en, &title).is_none(),
            "bundles are removed with the module"
        );
        let seen = module_events(&mut rx);
        assert!(
            seen.iter().any(|e| matches!(
                e,
                FrameworkEvent::Module(ModuleEvent::Unloaded { module, .. }) if module == "warehouse"
            )),
            "{seen:?}"
        );
    }

    /// The same lifecycle over `LocalFsVfs`: the store writes the documented layout, and the
    /// module served from disk behaves exactly like the in-memory one.
    #[tokio::test]
    async fn hot_module_on_disk_vfs_writes_the_documented_layout() {
        let root = scratch_dir("layout");
        let vfs = LocalFsVfs::open(&root).expect("local fs vfs");
        let d = demo(Arc::new(vfs), Some(root.clone()));
        seed(&d).await.expect("put on disk");

        for rel in [
            "modules/warehouse/1.0.0/descriptor.json",
            "modules/warehouse/1.0.0/i18n/fr.json",
            "modules/warehouse/1.0.0/i18n/en.json",
            "modules/warehouse/1.0.0/manifest.json",
        ] {
            assert!(root.join(rel).is_file(), "missing {rel}");
        }
        assert_eq!(
            d.store.list().await.expect("list"),
            vec![("warehouse".to_string(), "1.0.0".to_string())]
        );

        d.runtime()
            .load(ModuleSource::Stored {
                store: d.store.clone(),
                name: "warehouse".into(),
                version: None,
            })
            .await
            .expect("load from disk");
        let (s, created) = d
            .call_rest(
                Method::POST,
                "/parcels",
                "admin",
                "fr",
                Some(json!({ "reference": "d-1", "weight": 1.5 })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{created}");
        assert_eq!(
            d.host.locales.load().lookup(
                &Locale::new("fr"),
                &MessageKey::new("warehouse.parcel.title")
            ),
            Some("Colis")
        );

        d.runtime().unload("warehouse").await.expect("unload");
        let (s, v) = d
            .call_rest(Method::GET, "/parcels", "user", "fr", None)
            .await;
        assert_eq!(
            (s, code(&v)),
            (StatusCode::NOT_FOUND, "this.module.not_found"),
            "{v}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Load and unload driven by the admin interface, including its own `admin_only` policy.
    #[tokio::test]
    async fn hot_module_admin_interface_loads_and_unloads_from_the_store() {
        let d = demo(Arc::new(InMemoryVfs::new()), None);
        seed(&d).await.expect("stored in the VFS");

        let (s, list) = d
            .call_rest(Method::GET, "/admin/modules", "admin", "en", None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            list.as_array().map(Vec::len),
            Some(0),
            "the store is only read on request: {list}"
        );

        // A plain caller never loads code into a live host.
        let (s, v) = d
            .call_rest(
                Method::POST,
                "/admin/modules",
                "user",
                "en",
                Some(json!({ "store": { "name": "warehouse" } })),
            )
            .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");

        let (s, status) = d
            .call_rest(
                Method::POST,
                "/admin/modules",
                "admin",
                "en",
                Some(json!({ "store": { "name": "warehouse" } })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{status}");
        assert_eq!(status["state"], "active");
        assert_eq!(status["name"], "warehouse");

        let (s, list) = d
            .call_rest(Method::GET, "/admin/modules", "admin", "en", None)
            .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(list[0]["name"], "warehouse");
        assert_eq!(list[0]["state"], "active");

        let (s, created) = d
            .call_rest(
                Method::POST,
                "/parcels",
                "admin",
                "en",
                Some(json!({ "reference": "a-1", "weight": 2 })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{created}");

        let (s, status) = d
            .call_rest(
                Method::DELETE,
                "/admin/modules/warehouse",
                "admin",
                "en",
                None,
            )
            .await;
        assert_eq!(
            (s, status["state"].clone()),
            (StatusCode::OK, json!("unloaded"))
        );

        let (s, v) = d
            .call_rest(Method::GET, "/parcels", "user", "en", None)
            .await;
        assert_eq!(
            (s, code(&v)),
            (StatusCode::NOT_FOUND, "this.module.not_found"),
            "{v}"
        );
    }
}
