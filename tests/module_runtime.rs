//! Runtime modules on a LIVE host (stream modules-runtime): load → served → unload → 404,
//! dependents, replace, drain and cancellation, dispatch before the link wildcards,
//! declarative entities (events + links), admin interface and its policy, plugin
//! extension point.
//!
//! Every test goes through the real REST exposure (`RestExposure::build_router`) with a
//! real authentication layer (header-based provider), so the layer order is the
//! production one.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthProvider};
use this::core::events::{EntityEvent, FrameworkEvent, LinkEvent, ModuleEvent};
use this::core::module::Module;
use this::descriptor::{
    ContractError, EntityDesc, FieldDesc, FieldType, LinkDesc, ModuleDescriptor, ModuleReq,
    ModuleState, Operation, PolicyExpr,
};
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::host::ServerHost;
use this::server::module_runtime::{
    DefaultModuleLoader, ModuleCancellation, ModuleInterface, ModuleLoader, ModuleRuntime,
    ModuleSource, RuntimeOptions,
};
use this::storage::in_memory::InMemoryLinkService;

// ── fixtures ────────────────────────────────────────────────────────────────

/// `x-test-auth: admin|user` → context; absent → anonymous.
struct HeaderAuth;

#[async_trait]
impl AuthProvider for HeaderAuth {
    async fn extract_context(&self, headers: &HeaderMap) -> anyhow::Result<AuthContext> {
        Ok(
            match headers.get("x-test-auth").and_then(|v| v.to_str().ok()) {
                Some("admin") => AuthContext::Admin {
                    admin_id: uuid::Uuid::from_u128(9),
                },
                Some("user") => AuthContext::User {
                    user_id: uuid::Uuid::from_u128(1),
                    tenant_id: uuid::Uuid::from_u128(1),
                    roles: vec![],
                },
                _ => AuthContext::Anonymous,
            },
        )
    }
    async fn is_owner(&self, _: &uuid::Uuid, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// Static legacy module built into the host: `/orders`.
struct Shop;
struct Orders;
impl EntityDescriptor for Orders {
    fn entity_type(&self) -> &str {
        "order"
    }
    fn plural(&self) -> &str {
        "orders"
    }
    fn build_routes(&self) -> Router {
        Router::new().route("/orders", axum::routing::get(|| async { "[]" }))
    }
}
impl Module for Shop {
    fn name(&self) -> &str {
        "shop"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["order"]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".into(),
                plural: "orders".into(),
                auth: EntityAuthConfig {
                    list: "public".into(),
                    ..Default::default()
                },
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        })
    }
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(Orders));
    }
}

/// Declarative entity with explicit policies (stored modules have no defaults).
fn entity(name: &str, policy: PolicyExpr) -> EntityDesc {
    let mut e = EntityDesc::new(name);
    e.fields.push(FieldDesc::new("amount", FieldType::F64));
    for op in Operation::CRUD {
        e.permissions.ops.insert(op, policy.clone());
    }
    e
}

/// `billing` (invoice, payment, link payment -settles-> invoice).
fn billing(version: &str, policy: PolicyExpr) -> ModuleDescriptor {
    let mut d = ModuleDescriptor::new("billing", version);
    d.entities.push(entity("invoice", policy.clone()));
    d.entities.push(entity("payment", policy));
    d.links.push(LinkDesc::new("settle", "payment", "invoice"));
    d.normalize();
    d
}

/// `crm` requires `billing` (`req`).
fn crm(req: &str) -> ModuleDescriptor {
    let mut d = ModuleDescriptor::new("crm", "1.0.0");
    d.requires.push(ModuleReq {
        name: "billing".into(),
        version: req.into(),
        optional: false,
    });
    d.entities
        .push(entity("customer", PolicyExpr::Authenticated));
    d.normalize();
    d
}

fn manifest(d: &ModuleDescriptor) -> ModuleSource {
    ModuleSource::Manifest(d.to_json_pretty())
}

struct Harness {
    app: Router,
    host: Arc<ServerHost>,
}

impl Harness {
    fn runtime(&self) -> &Arc<ModuleRuntime> {
        self.host.module_runtime.as_ref().expect("runtime enabled")
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        who: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(w) = who {
            req = req.header("x-test-auth", w);
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()));
        (status, v)
    }
}

fn harness_with(b: ServerBuilder, authorize: bool) -> Harness {
    let mut b = b
        .with_link_service(InMemoryLinkService::new())
        .with_event_bus(256)
        .register_module(Shop)
        .expect("module");
    if !authorize {
        b = b.without_authorization();
    }
    let host = b
        .build_host()
        .expect("host")
        .with_auth_provider(Arc::new(HeaderAuth));
    let host = Arc::new(host);
    let app = RestExposure::build_router(host.clone(), vec![]).expect("router");
    Harness { app, host }
}

fn harness(authorize: bool) -> Harness {
    harness_with(ServerBuilder::new().with_module_runtime(), authorize)
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

fn code(v: &Value) -> &str {
    v["code"].as_str().unwrap_or("")
}

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_then_routes_served_unload_then_404() {
    let h = harness(false);
    let mut rx = h.host.event_bus().unwrap().subscribe();

    // Before load: the static router's link fallback answers (400 today), not a module.
    let (s, _) = h.call(Method::GET, "/invoices", None, None).await;
    assert!(s.is_client_error(), "not served before load: {s}");

    let status = h
        .runtime()
        .load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .expect("load");
    assert_eq!(status.state, ModuleState::Active);
    let gen_after_load = h.host.catalog.load().generation;
    assert!(h.host.catalog.load().entity_of_plural("invoices").is_some());

    let (s, created) = h
        .call(
            Method::POST,
            "/invoices",
            None,
            Some(json!({"name": "a", "amount": 3.5})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let (s, got) = h
        .call(Method::GET, &format!("/invoices/{id}"), None, None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["amount"], json!(3.5));
    let (s, list) = h.call(Method::GET, "/invoices", None, None).await;
    assert_eq!(
        (s, list["pagination"]["total"].clone()),
        (StatusCode::OK, json!(1))
    );
    // Static routes untouched.
    assert_eq!(
        h.call(Method::GET, "/orders", None, None).await.0,
        StatusCode::OK
    );

    let status = h.runtime().unload("billing").await.expect("unload");
    assert_eq!(status.state, ModuleState::Unloaded);
    assert!(h.host.catalog.load().generation > gen_after_load);
    assert!(h.host.catalog.load().entity_of_plural("invoices").is_none());
    for uri in ["/invoices".to_string(), format!("/invoices/{id}")] {
        let (s, v) = h.call(Method::GET, &uri, None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri} after unload");
        assert_eq!(code(&v), "this.module.not_found", "{uri}");
    }

    let events = module_events(&mut rx);
    assert!(events.iter().any(|e| matches!(e, FrameworkEvent::Module(ModuleEvent::Loaded { module, .. }) if module == "billing")));
    assert!(events.iter().any(|e| matches!(e, FrameworkEvent::Module(ModuleEvent::Unloaded { module, .. }) if module == "billing")));

    // Reload after unload: Unloaded -> Discovered -> Resolved -> Active.
    h.runtime()
        .load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .expect("reload");
    assert_eq!(
        h.call(Method::GET, "/invoices", None, None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn unload_refused_with_active_dependents() {
    let h = harness(false);
    let rt = h.runtime();
    let err = rt.load(manifest(&crm("^1"))).await.unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.dependency_missing");
    assert!(matches!(
        rt.status("crm").await.unwrap().state,
        ModuleState::Failed { .. }
    ));

    rt.load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap();
    rt.load(manifest(&crm("^1"))).await.unwrap();
    let err = rt.unload("billing").await.unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.has_dependents");
    assert_eq!(err.params["dependents"], json!("crm"));
    assert_eq!(
        rt.status("billing").await.unwrap().state,
        ModuleState::Active
    );
    assert_eq!(
        h.call(Method::GET, "/invoices", None, None).await.0,
        StatusCode::OK
    );

    rt.unload("crm").await.unwrap();
    rt.unload("billing").await.unwrap();
    // Built-in modules (no descriptor here) are not runtime-managed; unknown → not_found.
    assert_eq!(
        rt.unload("nope").await.unwrap_err().code.as_str(),
        "this.module.not_found"
    );
}

#[tokio::test]
async fn replace_checks_dependents_version_req() {
    let h = harness(false);
    let rt = h.runtime();
    let mut rx = h.host.event_bus().unwrap().subscribe();
    rt.load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap();
    rt.load(manifest(&crm("^1"))).await.unwrap();

    let err = rt
        .replace(manifest(&billing("2.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.dependency_version");
    assert_eq!(err.params["module"], json!("crm"));
    assert_eq!(
        rt.status("billing").await.unwrap().version,
        "1.0.0",
        "unchanged"
    );

    let s = rt
        .replace(manifest(&billing("1.1.0", PolicyExpr::Authenticated)))
        .await
        .unwrap();
    assert_eq!(
        (s.version.as_str(), s.state),
        ("1.1.0", ModuleState::Active)
    );
    let events = module_events(&mut rx);
    assert!(events.iter().any(|e| matches!(e,
        FrameworkEvent::Module(ModuleEvent::Replaced { from, to, .. }) if from == "1.0.0" && to == "1.1.0")));
    // The refused replace is observable too (same as a refused load).
    assert!(events.iter().any(|e| matches!(e,
        FrameworkEvent::Module(ModuleEvent::Failed { code, .. }) if code == "this.module.dependency_version")));
}

#[tokio::test]
async fn duplicate_entity_or_plural_with_host_is_refused_and_nothing_published() {
    let h = harness(false);
    let rt = h.runtime();
    let gen0 = h.host.catalog.load().generation;
    let mut d = ModuleDescriptor::new("shadow", "1.0.0");
    d.entities.push(entity("order", PolicyExpr::Authenticated));
    d.normalize();
    let err = rt.load(manifest(&d)).await.unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.duplicate_entity");
    assert_eq!(h.host.catalog.load().generation, gen0, "nothing published");
    assert!(matches!(
        rt.status("shadow").await.unwrap().state,
        ModuleState::Failed { .. }
    ));
}

#[tokio::test]
async fn json_entity_publishes_event_and_links() {
    let h = harness(false);
    let mut rx = h.host.event_bus().unwrap().subscribe();
    h.runtime()
        .load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap();
    let (_, inv) = h
        .call(
            Method::POST,
            "/invoices",
            None,
            Some(json!({"name": "i", "amount": 10.0})),
        )
        .await;
    let (_, pay) = h
        .call(
            Method::POST,
            "/payments",
            None,
            Some(json!({"name": "p", "amount": 10.0})),
        )
        .await;
    let (iid, pid) = (inv["id"].as_str().unwrap(), pay["id"].as_str().unwrap());

    // Validation from the descriptor: unknown key and wrong type are refused, same codes.
    let (s, bad) = h
        .call(
            Method::POST,
            "/invoices",
            None,
            Some(json!({"name": "x", "amount": "ten"})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    assert_eq!(code(&bad), "this.validation.failed");

    // Link route of the runtime module (`payments/{id}/invoices-settles/{id}`) is served
    // by the snapshot link router, not shadowed by the static link wildcards.
    let (s, link) = h
        .call(
            Method::POST,
            &format!("/payments/{pid}/invoices-settles/{iid}"),
            None,
            Some(json!({})),
        )
        .await;
    assert!(s.is_success(), "{s} {link}");
    let (s, listed) = h
        .call(
            Method::GET,
            &format!("/payments/{pid}/invoices-settles"),
            None,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{listed}");
    assert!(listed.to_string().contains(iid), "{listed}");

    let events = module_events(&mut rx);
    assert!(events.iter().any(|e| matches!(e,
        FrameworkEvent::Entity(EntityEvent::Created { entity_type, .. }) if entity_type == "invoice")));
    assert!(events.iter().any(|e| matches!(e,
        FrameworkEvent::Link(LinkEvent::Created { link_type, .. }) if link_type == "settle")));
}

#[tokio::test]
async fn dynamic_module_paths_not_shadowed_by_link_wildcards() {
    let h = harness(false);
    let id = uuid::Uuid::new_v4();
    // Before load, a 2-segment path falls to the static router (not a module route).
    let (before, _) = h
        .call(Method::GET, &format!("/invoices/{id}"), None, None)
        .await;
    h.runtime()
        .load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap();
    // After load: the module answers with ITS contract (`this.entity.not_found`), proving
    // the request reached the module router and not the link fallback.
    let (s, v) = h
        .call(Method::GET, &format!("/invoices/{id}"), None, None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        code(&v),
        "this.entity.not_found",
        "before load: {before}, after: {v}"
    );
    let (s, _) = h
        .call(
            Method::POST,
            "/invoices",
            None,
            Some(json!({"name": "n", "amount": 1.0})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
}

// ── drain and cancellation ─────────────────────────────────────────────────

/// Compiled module whose `/streams` handler waits for the module's cancellation.
/// Version given by the field (replace tests).
struct Streams(&'static str);
struct StreamRoutes;
impl EntityDescriptor for StreamRoutes {
    fn entity_type(&self) -> &str {
        "stream"
    }
    fn plural(&self) -> &str {
        "streams"
    }
    fn build_routes(&self) -> Router {
        Router::new().route(
            "/streams",
            axum::routing::get(
                |axum::Extension(mut c): axum::Extension<ModuleCancellation>| async move {
                    c.cancelled().await;
                    "cancelled"
                },
            ),
        )
    }
}
impl Module for Streams {
    fn name(&self) -> &str {
        "streams"
    }
    fn version(&self) -> &str {
        self.0
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["stream"]
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
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(StreamRoutes));
    }
}

#[tokio::test]
async fn draining_times_out_and_cancels_streams() {
    let h = Arc::new(harness_with(
        ServerBuilder::new()
            .with_module_runtime_options(RuntimeOptions {
                drain_grace: Duration::from_millis(150),
                ..Default::default()
            })
            .offer_module(Streams("1.0.0")),
        false,
    ));
    assert_eq!(
        h.runtime().status("streams").await.unwrap().state,
        ModuleState::Discovered
    );
    h.runtime()
        .load_offered("streams")
        .await
        .expect("load offered");

    let streaming = {
        let h = h.clone();
        tokio::spawn(async move { h.call(Method::GET, "/streams", None, None).await })
    };
    tokio::time::sleep(Duration::from_millis(40)).await;
    let started = std::time::Instant::now();
    let unloading = {
        let h = h.clone();
        tokio::spawn(async move { h.runtime().unload("streams").await })
    };
    tokio::time::sleep(Duration::from_millis(40)).await;
    // Draining: new requests get 503 this.module.unavailable.
    let (s, v) = h.call(Method::GET, "/streams", None, None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&v), "this.module.unavailable");
    assert_eq!(
        h.runtime().status("streams").await.unwrap().state,
        ModuleState::Draining
    );

    let unloaded = unloading.await.unwrap().expect("unload after grace");
    assert_eq!(unloaded.state, ModuleState::Unloaded);
    assert!(
        started.elapsed() >= Duration::from_millis(140),
        "waited for the grace delay"
    );
    let (s, body) = streaming.await.unwrap();
    assert_eq!(
        (s, body),
        (StatusCode::OK, Value::String("cancelled".into()))
    );
    assert_eq!(
        h.call(Method::GET, "/streams", None, None).await.0,
        StatusCode::NOT_FOUND
    );
}

// ── admin interface and policies ───────────────────────────────────────────

#[tokio::test]
async fn admin_interface_requires_admin_then_lists_loads_unloads() {
    let h = harness(true);
    let body =
        json!({"manifest": serde_json::to_value(billing("1.0.0", PolicyExpr::AdminOnly)).unwrap()});

    let (s, v) = h
        .call(Method::POST, "/admin/modules", None, Some(body.clone()))
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::UNAUTHORIZED, "this.auth.unauthenticated")
    );
    let (s, v) = h
        .call(
            Method::POST,
            "/admin/modules",
            Some("user"),
            Some(body.clone()),
        )
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::FORBIDDEN, "this.auth.forbidden")
    );

    let (s, v) = h
        .call(Method::POST, "/admin/modules", Some("admin"), Some(body))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["state"], json!("active"));
    let (s, v) = h
        .call(Method::GET, "/admin/modules", Some("admin"), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        v.as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == json!("billing"))
    );
    let (s, v) = h
        .call(Method::GET, "/admin/modules/billing", Some("admin"), None)
        .await;
    assert_eq!((s, v["version"].clone()), (StatusCode::OK, json!("1.0.0")));

    // Policies of the runtime module are enforced by the dispatch layer, with the
    // legacy authorization layer ARMED (it would refuse by abstention otherwise).
    assert!(h.host.authorize);
    let (s, _) = h.call(Method::GET, "/invoices", Some("user"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "admin_only refuses a user");
    let (s, v) = h.call(Method::GET, "/invoices", Some("admin"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Static routes keep the legacy policy layer.
    assert_eq!(
        h.call(Method::GET, "/orders", None, None).await.0,
        StatusCode::OK
    );

    let (s, v) = h
        .call(
            Method::DELETE,
            "/admin/modules/billing",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!((s, v["state"].clone()), (StatusCode::OK, json!("unloaded")));
    let (s, v) = h
        .call(Method::GET, "/admin/modules/missing", Some("admin"), None)
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::NOT_FOUND, "this.module.not_found")
    );
    let (s, v) = h
        .call(
            Method::POST,
            "/admin/modules",
            Some("admin"),
            Some(json!({"module": "a", "manifest": {}})),
        )
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::BAD_REQUEST, "this.transport.bad_request")
    );
}

#[tokio::test]
async fn host_without_runtime_has_no_admin_surface() {
    let h = harness_with(ServerBuilder::new(), false);
    assert!(h.host.module_runtime.is_none());
    let (s, _) = h
        .call(Method::GET, "/admin/modules", Some("admin"), None)
        .await;
    assert_ne!(s, StatusCode::OK);
}

// ── plugin extension point (WASM/WAMI) ─────────────────────────────────────

/// Stands in for a WASM/WAMI loader: produces a declarative interface in-process.
struct FakeWasm;
#[async_trait]
impl ModuleLoader for FakeWasm {
    async fn load(&self, source: &ModuleSource) -> Result<ModuleInterface, ContractError> {
        let ModuleSource::Plugin { location, .. } = source else {
            unreachable!()
        };
        assert_eq!(location, "billing.wasm");
        DefaultModuleLoader::new()
            .load(&manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
            .await
    }
}

#[tokio::test]
async fn plugin_source_is_an_extension_point() {
    let plain = harness(false);
    let err = plain
        .runtime()
        .load(ModuleSource::Plugin {
            kind: "wasm".into(),
            location: "billing.wasm".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code.as_str(), "this.not_implemented");

    let h = harness_with(
        ServerBuilder::new().with_module_loader(Arc::new(
            DefaultModuleLoader::new().with_plugin("wasm", Arc::new(FakeWasm)),
        )),
        false,
    );
    h.runtime()
        .load(ModuleSource::Plugin {
            kind: "wasm".into(),
            location: "billing.wasm".into(),
        })
        .await
        .expect("plugin loader used");
    assert_eq!(
        h.call(Method::GET, "/invoices", None, None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn stored_profile_refuses_missing_policies() {
    let h = harness(false);
    let mut d = ModuleDescriptor::new("lax", "1.0.0");
    d.entities.push(EntityDesc::new("note"));
    d.normalize();
    let err = h.runtime().load(manifest(&d)).await.unwrap_err();
    assert_eq!(err.code.as_str(), "this.descriptor.invalid");
}

// ── review (relecteur-approfondisseur): error paths, identity, policies ───────

/// Described compiled module, BUILT INTO the host or offered: `catalogue` (product).
/// `extra` adds an entity type its descriptor does not declare (identity mismatch).
struct Catalogue {
    extra: bool,
}
struct Products;
impl EntityDescriptor for Products {
    fn entity_type(&self) -> &str {
        "product"
    }
    fn plural(&self) -> &str {
        "products"
    }
    fn build_routes(&self) -> Router {
        Router::new().route("/products", axum::routing::get(|| async { "[]" }))
    }
}
impl Module for Catalogue {
    fn name(&self) -> &str {
        "catalogue"
    }
    fn entity_types(&self) -> Vec<&str> {
        if self.extra {
            vec!["product", "order"]
        } else {
            vec!["product"]
        }
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
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(Products));
    }
    fn descriptor(&self) -> Option<ModuleDescriptor> {
        let mut d = ModuleDescriptor::new("catalogue", "1.0.0");
        d.entities.push(EntityDesc::new("product"));
        Some(d)
    }
}

#[tokio::test]
async fn admin_interface_enforced_even_with_authorization_disarmed() {
    let h = harness(false);
    assert!(!h.host.authorize, "authorization disarmed on this host");
    let body = json!({"manifest": serde_json::to_value(billing("1.0.0", PolicyExpr::Authenticated)).unwrap()});

    let (s, v) = h.call(Method::GET, "/admin/modules", None, None).await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::UNAUTHORIZED, "this.auth.unauthenticated")
    );
    let (s, v) = h
        .call(Method::POST, "/admin/modules", Some("user"), Some(body))
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::FORBIDDEN, "this.auth.forbidden")
    );
    assert_eq!(
        h.runtime()
            .status("billing")
            .await
            .unwrap_err()
            .code
            .as_str(),
        "this.module.not_found",
        "a refused admin call loads nothing"
    );
    // An unsupported operation says so (not a misleading `module.not_found`).
    let (s, v) = h
        .call(Method::PUT, "/admin/modules/billing", Some("admin"), None)
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::BAD_REQUEST, "this.transport.bad_request")
    );
}

#[tokio::test]
async fn loading_an_active_module_again_is_refused_and_unload_twice_too() {
    let h = harness(false);
    let rt = h.runtime();
    rt.load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap();
    let gen1 = h.host.catalog.load().generation;

    let err = rt
        .load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.duplicate_module");
    assert_eq!(h.host.catalog.load().generation, gen1, "nothing published");
    assert_eq!(
        rt.status("billing").await.unwrap().state,
        ModuleState::Active,
        "live record kept"
    );
    assert_eq!(
        h.call(Method::GET, "/invoices", None, None).await.0,
        StatusCode::OK
    );

    rt.unload("billing").await.unwrap();
    let err = rt.unload("billing").await.unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.illegal_transition");
}

#[tokio::test]
async fn builtin_described_module_cannot_be_unloaded_nor_shadowed() {
    let h = harness_with(
        ServerBuilder::new()
            .with_module_runtime()
            .register_module(Catalogue { extra: false })
            .unwrap(),
        false,
    );
    let rt = h.runtime();
    let s = rt.status("catalogue").await.unwrap();
    assert_eq!((s.builtin, s.state), (true, ModuleState::Active));
    let err = rt.unload("catalogue").await.unwrap_err();
    assert_eq!(err.code.as_str(), "this.not_implemented");
    assert_eq!(
        h.call(Method::GET, "/products", None, None).await.0,
        StatusCode::OK
    );

    // A runtime module can take neither its entity nor its plural.
    let mut d = ModuleDescriptor::new("shadow", "1.0.0");
    d.entities
        .push(entity("product", PolicyExpr::Authenticated));
    d.normalize();
    let err = rt.load(manifest(&d)).await.unwrap_err();
    assert!(
        matches!(
            err.code.as_str(),
            "this.module.duplicate_entity" | "this.module.duplicate_plural"
        ),
        "{}",
        err.code.as_str()
    );
}

#[tokio::test]
async fn compiled_module_must_match_its_descriptor_at_runtime() {
    let h = harness_with(
        ServerBuilder::new()
            .with_module_runtime()
            .offer_module(Catalogue { extra: true }),
        false,
    );
    let gen0 = h.host.catalog.load().generation;
    let err = h.runtime().load_offered("catalogue").await.unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.identity_mismatch");
    assert!(
        err.params["what"].to_string().contains("order"),
        "{:?}",
        err.params
    );
    assert_eq!(h.host.catalog.load().generation, gen0, "nothing published");
}

/// Plugin loader that smuggles a fetcher for the HOST's `order` entity.
struct Rogue;
#[async_trait]
impl ModuleLoader for Rogue {
    async fn load(&self, _: &ModuleSource) -> Result<ModuleInterface, ContractError> {
        let mut i = DefaultModuleLoader::new()
            .load(&manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
            .await?;
        let f = i.fetchers["invoice"].clone();
        i.fetchers.insert("order".into(), f);
        Ok(i)
    }
}

#[tokio::test]
async fn fetcher_outside_the_descriptor_is_refused_not_last_wins() {
    let h = harness_with(
        ServerBuilder::new().with_module_loader(Arc::new(
            DefaultModuleLoader::new().with_plugin("rogue", Arc::new(Rogue)),
        )),
        false,
    );
    let before = h.host.catalog.load();
    let err = h
        .runtime()
        .load(ModuleSource::Plugin {
            kind: "rogue".into(),
            location: "x".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code.as_str(), "this.module.identity_mismatch");
    let after = h.host.catalog.load();
    assert_eq!(after.generation, before.generation, "nothing published");
    assert_eq!(
        after.fetchers.contains_key("order"),
        before.fetchers.contains_key("order")
    );
    assert!(
        h.call(Method::GET, "/invoices", None, None)
            .await
            .0
            .is_client_error()
    );
}

#[tokio::test]
async fn runtime_module_policies_enforced_on_crud_and_link_routes() {
    let h = harness(true);
    h.runtime()
        .load(manifest(&billing("1.0.0", PolicyExpr::Authenticated)))
        .await
        .unwrap();

    let (s, v) = h.call(Method::GET, "/invoices", None, None).await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::UNAUTHORIZED, "this.auth.unauthenticated")
    );
    let (s, v) = h
        .call(
            Method::POST,
            "/invoices",
            None,
            Some(json!({"name": "i", "amount": 1.0})),
        )
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::UNAUTHORIZED, "this.auth.unauthenticated")
    );

    let (s, inv) = h
        .call(
            Method::POST,
            "/invoices",
            Some("user"),
            Some(json!({"name": "i", "amount": 1.0})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{inv}");
    let (_, pay) = h
        .call(
            Method::POST,
            "/payments",
            Some("user"),
            Some(json!({"name": "p", "amount": 1.0})),
        )
        .await;
    let (iid, pid) = (inv["id"].as_str().unwrap(), pay["id"].as_str().unwrap());

    // Link routes of the runtime module are judged too (not left to a frozen layer).
    let link = format!("/payments/{pid}/invoices-settles/{iid}");
    let (s, v) = h.call(Method::POST, &link, None, Some(json!({}))).await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::UNAUTHORIZED, "this.auth.unauthenticated")
    );
    let (s, v) = h
        .call(
            Method::GET,
            &format!("/payments/{pid}/invoices-settles"),
            None,
            None,
        )
        .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::UNAUTHORIZED, "this.auth.unauthenticated")
    );
    // Unknown shape under a runtime plural: refused, never passed through.
    let (s, _) = h
        .call(Method::OPTIONS, "/invoices", Some("user"), None)
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

/// Legacy compiled module WITHOUT `Module::descriptor()`: its policies live only in its
/// YAML `auth:` (`list: admin_only`).
struct Vault;
struct Secrets;
impl EntityDescriptor for Secrets {
    fn entity_type(&self) -> &str {
        "secret"
    }
    fn plural(&self) -> &str {
        "secrets"
    }
    fn build_routes(&self) -> Router {
        Router::new().route("/secrets", axum::routing::get(|| async { "[]" }))
    }
}
impl Module for Vault {
    fn name(&self) -> &str {
        "vault"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["secret"]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![EntityConfig {
                singular: "secret".into(),
                plural: "secrets".into(),
                auth: EntityAuthConfig {
                    list: "admin_only".into(),
                    ..Default::default()
                },
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        })
    }
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(Secrets));
    }
}

/// Review regression: a hot-loaded compiled module without a descriptor kept its YAML
/// `auth:` only on paper — the derived (policy-less) descriptor was overlaid on it and the
/// dispatch judged from descriptors alone, so `list: admin_only` became `authenticated`.
#[tokio::test]
async fn hot_loaded_module_without_descriptor_keeps_its_yaml_policies() {
    let h = harness(true);
    h.runtime()
        .load(ModuleSource::Compiled(Arc::new(Vault)))
        .await
        .expect("load");
    let (s, v) = h.call(Method::GET, "/secrets", Some("user"), None).await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::FORBIDDEN, "this.auth.forbidden"),
        "YAML `list: admin_only` must hold for a plain user: {v}"
    );
    let (s, _) = h.call(Method::GET, "/secrets", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v) = h.call(Method::GET, "/secrets", Some("admin"), None).await;
    assert_eq!(s, StatusCode::OK, "served to the admin: {v}");
}

#[tokio::test]
async fn report_only_observes_runtime_refusals_without_blocking() {
    let h = harness_with(
        ServerBuilder::new()
            .with_module_runtime()
            .authorization_report_only(),
        true,
    );
    assert!(h.host.authorize_report_only);
    h.runtime()
        .load(manifest(&billing("1.0.0", PolicyExpr::AdminOnly)))
        .await
        .unwrap();
    assert_eq!(
        h.call(Method::GET, "/invoices", None, None).await.0,
        StatusCode::OK
    );
    // The admin interface is NOT report-only: loading code stays admin-only.
    let (s, _) = h.call(Method::GET, "/admin/modules", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn replace_cancels_old_version_streams_after_grace() {
    let h = Arc::new(harness_with(
        ServerBuilder::new()
            .with_module_runtime_options(RuntimeOptions {
                drain_grace: Duration::from_millis(150),
                ..Default::default()
            })
            .offer_module(Streams("1.0.0")),
        false,
    ));
    h.runtime()
        .load_offered("streams")
        .await
        .expect("load offered");
    let old_stream = {
        let h = h.clone();
        tokio::spawn(async move { h.call(Method::GET, "/streams", None, None).await })
    };
    tokio::time::sleep(Duration::from_millis(40)).await;
    let started = std::time::Instant::now();
    let s = h
        .runtime()
        .replace(ModuleSource::Compiled(Arc::new(Streams("1.1.0"))))
        .await
        .expect("replace");
    assert_eq!(
        (s.version.as_str(), s.state),
        ("1.1.0", ModuleState::Active)
    );

    // The stream opened on 1.0.0 ends after the grace delay (it would hang forever on
    // code that is no longer published otherwise).
    let (st, body) = tokio::time::timeout(Duration::from_secs(3), old_stream)
        .await
        .expect("old version's stream cancelled")
        .unwrap();
    assert_eq!(
        (st, body),
        (StatusCode::OK, Value::String("cancelled".into()))
    );
    assert!(
        started.elapsed() >= Duration::from_millis(140),
        "waited for the grace delay"
    );

    // A stream on the NEW version is not cancelled by the old version's drain.
    let fresh = {
        let h = h.clone();
        tokio::spawn(async move { h.call(Method::GET, "/streams", None, None).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!fresh.is_finished(), "new version's stream still open");
    fresh.abort();
}
