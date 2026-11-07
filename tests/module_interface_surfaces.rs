//! ONE declared module, served everywhere: its entity, its INTERFACE (views, actions, i18n
//! hints), its declared error codes and its declared links.
//!
//! What this pins (the N4 deliverable of the module wave):
//! - REST serves the entity (`DescriptorEntity` behind the common guard) AND the interface
//!   export (`/_this/views`, `/_this/errors`) that lets an app draw what the module declared
//!   without parsing the raw descriptor;
//! - GraphQL publishes the same document as `_thisViews`, and the links a module DECLARES as
//!   relation fields (sanitized to valid GraphQL names);
//! - a module's business code is mapped by its DECLARATION on all three structured surfaces
//!   — the store raises it with the wrong kind and every surface still answers the declared
//!   status — and a code no module declares is a named refusal, never an anonymous 500;
//! - gRPC serves the entity and its derived proto.
//!
//! Run: `cargo test -p this-rs --features graphql,grpc --test module_interface_surfaces`.

#![cfg(all(feature = "graphql", feature = "grpc"))]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use this::config::LinksConfig;
use this::core::errors::from_anyhow;
use this::core::{EntityCreator, EntityFetcher};
use this::descriptor::{
    ContractError, DescriptorSet, EntityDesc, ErrorKind, ModuleDescriptor, Operation, PolicyExpr,
};
use this::server::RestExposure;
use this::server::descriptor_entity::DescriptorEntity;
use this::server::entity_registry::EntityRegistry;
use this::server::exposure::GraphQLExposure;
use this::server::exposure::grpc::entity_service::EntityServiceImpl;
use this::server::exposure::grpc::proto::CreateEntityRequest;
use this::server::exposure::grpc::proto::entity_service_server::EntityService;
use this::server::exposure::grpc::proto_generator::ProtoGenerator;
use this::server::host::ServerHost;
use this::storage::in_memory::InMemoryLinkService;

// ── fixtures ────────────────────────────────────────────────────────────────

/// `billing`: two entities, one declared link, one declared view, one declared action, and
/// two declared error codes (`already_paid` → conflict, kept for the mapping test).
fn billing() -> ModuleDescriptor {
    let json = json!({
        "name": "billing",
        "version": "1.0.0",
        "i18n": { "default_locale": "fr", "locales": ["fr", "en"] },
        "entities": [
            { "name": "invoice", "plural": "invoices",
              "fields": [{ "name": "amount", "ty": "f64" }],
              "i18n": { "label": "billing.invoice.label",
                        "label_plural": "billing.invoice.plural" } },
            { "name": "payment", "plural": "payments",
              "fields": [{ "name": "amount", "ty": "f64" }] },
            { "name": "receipt", "plural": "receipts",
              "fields": [{ "name": "amount", "ty": "f64" }] }
        ],
        "links": [{ "link_type": "settle", "source": "invoice", "target": "payment" }],
        "errors": [{
            "code": "billing.invoice.already_paid",
            "kind": "conflict",
            "message_key": "billing.invoice.already_paid",
            "params": ["invoice"],
            "description": "The invoice was already paid."
        }],
        "views": [{
            "name": "invoices",
            "surface": "main",
            "title": "Invoices",
            "root": { "type": "leaf", "brick": {
                "type": "list", "entity": "invoice", "label": "amount" } }
        }],
        "actions": [{ "kind": "create", "name": "new_invoice",
                      "entity": "invoice", "fields": ["amount"] }]
    })
    .to_string();
    let mut module = ModuleDescriptor::from_json(&json).expect("descriptor");
    // A module loaded through the runtime must declare its policies explicitly; the four
    // tests that use the descriptor-only path are indifferent (their host is disarmed).
    for entity in &mut module.entities {
        for op in Operation::CRUD {
            entity.permissions.ops.insert(op, PolicyExpr::Authenticated);
        }
    }
    module
}

/// A store whose WRITES raise a contract error, deliberately with the WRONG kind, so the
/// declaration is what decides the status on every surface:
/// - `invoice` raises a DECLARED code (`billing.invoice.already_paid`, declared `conflict`);
/// - `receipt` raises a code NOBODY declares (`billing.receipt.ghost`);
/// - `payment` writes normally.
struct Store {
    entity: String,
}

#[async_trait]
impl EntityFetcher for Store {
    async fn fetch_as_json(&self, id: &Uuid) -> Result<Value> {
        Ok(json!({ "id": id, "amount": 1.0 }))
    }
    async fn list_as_json(&self, _limit: Option<i32>, _offset: Option<i32>) -> Result<Vec<Value>> {
        Ok(vec![json!({ "id": Uuid::nil(), "amount": 1.0 })])
    }
    async fn get_sample_entity(&self) -> Result<Value> {
        Ok(json!({ "id": Uuid::nil(), "amount": 1.0 }))
    }
}

#[async_trait]
impl EntityCreator for Store {
    async fn create_from_json(&self, data: Value) -> Result<Value> {
        match self.entity.as_str() {
            "invoice" => Err(anyhow::Error::new(ContractError::new(
                "billing.invoice.already_paid",
                ErrorKind::Internal,
            ))),
            "receipt" => Err(anyhow::Error::new(ContractError::new(
                "billing.receipt.ghost",
                ErrorKind::Internal,
            ))),
            _ => Ok(data),
        }
    }
}

/// A host carrying the descriptor, the registry entries, the stores, and the policies
/// DISARMED (identity is not what these tests measure; it is proven elsewhere).
fn host() -> Arc<ServerHost> {
    let module = billing();
    let entities: Vec<EntityDesc> = module.entities.clone();
    let mut set = DescriptorSet::new();
    set.insert(module).expect("insert");

    let mut registry = EntityRegistry::new();
    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    for entity in entities {
        let store = Arc::new(Store {
            entity: entity.name.clone(),
        });
        registry.register(Box::new(DescriptorEntity::new(
            entity.clone(),
            store.clone(),
            store.clone(),
        )));
        fetchers.insert(entity.name.clone(), store.clone());
        creators.insert(entity.name, store);
    }

    let mut host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        LinksConfig::default_config(),
        registry,
        fetchers,
        creators,
    )
    .expect("host");
    host.authorize = false;
    Arc::new(host.with_descriptors(set))
}

async fn rest(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = match body {
        Some(body) => Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty()),
    }
    .expect("request");
    let res = app.clone().oneshot(req).await.expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn gql(app: &Router, query: &str) -> Value {
    let res = app
        .clone()
        .oneshot(
            Request::post("/graphql")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "query": query }).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn sdl(app: &Router) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::get("/graphql/schema")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

fn grpc_struct(v: Value) -> prost_types::Struct {
    use prost_types::value::Kind;
    let Value::Object(map) = v else {
        panic!("an object")
    };
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| {
                let kind = match v {
                    Value::String(s) => Kind::StringValue(s),
                    Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or_default()),
                    Value::Bool(b) => Kind::BoolValue(b),
                    _ => Kind::NullValue(0),
                };
                (k, prost_types::Value { kind: Some(kind) })
            })
            .collect(),
    }
}

async fn grpc_create(host: &Arc<ServerHost>, entity: &str) -> tonic::Status {
    EntityServiceImpl::new(host.clone())
        .create_entity(tonic::Request::new(CreateEntityRequest {
            entity_type: entity.into(),
            data: Some(grpc_struct(json!({ "amount": 1.0 }))),
        }))
        .await
        .expect_err("the store refuses every write of this fixture")
}

// ── REST: entity + interface ────────────────────────────────────────────────

#[tokio::test]
async fn rest_serves_the_entity_and_exports_the_interface() {
    let host = host();
    let app = RestExposure::build_router(host.clone(), vec![]).expect("REST");

    // The entity: served, and refused with the DECLARED status (409 `conflict`) even though
    // the store raised the code with kind `internal`.
    let (status, body) = rest(&app, "POST", "/invoices", Some(json!({ "amount": 1.0 }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("billing.invoice.already_paid"));
    assert_eq!(body["kind"], json!("conflict"));

    // A read goes through the fetcher of the live catalog.
    let id = Uuid::nil();
    let (status, body) = rest(&app, "GET", &format!("/invoices/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The interface: what to draw, where, what it replaces, and what the bricks read.
    let (status, views) = rest(&app, "GET", "/_this/views", None).await;
    assert_eq!(status, StatusCode::OK, "{views}");
    let view = &views["views"][0];
    assert_eq!(view["module"], json!("billing"));
    assert_eq!(view["name"], json!("invoices"));
    assert_eq!(view["surface"], json!("main"));
    assert_eq!(view["title"], json!("Invoices"));
    // The entity the view reads is resolved for the app: plural, REST path, i18n label keys,
    // and `served` read from the live catalog (this host does serve it).
    assert_eq!(view["entities"][0]["name"], json!("invoice"));
    assert_eq!(view["entities"][0]["plural"], json!("invoices"));
    assert_eq!(view["entities"][0]["path"], json!("/invoices"));
    assert_eq!(view["entities"][0]["served"], json!(true));
    assert_eq!(view["entities"][0]["label"], json!("billing.invoice.label"));
    // Locale hints and actions, so the app knows which bundle to fetch and what buttons mean.
    assert_eq!(views["modules"][0]["default_locale"], json!("fr"));
    assert_eq!(views["actions"][0]["name"], json!("new_invoice"));
    assert_eq!(views["actions"][0]["action"]["entity"], json!("invoice"));

    // The declared error catalogue: the mapping of every code, per surface.
    let (status, errors) = rest(&app, "GET", "/_this/errors", None).await;
    assert_eq!(status, StatusCode::OK, "{errors}");
    let paid = errors["errors"]
        .as_array()
        .expect("errors")
        .iter()
        .find(|e| e["code"] == json!("billing.invoice.already_paid"))
        .expect("the declared code is exported");
    assert_eq!(paid["module"], json!("billing"));
    assert_eq!(paid["http_status"], json!(409));
    assert_eq!(paid["grpc_name"], json!("ALREADY_EXISTS"));
    assert_eq!(paid["graphql_code"], json!("CONFLICT"));
}

// ── GraphQL: interface + declared links as relations ────────────────────────

#[tokio::test]
async fn graphql_serves_the_views_document_and_the_declared_relations() {
    let host = host();
    let app = GraphQLExposure::build_router(host.clone()).expect("GraphQL");

    let schema = sdl(&app).await;
    assert!(
        schema.contains("_thisViews"),
        "SDL lacks the interface export:\n{schema}"
    );
    // A module link, declared with the REST route grammar (`payments-settles`), is published
    // as a GraphQL field that is actually SPELLABLE: the dash is refused by GraphQL.
    assert!(
        schema.contains("payments_settles: [Payment!]!"),
        "forward relation missing from `type Invoice`:\n{schema}"
    );
    assert!(
        schema.contains("invoices_settles: Invoice"),
        "reverse relation missing from `type Payment`:\n{schema}"
    );

    // The same document as `/_this/views`, read without the raw descriptor.
    let out = gql(&app, "{ _thisViews }").await;
    assert!(out["errors"].is_null(), "{out}");
    let views = &out["data"]["_thisViews"];
    assert_eq!(views["views"][0]["module"], json!("billing"));
    assert_eq!(views["views"][0]["name"], json!("invoices"));
    assert_eq!(views["views"][0]["entities"][0]["path"], json!("/invoices"));

    // Filters, like the REST query string.
    let out = gql(&app, r#"{ _thisViews(surface: "header") }"#).await;
    assert_eq!(out["data"]["_thisViews"]["views"], json!([]), "{out}");
    let out = gql(&app, r#"{ _thisViews(module: "nope") }"#).await;
    assert_eq!(
        out["errors"][0]["extensions"]["this_code"],
        json!("this.entity.not_found"),
        "{out}"
    );

    // A host WITHOUT descriptors keeps its document unchanged: the field (and the `JSON`
    // scalar it returns) is declared only when there is something to export.
    let bare = Arc::new(
        ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .expect("host"),
    );
    let bare_schema = sdl(&GraphQLExposure::build_router(bare).expect("GraphQL")).await;
    assert!(
        !bare_schema.contains("_thisViews"),
        "a host without descriptors must not announce an empty export:\n{bare_schema}"
    );
}

// ── error codes: the declaration decides, on every surface ─────────────────

#[tokio::test]
async fn one_declared_code_gets_the_same_status_on_rest_graphql_and_grpc() {
    let host = host();
    let rest_app = RestExposure::build_router(host.clone(), vec![]).expect("REST");
    let gql_app = GraphQLExposure::build_router(host.clone()).expect("GraphQL");

    // REST: 409 (declared `conflict`), not 500.
    let (status, body) = rest(
        &rest_app,
        "POST",
        "/invoices",
        Some(json!({ "amount": 1.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        from_anyhow(&anyhow::anyhow!("x")).code.as_str(),
        "this.internal"
    );

    // GraphQL: `CONFLICT` + the module's code in `this_code`.
    let out = gql(
        &gql_app,
        r#"mutation { createInvoice(data: {amount: 1.0}) { id } }"#,
    )
    .await;
    assert_eq!(
        out["errors"][0]["extensions"]["code"],
        json!("CONFLICT"),
        "{out}"
    );
    assert_eq!(
        out["errors"][0]["extensions"]["this_code"],
        json!("billing.invoice.already_paid"),
        "{out}"
    );

    // gRPC: ALREADY_EXISTS.
    let status = grpc_create(&host, "invoice").await;
    assert_eq!(status.code(), tonic::Code::AlreadyExists, "{status:?}");

    // An UNDECLARED module code is a NAMED refusal everywhere, never an anonymous 500.
    let (status, body) = rest(
        &rest_app,
        "POST",
        "/receipts",
        Some(json!({ "amount": 1.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "{body}");
    assert_eq!(body["code"], json!("this.module.reference_unknown"));
    assert!(
        body.to_string().contains("billing"),
        "the refusal names the module: {body}"
    );

    let out = gql(
        &gql_app,
        r#"mutation { createReceipt(data: {amount: 1.0}) { id } }"#,
    )
    .await;
    assert_eq!(
        out["errors"][0]["extensions"]["this_code"],
        json!("this.module.reference_unknown"),
        "{out}"
    );

    let status = grpc_create(&host, "receipt").await;
    assert_eq!(status.code(), tonic::Code::FailedPrecondition, "{status:?}");
}

// ── the interface follows the LIVE catalog (hot load / unload) ─────────────

#[tokio::test]
async fn the_interface_export_follows_a_hot_loaded_module() {
    use this::server::builder::ServerBuilder;
    use this::server::module_runtime::ModuleSource;

    // An ARMED host refuses the interface export to an anonymous caller, exactly like every
    // other `/_this/*` route: the view catalogue describes the application's structure, so
    // it inherits `Descriptor · read` (`authenticated`) without any code of its own.
    let armed = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .build_host()
        .expect("host");
    assert!(armed.authorize, "policies are armed by default");
    let (status, body) = rest(
        &RestExposure::build_router(Arc::new(armed), vec![]).expect("REST"),
        "GET",
        "/_this/views",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["scope"], json!("Descriptor · read"), "{body}");

    // A real host with a module runtime, policies disarmed: the app can ask what a module
    // declares WITHOUT reading the descriptor, and the answer appears when the module is
    // loaded and disappears when it is unloaded.
    let host = ServerBuilder::new()
        .with_module_runtime()
        .with_hot_load_enabled(true)
        .with_link_service(InMemoryLinkService::new())
        .without_authorization()
        .build_host()
        .expect("host");
    let host = Arc::new(host);
    let app = RestExposure::build_router(host.clone(), vec![]).expect("REST");

    let (status, before) = rest(&app, "GET", "/_this/views", None).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(before["views"], json!([]));

    let runtime = host.module_runtime.clone().expect("runtime");
    runtime
        .load(ModuleSource::Manifest(billing().to_json_pretty()))
        .await
        .expect("hot load");

    let (status, loaded) = rest(&app, "GET", "/_this/views", None).await;
    assert_eq!(status, StatusCode::OK, "{loaded}");
    assert_eq!(loaded["views"][0]["module"], json!("billing"), "{loaded}");
    assert_eq!(loaded["views"][0]["name"], json!("invoices"));
    // A declarative module serves its own entity: the export says so.
    assert_eq!(loaded["views"][0]["entities"][0]["served"], json!(true));
    let (status, errors) = rest(&app, "GET", "/_this/errors/billing", None).await;
    assert_eq!(status, StatusCode::OK, "{errors}");
    assert!(
        errors["errors"]
            .as_array()
            .expect("errors")
            .iter()
            .any(|e| e["code"] == json!("billing.invoice.already_paid")),
        "{errors}"
    );

    runtime.unload("billing").await.expect("unload");
    let (status, after) = rest(&app, "GET", "/_this/views", None).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["views"], json!([]), "{after}");
    assert_eq!(after["actions"], json!([]), "{after}");
    let (status, after) = rest(&app, "GET", "/_this/errors/billing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{after}");
}

// ── gRPC: the entity, and the proto derived from the declaration ────────────

#[tokio::test]
async fn grpc_and_its_proto_follow_the_declaration() {
    let host = host();
    let proto = ProtoGenerator::new(host.clone()).generate_proto().await;
    // A typed message per declared entity, from the descriptor (no JSON sample needed).
    assert!(proto.contains("message Invoice"), "{proto}");
    assert!(proto.contains("message Payment"), "{proto}");
    // Links are carried by the generic `LinkService` (keyed by `link_type`), on every host:
    // this-rs' gRPC wire is `google.protobuf.Struct`, so there is no typed message per link.
    assert!(proto.contains("service LinkService"), "{proto}");

    // The declared entity is listed through the live catalog (empty store → empty list).
    let listed = EntityServiceImpl::new(host.clone())
        .list_entities(tonic::Request::new(
            this::server::exposure::grpc::proto::ListEntitiesRequest {
                entity_type: "invoice".into(),
                limit: 10,
                offset: 0,
            },
        ))
        .await
        .expect("listed");
    assert_eq!(listed.into_inner().entities.len(), 1);
}
