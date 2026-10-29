//! Validation and typed filters in the COMMON pipeline (RFC module-descriptor §3.5):
//! the same invalid payload is refused on REST, GraphQL and gRPC with the same contract
//! code, the store is never reached, and list filters/sorts are really applied (a
//! malformed one is a typed error, never ignored). An entity WITHOUT descriptor on the
//! same host keeps its historical behaviour.
//!
//! Run: `cargo test -p this-rs --features graphql,grpc --test validation_cross_protocol`.

#![cfg(all(feature = "graphql", feature = "grpc"))]

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::routing::post;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::{EntityCreator, EntityFetcher};
use this::descriptor::{
    Applies, DescriptorSet, EntityDesc, FieldDesc, FieldType, FilterOp, ModuleDescriptor,
    TransformSpec, ValidatorSpec,
};
use this::server::GraphQLExposure;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::grpc::entity_service::{
    EntityServiceImpl, FILTER_METADATA, SORT_METADATA,
};
use this::server::exposure::grpc::proto::entity_service_server::EntityService;
use this::server::exposure::grpc::proto::{CreateEntityRequest, ListEntitiesRequest};
use this::server::exposure::rest::RestExposure;
use this::server::host::ServerHost;
use this::storage::InMemoryLinkService;
use tower::ServiceExt;
use uuid::Uuid;

/// In-memory store: fetcher + creator, records every write that reaches it.
#[derive(Default)]
struct Store {
    rows: Mutex<Vec<Value>>,
    writes: Mutex<usize>,
}

#[async_trait]
impl EntityCreator for Store {
    async fn create_from_json(&self, mut v: Value) -> anyhow::Result<Value> {
        *self.writes.lock().unwrap() += 1;
        v["id"] = json!(Uuid::new_v4().to_string());
        self.rows.lock().unwrap().push(v.clone());
        Ok(v)
    }
}

#[async_trait]
impl EntityFetcher for Store {
    async fn fetch_as_json(&self, id: &Uuid) -> anyhow::Result<Value> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("not found"))
    }
    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> anyhow::Result<Vec<Value>> {
        let rows = self.rows.lock().unwrap().clone();
        let skip = offset.unwrap_or(0).max(0) as usize;
        let take = limit.map(|l| l.max(0) as usize).unwrap_or(usize::MAX);
        Ok(rows.into_iter().skip(skip).take(take).collect())
    }
}

/// REST entity routes as the CLI generates them: `Json<Value>` straight to the store.
struct Routes {
    singular: &'static str,
    plural: &'static str,
    store: Arc<Store>,
}

impl EntityDescriptor for Routes {
    fn entity_type(&self) -> &str {
        self.singular
    }
    fn plural(&self) -> &str {
        self.plural
    }
    fn build_routes(&self) -> Router {
        let store = self.store.clone();
        Router::new().route(
            &format!("/{}", self.plural),
            post(move |axum::Json(v): axum::Json<Value>| {
                let store = store.clone();
                async move {
                    let created = store.create_from_json(v).await.unwrap();
                    axum::Json(created)
                }
            }),
        )
    }
}

fn billing() -> ModuleDescriptor {
    let mut e = EntityDesc::new("invoice");
    e.fields = vec![
        FieldDesc::new("number", FieldType::String)
            .transform(TransformSpec::Trim, Applies::Always)
            .transform(TransformSpec::Uppercase, Applies::Always)
            .validator(
                ValidatorSpec::StringLength { min: 3, max: 50 },
                Applies::Always,
            )
            .filterable(&[FilterOp::Eq])
            .sortable(),
        FieldDesc::new("amount", FieldType::F64)
            .validator(ValidatorSpec::Positive, Applies::Always)
            .filterable(&[FilterOp::Gt, FilterOp::Eq])
            .sortable(),
        FieldDesc::new("note", FieldType::String).optional(),
    ];
    let mut m = ModuleDescriptor::new("billing", "1.0.0");
    m.entities.push(e);
    m
}

struct Fixture {
    host: Arc<ServerHost>,
    invoices: Arc<Store>,
    orders: Arc<Store>,
}

fn fixture() -> Fixture {
    let invoices = Arc::new(Store::default());
    let orders = Arc::new(Store::default());
    let config = LinksConfig {
        entities: ["invoice", "order"]
            .iter()
            .map(|s| EntityConfig {
                singular: s.to_string(),
                plural: format!("{s}s"),
                auth: EntityAuthConfig::default(),
            })
            .collect(),
        links: vec![],
        validation_rules: None,
        events: None,
        sinks: None,
        auth: None,
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Routes {
        singular: "invoice",
        plural: "invoices",
        store: invoices.clone(),
    }));
    registry.register(Box::new(Routes {
        singular: "order",
        plural: "orders",
        store: orders.clone(),
    }));
    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert("invoice".into(), invoices.clone());
    fetchers.insert("order".into(), orders.clone());
    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert("invoice".into(), invoices.clone());
    creators.insert("order".into(), orders.clone());

    let mut set = DescriptorSet::new();
    set.insert(billing()).unwrap();
    let mut host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        config,
        registry,
        fetchers,
        creators,
    )
    .unwrap()
    .with_descriptors(set);
    // No `auth:` declared in this fixture: the policy layer is not under test here.
    host.authorize = false;
    Fixture {
        host: Arc::new(host),
        invoices,
        orders,
    }
}

async fn send(app: Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let res = app
        .oneshot(
            HttpRequest::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn graphql(host: &Arc<ServerHost>, query: &str) -> Value {
    let app = GraphQLExposure::build_router_unenforced(host.clone()).unwrap();
    send(app, "POST", "/graphql", json!({ "query": query }))
        .await
        .1
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

async fn grpc_create(
    host: &Arc<ServerHost>,
    entity: &str,
    data: Value,
) -> Result<(), tonic::Status> {
    EntityServiceImpl::new(host.clone())
        .create_entity(tonic::Request::new(CreateEntityRequest {
            entity_type: entity.into(),
            data: Some(grpc_struct(data)),
        }))
        .await
        .map(|_| ())
}

#[tokio::test]
async fn validating_creator_blocks_invalid_graphql_payload() {
    let f = fixture();
    let out = graphql(
        &f.host,
        r#"mutation { createInvoice(data: {number: "abc", amount: -1}) { id } }"#,
    )
    .await;
    let err = &out["errors"][0];
    assert_eq!(err["extensions"]["code"], "BAD_USER_INPUT", "{out}");
    assert_eq!(err["extensions"]["this_code"], "this.validation.failed");
    assert_eq!(
        err["extensions"]["fields"][0]["code"],
        "this.validation.positive"
    );
    assert_eq!(*f.invoices.writes.lock().unwrap(), 0, "store never reached");

    // Absent required field (`amount`) is detected, not silently defaulted.
    let out = graphql(
        &f.host,
        r#"mutation { createInvoice(data: {number: "abc"}) { id } }"#,
    )
    .await;
    assert_eq!(
        out["errors"][0]["extensions"]["fields"][0]["code"], "this.validation.required",
        "{out}"
    );

    // Valid payload: transformed THEN stored.
    let out = graphql(
        &f.host,
        r#"mutation { createInvoice(data: {number: " ab1 ", amount: 2}) { id } }"#,
    )
    .await;
    assert!(out.get("errors").is_none(), "{out}");
    assert_eq!(f.invoices.rows.lock().unwrap()[0]["number"], "AB1");
}

#[tokio::test]
async fn validating_creator_blocks_invalid_grpc_payload() {
    let f = fixture();
    let st = grpc_create(&f.host, "invoice", json!({"number": "abc", "amount": -1}))
        .await
        .unwrap_err();
    assert_eq!(st.code(), tonic::Code::InvalidArgument);
    let details: Value = serde_json::from_slice(st.details()).unwrap();
    assert_eq!(details["code"], "this.validation.failed");
    assert_eq!(details["fields"][0]["code"], "this.validation.positive");
    // Unknown key: fail-closed.
    let st = grpc_create(
        &f.host,
        "invoice",
        json!({"number": "abc", "amount": 1, "zz": 1}),
    )
    .await
    .unwrap_err();
    let details: Value = serde_json::from_slice(st.details()).unwrap();
    assert_eq!(
        details["fields"][0]["code"],
        "this.validation.unknown_field"
    );
    assert_eq!(*f.invoices.writes.lock().unwrap(), 0);
    grpc_create(&f.host, "invoice", json!({"number": "abc", "amount": 1}))
        .await
        .unwrap();
    assert_eq!(*f.invoices.writes.lock().unwrap(), 1);
}

#[tokio::test]
async fn same_invalid_payload_same_code_on_rest_graphql_grpc() {
    let f = fixture();
    let bad = json!({"number": "abc", "amount": -1});
    let rest = RestExposure::build_router(f.host.clone(), vec![]).unwrap();
    let (status, body) = send(rest, "POST", "/invoices", bad.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let rest_code = body["fields"][0]["code"].clone();

    let gql = graphql(
        &f.host,
        r#"mutation { createInvoice(data: {number: "abc", amount: -1}) { id } }"#,
    )
    .await;
    let gql_code = gql["errors"][0]["extensions"]["fields"][0]["code"].clone();

    let st = grpc_create(&f.host, "invoice", bad).await.unwrap_err();
    let grpc_code =
        serde_json::from_slice::<Value>(st.details()).unwrap()["fields"][0]["code"].clone();

    assert_eq!(rest_code, json!("this.validation.positive"));
    assert_eq!(gql_code, rest_code);
    assert_eq!(grpc_code, rest_code);
    assert_eq!(*f.invoices.writes.lock().unwrap(), 0);
}

#[tokio::test]
async fn entity_without_descriptor_keeps_historical_behaviour() {
    let f = fixture();
    // Any payload: no descriptor, no rule.
    let rest = RestExposure::build_router(f.host.clone(), vec![]).unwrap();
    let (status, _) = send(rest, "POST", "/orders", json!({"whatever": -1})).await;
    assert_eq!(status, StatusCode::OK);
    let out = graphql(
        &f.host,
        r#"mutation { createOrder(data: {whatever: -1}) { id } }"#,
    )
    .await;
    assert!(out.get("errors").is_none(), "{out}");
    grpc_create(&f.host, "order", json!({"whatever": -1}))
        .await
        .unwrap();
    assert_eq!(*f.orders.writes.lock().unwrap(), 3);
}

async fn seed(f: &Fixture) {
    for (n, a) in [("AAA", 5.0), ("BBB", 50.0), ("CCC", 500.0), ("DDD", 7.0)] {
        f.invoices
            .rows
            .lock()
            .unwrap()
            .push(json!({"id": Uuid::new_v4().to_string(), "number": n, "amount": a}));
    }
}

#[tokio::test]
async fn graphql_list_applies_filter_and_sort_and_rejects_malformed() {
    let f = fixture();
    seed(&f).await;
    let out = graphql(
        &f.host,
        r#"{ invoices(filter: {amountGt: 0}, sort: "amount") { number } }"#,
    )
    .await;
    // Unknown filter field `amountGt`: typed error, not an unfiltered list.
    assert_eq!(
        out["errors"][0]["extensions"]["this_code"], "this.query.field_not_filterable",
        "{out}"
    );

    let out = graphql(
        &f.host,
        r#"{ invoices(filter: "{\"amount>\": 6}", sort: "amount:desc", limit: 2) { number } }"#,
    )
    .await;
    assert!(out.get("errors").is_none(), "{out}");
    let numbers: Vec<&str> = out["data"]["invoices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["number"].as_str().unwrap())
        .collect();
    assert_eq!(
        numbers,
        vec!["CCC", "BBB"],
        "filter before pagination, then sort"
    );

    let out = graphql(&f.host, r#"{ invoices(filter: "{oops") { number } }"#).await;
    assert_eq!(
        out["errors"][0]["extensions"]["this_code"], "this.query.malformed_filter",
        "{out}"
    );
    let out = graphql(&f.host, r#"{ invoices(sort: "note") { number } }"#).await;
    assert_eq!(
        out["errors"][0]["extensions"]["this_code"], "this.query.field_not_sortable",
        "{out}"
    );
}

#[tokio::test]
async fn grpc_list_applies_filter_and_sort_from_metadata() {
    let f = fixture();
    seed(&f).await;
    let svc = EntityServiceImpl::new(f.host.clone());
    let list = |filter: &str, sort: &str| {
        let mut req = tonic::Request::new(ListEntitiesRequest {
            entity_type: "invoice".into(),
            limit: 0,
            offset: 0,
        });
        req.metadata_mut()
            .insert(FILTER_METADATA, filter.parse().unwrap());
        req.metadata_mut()
            .insert(SORT_METADATA, sort.parse().unwrap());
        req
    };
    let res = svc
        .list_entities(list(r#"{"amount>": 6}"#, "-amount"))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(res.total, 3);
    let first = &res.entities[0].fields["number"];
    assert_eq!(
        first.kind,
        Some(prost_types::value::Kind::StringValue("CCC".into()))
    );
    let st = svc
        .list_entities(list("{oops", "amount"))
        .await
        .unwrap_err();
    assert_eq!(st.code(), tonic::Code::InvalidArgument);
    let st = svc.list_entities(list("{}", "note")).await.unwrap_err();
    assert_eq!(st.code(), tonic::Code::InvalidArgument);
}
