//! Module descriptor, end to end (integration of the md/* streams).
//!
//! ONE descriptor drives every surface of a real host:
//! - the same invalid payload is refused with the same contract code on REST (real
//!   `RestExposure` with its validation, policy, localization and authentication layers),
//!   GraphQL (armed router) and gRPC (`EntityService`, identity from the call metadata);
//! - each protocol maps the contract kind to its own status (422 / `INVALID_ARGUMENT` /
//!   `BAD_USER_INPUT`, 403 / `PERMISSION_DENIED` / `FORBIDDEN`);
//! - the message is rendered in the request locale (fr, then en) on all three;
//! - a policy declared only in the descriptor refuses the same caller everywhere;
//! - a module stored in a VFS is hot-loaded (routes, validation, bundles), served, then
//!   unloaded (404 `this.module.not_found`, bundles gone).
//!
//! Run: `cargo test -p this-rs --features graphql,grpc --test module_descriptor_e2e`.

#![cfg(all(feature = "graphql", feature = "grpc"))]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthProvider};
use this::core::errors::grpc::contract_error_of;
use this::core::{EntityCreator, EntityFetcher};
use this::descriptor::{
    Applies, DescriptorSet, EntityDesc, FieldDesc, FieldType, Locale, MessageBundle, MessageKey,
    MessageSyntax, ModuleDescriptor, Operation, PolicyExpr, ValidatorSpec,
};
use this::server::builder::ServerBuilder;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::GraphQLExposure;
use this::server::exposure::grpc::entity_service::EntityServiceImpl;
use this::server::exposure::grpc::proto::CreateEntityRequest;
use this::server::exposure::grpc::proto::entity_service_server::EntityService;
use this::server::exposure::grpc::proto_generator::ProtoGenerator;
use this::server::exposure::rest::RestExposure;
use this::server::host::ServerHost;
use this::server::module_runtime::ModuleSource;
use this::storage::InMemoryLinkService;
use this::storage::vfs::{InMemoryVfs, ModuleStore, VfsModuleStore};

// ── fixtures ────────────────────────────────────────────────────────────────

/// Records every write that reaches it: a refused call must never get here.
#[derive(Default)]
struct Store {
    writes: Mutex<usize>,
}

#[async_trait]
impl EntityCreator for Store {
    async fn create_from_json(&self, mut v: Value) -> anyhow::Result<Value> {
        *self.writes.lock().unwrap() += 1;
        v["id"] = json!(Uuid::new_v4().to_string());
        Ok(v)
    }
    async fn update_from_json(&self, _: &Uuid, v: Value) -> anyhow::Result<Value> {
        *self.writes.lock().unwrap() += 1;
        Ok(v)
    }
    async fn delete(&self, _: &Uuid) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl EntityFetcher for Store {
    async fn fetch_as_json(&self, id: &Uuid) -> anyhow::Result<Value> {
        Ok(json!({ "id": id.to_string() }))
    }
    async fn list_as_json(&self, _: Option<i32>, _: Option<i32>) -> anyhow::Result<Vec<Value>> {
        Ok(vec![])
    }
}

/// REST routes as the CLI generates them: the payload straight to the store (the
/// validation layer in front of them is what is under test).
struct Routes(Arc<Store>);

impl EntityDescriptor for Routes {
    fn entity_type(&self) -> &str {
        "invoice"
    }
    fn plural(&self) -> &str {
        "invoices"
    }
    fn build_routes(&self) -> Router {
        let store = self.0.clone();
        Router::new().route(
            "/invoices",
            axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
                let store = store.clone();
                async move {
                    let created = store.create_from_json(v).await.unwrap();
                    (StatusCode::CREATED, axum::Json(created))
                }
            }),
        )
    }
}

/// `Bearer admin` → role `admin`; `Bearer user` → no role; anything else is refused.
struct Tokens;

#[async_trait]
impl AuthProvider for Tokens {
    async fn extract_context(&self, h: &HeaderMap) -> anyhow::Result<AuthContext> {
        let roles = match h.get("authorization").and_then(|v| v.to_str().ok()) {
            Some("Bearer admin") => vec!["admin".to_string()],
            Some("Bearer user") => vec![],
            _ => anyhow::bail!("no or bad token"),
        };
        Ok(AuthContext::User {
            user_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            roles,
        })
    }
    async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// `billing`: `invoice { number: string 3..50, amount: f64 > 0 }`, declared ONCE.
/// Policies live in the descriptor only: create = `role:admin`, the rest authenticated.
fn billing() -> ModuleDescriptor {
    let mut e = EntityDesc::new("invoice");
    e.fields = vec![
        FieldDesc::new("number", FieldType::String).validator(
            ValidatorSpec::StringLength { min: 3, max: 50 },
            Applies::Always,
        ),
        FieldDesc::new("amount", FieldType::F64)
            .validator(ValidatorSpec::Positive, Applies::Always),
    ];
    for op in Operation::CRUD {
        e.permissions.ops.insert(op, PolicyExpr::Authenticated);
    }
    e.permissions
        .ops
        .insert(Operation::Create, PolicyExpr::HasRole(vec!["admin".into()]));
    let mut m = ModuleDescriptor::new("billing", "1.0.0");
    m.entities.push(e);
    m
}

struct Surfaces {
    host: Arc<ServerHost>,
    store: Arc<Store>,
    rest: Router,
    graphql: Router,
}

fn surfaces() -> Surfaces {
    let store = Arc::new(Store::default());
    let config = LinksConfig {
        entities: vec![EntityConfig {
            singular: "invoice".into(),
            plural: "invoices".into(),
            auth: EntityAuthConfig::default(),
        }],
        links: vec![],
        validation_rules: None,
        events: None,
        sinks: None,
        auth: Some(serde_yaml::from_str("default_policy: authenticated").unwrap()),
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Routes(store.clone())));
    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert("invoice".into(), store.clone());
    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert("invoice".into(), store.clone());
    let mut set = DescriptorSet::new();
    set.insert(billing()).unwrap();

    let host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        config,
        registry,
        fetchers,
        creators,
    )
    .unwrap()
    .with_descriptors(set)
    .with_auth_provider(Arc::new(Tokens));
    assert!(host.authorize, "policies are armed by default");
    let host = Arc::new(host);
    Surfaces {
        rest: RestExposure::build_router(host.clone(), vec![]).expect("REST"),
        graphql: GraphQLExposure::build_router(host.clone()).expect("GraphQL served armed"),
        host,
        store,
    }
}

const INVALID: &str = r#"{"number": "ab", "amount": -1}"#;
const VALID: &str = r#"{"number": "F-001", "amount": 12.5}"#;

struct Rest {
    status: StatusCode,
    error_header: Option<String>,
    body: Value,
}

async fn rest_create(s: &Surfaces, token: &str, lang: &str, body: &str) -> Rest {
    let res = s
        .rest
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/invoices")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .header("accept-language", lang)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let error_header = res
        .headers()
        .get("x-this-error")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    Rest {
        status,
        error_header,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// First GraphQL error of `createInvoice(data: <body>)`.
async fn graphql_create(s: &Surfaces, token: &str, lang: &str, body: &str) -> Value {
    let data: Value = serde_json::from_str(body).unwrap();
    let fields: Vec<String> = data
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect();
    let query = format!(
        "mutation {{ createInvoice(data: {{{}}}) {{ id }} }}",
        fields.join(", ")
    );
    let res = s
        .graphql
        .clone()
        .oneshot(
            Request::post("/graphql")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .header("accept-language", lang)
                .body(Body::from(json!({ "query": query }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let out: Value = serde_json::from_slice(&bytes).unwrap();
    out["errors"][0].clone()
}

fn grpc_struct(v: Value) -> prost_types::Struct {
    use prost_types::value::Kind;
    let Value::Object(map) = v else { panic!() };
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| {
                let kind = match v {
                    Value::String(s) => Kind::StringValue(s),
                    Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap()),
                    Value::Bool(b) => Kind::BoolValue(b),
                    _ => Kind::NullValue(0),
                };
                (k, prost_types::Value { kind: Some(kind) })
            })
            .collect(),
    }
}

/// `CreateEntity` through the armed `EntityService`: identity and locale come from the
/// call metadata, as on the wire.
async fn grpc_create(s: &Surfaces, token: &str, lang: &str, body: &str) -> tonic::Status {
    let mut req = tonic::Request::new(CreateEntityRequest {
        entity_type: "invoice".into(),
        data: Some(grpc_struct(serde_json::from_str(body).unwrap())),
    });
    let md = req.metadata_mut();
    md.insert("authorization", format!("Bearer {token}").parse().unwrap());
    md.insert("accept-language", lang.parse().unwrap());
    EntityServiceImpl::new(s.host.clone())
        .create_entity(req)
        .await
        .expect_err("refused")
}

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn same_invalid_payload_same_code_on_rest_graphql_grpc() {
    let s = surfaces();

    let rest = rest_create(&s, "admin", "en", INVALID).await;
    assert_eq!(rest.body["code"], "this.validation.failed", "{}", rest.body);
    assert_eq!(rest.error_header.as_deref(), Some("this.validation.failed"));
    let rest_fields: Vec<&str> = rest.body["fields"]
        .as_array()
        .expect("REST fields")
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();

    let gql = graphql_create(&s, "admin", "en", INVALID).await;
    assert_eq!(
        gql["extensions"]["this_code"], "this.validation.failed",
        "{gql}"
    );
    let gql_fields: Vec<&str> = gql["extensions"]["fields"]
        .as_array()
        .expect("GraphQL fields")
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();

    let status = grpc_create(&s, "admin", "en", INVALID).await;
    let envelope = contract_error_of(&status).expect("gRPC details carry the envelope");
    assert_eq!(envelope.code.as_str(), "this.validation.failed");
    let grpc_fields: Vec<&str> = envelope.fields.iter().map(|f| f.code.as_str()).collect();

    // Both violations, the same codes, on every surface.
    for fields in [&rest_fields, &gql_fields, &grpc_fields] {
        assert!(
            fields.contains(&"this.validation.string_length"),
            "{fields:?}"
        );
        assert!(fields.contains(&"this.validation.positive"), "{fields:?}");
    }
    assert_eq!(rest_fields, gql_fields);
    assert_eq!(rest_fields, grpc_fields);
    assert_eq!(
        *s.store.writes.lock().unwrap(),
        0,
        "the store is never reached"
    );

    // The same VALID payload is accepted everywhere (the refusal is the validation's).
    assert_eq!(
        rest_create(&s, "admin", "en", VALID).await.status,
        StatusCode::CREATED
    );
    assert_eq!(*s.store.writes.lock().unwrap(), 1);
}

#[tokio::test]
async fn status_mapping_per_protocol() {
    let s = surfaces();

    // Validation: 422 / INVALID_ARGUMENT / BAD_USER_INPUT.
    assert_eq!(
        rest_create(&s, "admin", "en", INVALID).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        graphql_create(&s, "admin", "en", INVALID).await["extensions"]["code"],
        "BAD_USER_INPUT"
    );
    assert_eq!(
        grpc_create(&s, "admin", "en", INVALID).await.code(),
        tonic::Code::InvalidArgument
    );

    // Policy: 403 / PERMISSION_DENIED / FORBIDDEN.
    assert_eq!(
        rest_create(&s, "user", "en", VALID).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        graphql_create(&s, "user", "en", VALID).await["extensions"]["code"],
        "FORBIDDEN"
    );
    assert_eq!(
        grpc_create(&s, "user", "en", VALID).await.code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(*s.store.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn message_localized_fr_then_en() {
    let s = surfaces();
    let expected = [
        ("fr", "Validation échouée", "doit être positif"),
        ("en", "Validation failed", "must be positive"),
    ];
    for (lang, top, positive) in expected {
        let rest = rest_create(&s, "admin", lang, INVALID).await;
        assert_eq!(rest.body["message"], top, "REST {lang}: {}", rest.body);
        let rest_positive = rest.body["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["code"] == "this.validation.positive")
            .unwrap()["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            rest_positive.contains(positive),
            "REST {lang}: {rest_positive}"
        );

        let gql = graphql_create(&s, "admin", lang, INVALID).await;
        assert_eq!(gql["message"], top, "GraphQL {lang}: {gql}");
        let gql_positive = gql["extensions"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["code"] == "this.validation.positive")
            .unwrap()["message"]
            .as_str()
            .unwrap()
            .to_string();

        let status = grpc_create(&s, "admin", lang, INVALID).await;
        assert_eq!(status.message(), top, "gRPC {lang}");
        let envelope = contract_error_of(&status).unwrap();
        let grpc_positive = envelope
            .fields
            .iter()
            .find(|f| f.code.as_str() == "this.validation.positive")
            .unwrap()
            .message
            .clone();

        // The same text, word for word, on the three surfaces.
        assert_eq!(rest_positive, gql_positive, "{lang}");
        assert_eq!(rest_positive, grpc_positive, "{lang}");
    }
}

#[tokio::test]
async fn policy_denied_on_all_protocols() {
    let s = surfaces();

    // `role:admin` on create is declared in the descriptor ONLY (YAML says
    // `authenticated`): a plain user is refused everywhere, the store is never reached.
    let rest = rest_create(&s, "user", "en", VALID).await;
    assert_eq!(rest.status, StatusCode::FORBIDDEN, "{}", rest.body);
    let gql = graphql_create(&s, "user", "en", VALID).await;
    assert_eq!(
        gql["extensions"]["this_code"], "this.auth.forbidden",
        "{gql}"
    );
    let status = grpc_create(&s, "user", "en", VALID).await;
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
    assert_eq!(
        contract_error_of(&status).unwrap().code.as_str(),
        "this.auth.forbidden"
    );
    assert_eq!(*s.store.writes.lock().unwrap(), 0);

    // No identity at all: refused as unauthenticated, not as a validation error.
    let rest = rest_create(&s, "nobody", "en", INVALID).await;
    assert!(
        matches!(
            rest.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "{}: {}",
        rest.status,
        rest.body
    );
    assert_ne!(rest.body["code"], "this.validation.failed");
}

// ── hot loading from a VFS ──────────────────────────────────────────────────

/// `x-test-auth: user` → authenticated user; absent → anonymous.
struct HeaderAuth;

#[async_trait]
impl AuthProvider for HeaderAuth {
    async fn extract_context(&self, headers: &HeaderMap) -> anyhow::Result<AuthContext> {
        Ok(
            match headers.get("x-test-auth").and_then(|v| v.to_str().ok()) {
                Some("user") => AuthContext::User {
                    user_id: Uuid::from_u128(1),
                    tenant_id: Uuid::from_u128(1),
                    roles: vec![],
                },
                _ => AuthContext::Anonymous,
            },
        )
    }
    async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// Declarative `crm` module: `lead { amount: f64 > 0 }`, explicit policies, fr + en.
fn crm() -> ModuleDescriptor {
    let mut e = EntityDesc::new("lead");
    e.fields.push(
        FieldDesc::new("amount", FieldType::F64)
            .validator(ValidatorSpec::Positive, Applies::Always),
    );
    for op in Operation::CRUD {
        e.permissions.ops.insert(op, PolicyExpr::Authenticated);
    }
    let mut m = ModuleDescriptor::new("crm", "1.0.0");
    m.entities.push(e);
    m.i18n.locales = vec![Locale::new("fr"), Locale::new("en")];
    m.normalize();
    m
}

fn crm_bundle(locale: &str, text: &str) -> MessageBundle {
    let mut messages = BTreeMap::new();
    messages.insert(MessageKey::new("crm.lead.title"), text.to_string());
    MessageBundle {
        module: "crm".into(),
        locale: Locale::new(locale),
        syntax: MessageSyntax::Simple,
        messages,
    }
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-test-auth", "user")
        .header("accept-language", "en");
    let req = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            req.body(Body::from(b.to_string()))
        }
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn hot_loaded_module_from_vfs_served_then_404_after_unload() {
    // The module lives in a VFS-backed store (in memory here; LocalFsVfs / ObrainVfs
    // implement the same trait), with its bundles.
    let store: Arc<dyn ModuleStore> = Arc::new(VfsModuleStore::new(Arc::new(InMemoryVfs::new())));
    store
        .put(
            &crm(),
            &[crm_bundle("fr", "Prospect"), crm_bundle("en", "Lead")],
        )
        .await
        .expect("stored");

    let host = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .with_module_runtime()
        .with_module_store(store.clone())
        .build_host()
        .expect("host")
        .with_auth_provider(Arc::new(HeaderAuth));
    let host = Arc::new(host);
    let app = RestExposure::build_router(host.clone(), vec![]).expect("router");
    let runtime = host.module_runtime.clone().expect("runtime enabled");
    let title = MessageKey::new("crm.lead.title");
    let en = Locale::new("en");

    // Not served before the load.
    let (s, _) = call(&app, Method::GET, "/leads", None).await;
    assert!(s.is_client_error(), "not served before load: {s}");
    assert!(host.locales.load().lookup(&en, &title).is_none());

    runtime
        .load(ModuleSource::Stored {
            store: store.clone(),
            name: "crm".into(),
            version: None,
        })
        .await
        .expect("load from the VFS");

    // Served, validated by the descriptor, its texts in the host catalog.
    let (s, created) = call(&app, Method::POST, "/leads", Some(json!({"amount": 5.0}))).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_string();
    let (s, got) = call(&app, Method::GET, &format!("/leads/{id}"), None).await;
    assert_eq!((s, got["amount"].clone()), (StatusCode::OK, json!(5.0)));
    let (s, refused) = call(&app, Method::POST, "/leads", Some(json!({"amount": -5.0}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "this.validation.failed");
    assert_eq!(refused["fields"][0]["code"], "this.validation.positive");
    assert_eq!(host.locales.load().lookup(&en, &title), Some("Lead"));
    assert!(host.catalog.load().entity_of_plural("leads").is_some());

    runtime.unload("crm").await.expect("unload");

    for uri in ["/leads".to_string(), format!("/leads/{id}")] {
        let (s, v) = call(&app, Method::GET, &uri, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri} after unload: {v}");
        assert_eq!(v["code"], "this.module.not_found", "{uri}");
    }
    assert!(host.catalog.load().entity_of_plural("leads").is_none());
    assert!(
        host.locales.load().lookup(&en, &title).is_none(),
        "bundles removed with the module"
    );
}

/// Compiled module loaded at runtime: `catalog` (product { price: f64 > 0 }). Its REST
/// route writes the payload straight to its store, like a CLI-generated handler.
struct Catalog(Arc<Store>);
struct Products(Arc<Store>);

impl EntityDescriptor for Products {
    fn entity_type(&self) -> &str {
        "product"
    }
    fn plural(&self) -> &str {
        "products"
    }
    fn build_routes(&self) -> Router {
        let store = self.0.clone();
        Router::new().route(
            "/products",
            axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
                let store = store.clone();
                async move {
                    let created = store.create_from_json(v).await.unwrap();
                    (StatusCode::CREATED, axum::Json(created))
                }
            }),
        )
    }
}

impl this::core::module::Module for Catalog {
    fn name(&self) -> &str {
        "catalog"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["product"]
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
        registry.register(Box::new(Products(self.0.clone())));
    }
    fn get_entity_fetcher(&self, _: &str) -> Option<Arc<dyn EntityFetcher>> {
        Some(self.0.clone())
    }
    fn get_entity_creator(&self, _: &str) -> Option<Arc<dyn EntityCreator>> {
        Some(self.0.clone())
    }
    fn descriptor(&self) -> Option<ModuleDescriptor> {
        let mut e = EntityDesc::new("product");
        e.fields.push(
            FieldDesc::new("price", FieldType::F64)
                .validator(ValidatorSpec::Positive, Applies::Always),
        );
        let mut d = ModuleDescriptor::new("catalog", "1.0.0");
        d.entities.push(e);
        Some(d)
    }
}

#[tokio::test]
async fn hot_loaded_compiled_module_is_validated_on_rest_and_through_its_creator() {
    let store = Arc::new(Store::default());
    let host = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .with_module_runtime()
        .without_authorization()
        .build_host()
        .expect("host")
        .with_auth_provider(Arc::new(HeaderAuth));
    let host = Arc::new(host);
    let app = RestExposure::build_router(host.clone(), vec![]).expect("router");
    host.module_runtime
        .clone()
        .expect("runtime")
        .load(ModuleSource::Compiled(Arc::new(Catalog(store.clone()))))
        .await
        .expect("load");

    // REST: the dispatch layer is outside the host's validation layer; the module's
    // routes carry their own.
    let (s, v) = call(
        &app,
        Method::POST,
        "/products",
        Some(json!({"price": -3.0})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["fields"][0]["code"], "this.validation.positive");
    assert_eq!(
        *store.writes.lock().unwrap(),
        0,
        "the store is never reached"
    );

    // GraphQL / gRPC / link handlers reach the module through the catalog creator.
    let creator = host.catalog.load().creators["product"].clone();
    let err = creator
        .create_from_json(json!({"price": -3.0}))
        .await
        .expect_err("validated");
    assert_eq!(
        this::core::errors::from_anyhow(&err).code.as_str(),
        "this.validation.failed"
    );
    assert_eq!(*store.writes.lock().unwrap(), 0);

    let (s, v) = call(&app, Method::POST, "/products", Some(json!({"price": 3.0}))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(*store.writes.lock().unwrap(), 1);
}

// ── hot-loaded module on REST + GraphQL + gRPC, policies ARMED ───────────────

/// `ledger { entry { amount: f64 } }`: every CRUD operation requires the `admin` role.
fn ledger() -> ModuleDescriptor {
    let mut e = EntityDesc::new("entry");
    e.fields.push(FieldDesc::new("amount", FieldType::F64));
    for op in Operation::CRUD {
        e.permissions
            .ops
            .insert(op, PolicyExpr::HasRole(vec!["admin".into()]));
    }
    let mut m = ModuleDescriptor::new("ledger", "1.0.0");
    m.entities.push(e);
    m.normalize();
    m
}

struct Armed {
    host: Arc<ServerHost>,
    rest: Router,
    graphql: Router,
}

fn armed_runtime_host() -> Armed {
    let host = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .with_module_runtime()
        .build_host()
        .expect("host")
        .with_auth_provider(Arc::new(Tokens));
    assert!(host.authorize, "policies are armed by default");
    let host = Arc::new(host);
    Armed {
        rest: RestExposure::build_router(host.clone(), vec![]).expect("REST"),
        graphql: GraphQLExposure::build_router(host.clone()).expect("GraphQL armed"),
        host,
    }
}

impl Armed {
    async fn rest(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"));
        let req = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                req.body(Body::from(b.to_string()))
            }
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.rest.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn gql(&self, token: &str, query: &str) -> Value {
        let res = self
            .graphql
            .clone()
            .oneshot(
                Request::post("/graphql")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(json!({ "query": query }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn sdl(&self) -> String {
        let res = self
            .graphql
            .clone()
            .oneshot(Request::get("/graphql/schema").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn grpc_req<T>(&self, token: &str, msg: T) -> tonic::Request<T> {
        let mut req = tonic::Request::new(msg);
        req.metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        req
    }

    async fn grpc_create(
        &self,
        token: &str,
        entity: &str,
        body: Value,
    ) -> Result<(), tonic::Status> {
        EntityServiceImpl::new(self.host.clone())
            .create_entity(self.grpc_req(
                token,
                CreateEntityRequest {
                    entity_type: entity.into(),
                    data: Some(grpc_struct(body)),
                },
            ))
            .await
            .map(|_| ())
    }

    async fn grpc_list(&self, token: &str, entity: &str) -> Result<usize, tonic::Status> {
        use this::server::exposure::grpc::proto::ListEntitiesRequest;
        EntityServiceImpl::new(self.host.clone())
            .list_entities(self.grpc_req(
                token,
                ListEntitiesRequest {
                    entity_type: entity.into(),
                    limit: 10,
                    offset: 0,
                },
            ))
            .await
            .map(|r| r.into_inner().entities.len())
    }
}

/// Records a failed expectation instead of aborting: ONE run reports every surface that
/// does not follow the live catalog (a red proof names REST / GraphQL / gRPC separately).
fn ck(failures: &mut Vec<String>, ok: bool, what: impl FnOnce() -> String) {
    if !ok {
        failures.push(what());
    }
}

fn err_code(v: &Value) -> &str {
    v["errors"][0]["extensions"]["this_code"]
        .as_str()
        .unwrap_or("")
}

#[tokio::test]
async fn hot_loaded_module_served_on_rest_graphql_grpc() {
    let a = armed_runtime_host();
    let runtime = a.host.module_runtime.clone().expect("runtime");
    let mut f: Vec<String> = vec![];
    let create_gql = r#"mutation { createEntry(data: {name: "n", amount: 2.0}) { id } }"#;
    let body = json!({"name": "n", "amount": 1.0});

    // Before the load: nothing is served on any surface.
    ck(&mut f, !a.sdl().await.contains("type Entry"), || {
        "SDL has Entry before load".into()
    });
    let (s, _) = a.rest(Method::GET, "/entries", "admin", None).await;
    ck(&mut f, s.is_client_error(), || {
        format!("REST before load: {s}")
    });
    let out = a.gql("admin", "{ entries { id } }").await;
    ck(&mut f, out["errors"].is_array(), || {
        format!("GraphQL before load: {out}")
    });
    let st = a
        .grpc_create("admin", "entry", body.clone())
        .await
        .unwrap_err();
    ck(&mut f, st.code() == tonic::Code::NotFound, || {
        format!("gRPC before load: {st:?}")
    });

    runtime
        .load(ModuleSource::Manifest(ledger().to_json_pretty()))
        .await
        .expect("hot load");

    // REST: refused without the role, served with it.
    let (s, v) = a
        .rest(Method::POST, "/entries", "user", Some(body.clone()))
        .await;
    ck(&mut f, s == StatusCode::FORBIDDEN, || {
        format!("REST user create: {s} {v}")
    });
    let (s, v) = a
        .rest(Method::POST, "/entries", "admin", Some(body.clone()))
        .await;
    ck(&mut f, s == StatusCode::CREATED, || {
        format!("REST admin create: {s} {v}")
    });

    // GraphQL: schema and resolvers follow the live catalog.
    let sdl = a.sdl().await;
    ck(
        &mut f,
        sdl.contains("type Entry") && sdl.contains("entries"),
        || format!("GraphQL SDL after load:\n{sdl}"),
    );
    let out = a.gql("user", create_gql).await;
    ck(&mut f, err_code(&out) == "this.auth.forbidden", || {
        format!("GraphQL user create: {out}")
    });
    let out = a.gql("user", "{ entries { id } }").await;
    ck(&mut f, err_code(&out) == "this.auth.forbidden", || {
        format!("GraphQL user list: {out}")
    });
    let out = a.gql("admin", create_gql).await;
    ck(&mut f, out["errors"].is_null(), || {
        format!("GraphQL admin create: {out}")
    });
    let out = a.gql("admin", "{ entries { id } }").await;
    ck(
        &mut f,
        out["data"]["entries"].as_array().map(Vec::len) == Some(2),
        || format!("GraphQL admin list (expected 2 rows): {out}"),
    );

    // gRPC: entity service and exported proto follow the live catalog.
    let st = a.grpc_create("user", "entry", body.clone()).await;
    ck(
        &mut f,
        st.as_ref().err().map(|s| s.code()) == Some(tonic::Code::PermissionDenied),
        || format!("gRPC user create: {st:?}"),
    );
    let r = a.grpc_create("admin", "entry", body.clone()).await;
    ck(&mut f, r.is_ok(), || format!("gRPC admin create: {r:?}"));
    let r = a.grpc_list("admin", "entry").await;
    ck(&mut f, r.as_ref().ok() == Some(&3), || {
        format!("gRPC admin list (expected 3 rows): {r:?}")
    });
    let r = a.grpc_list("user", "entry").await;
    ck(
        &mut f,
        r.as_ref().err().map(|s| s.code()) == Some(tonic::Code::PermissionDenied),
        || format!("gRPC user list: {r:?}"),
    );
    let proto = ProtoGenerator::new(a.host.clone()).generate_proto().await;
    ck(&mut f, proto.contains("message Entry"), || {
        format!("proto after load:\n{proto}")
    });

    // Unload: 404 / unknown everywhere; schema and proto back to before.
    runtime.unload("ledger").await.expect("unload");
    let (s, v) = a.rest(Method::GET, "/entries", "admin", None).await;
    ck(
        &mut f,
        s == StatusCode::NOT_FOUND && v["code"] == "this.module.not_found",
        || format!("REST after unload: {s} {v}"),
    );
    let out = a.gql("admin", "{ entries { id } }").await;
    ck(
        &mut f,
        out["data"].is_null() && out["errors"].is_array(),
        || format!("GraphQL list after unload: {out}"),
    );
    let out = a.gql("admin", create_gql).await;
    ck(&mut f, out["errors"].is_array(), || {
        format!("GraphQL create after unload: {out}")
    });
    let r = a.grpc_list("admin", "entry").await;
    ck(
        &mut f,
        r.as_ref().err().map(|s| s.code()) == Some(tonic::Code::NotFound),
        || format!("gRPC after unload: {r:?}"),
    );
    ck(&mut f, !a.sdl().await.contains("type Entry"), || {
        "SDL still has Entry after unload".into()
    });
    let proto = ProtoGenerator::new(a.host.clone()).generate_proto().await;
    ck(&mut f, !proto.contains("message Entry"), || {
        "proto still has Entry after unload".into()
    });

    assert!(
        f.is_empty(),
        "{} expectation(s) failed:\n- {}",
        f.len(),
        f.join("\n- ")
    );
}
