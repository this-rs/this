//! A write emits exactly ONE `EntityEvent`, whatever the protocol (REST, GraphQL, gRPC):
//! the event is published by the shared write seam (`SurfaceGuard::creator`), not by
//! the handlers.
//!
//! Run: `cargo test -p this-rs --features graphql,grpc --test event_iso_cross_protocol`.

#![cfg(all(feature = "graphql", feature = "grpc"))]

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::events::{EntityEvent, EventBus, FrameworkEvent};
use this::core::{EntityCreator, EntityFetcher};
use this::descriptor::{DescriptorSet, EntityDesc, FieldDesc, FieldType, ModuleDescriptor};
use this::server::GraphQLExposure;
use this::server::descriptor_entity::DescriptorEntity;
use this::server::entity_registry::EntityRegistry;
use this::server::exposure::grpc::entity_service::EntityServiceImpl;
use this::server::exposure::grpc::proto::entity_service_server::EntityService;
use this::server::exposure::grpc::proto::{
    CreateEntityRequest, DeleteEntityRequest, UpdateEntityRequest,
};
use this::server::exposure::rest::RestExposure;
use this::server::host::ServerHost;
use this::storage::InMemoryLinkService;
use tokio::sync::broadcast::Receiver;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Default)]
struct Store {
    rows: Mutex<Vec<Value>>,
}

#[async_trait]
impl EntityCreator for Store {
    async fn create_from_json(&self, mut v: Value) -> anyhow::Result<Value> {
        v["id"] = json!(Uuid::new_v4().to_string());
        self.rows.lock().unwrap().push(v.clone());
        Ok(v)
    }
    async fn update_from_json(&self, id: &Uuid, v: Value) -> anyhow::Result<Value> {
        let mut rows = self.rows.lock().unwrap();
        let row = rows
            .iter_mut()
            .find(|r| r["id"] == json!(id.to_string()))
            .ok_or_else(|| anyhow::anyhow!("not found"))?;
        if let (Some(row), Value::Object(patch)) = (row.as_object_mut(), v) {
            row.extend(patch);
        }
        Ok(row.clone())
    }
    async fn delete(&self, id: &Uuid) -> anyhow::Result<()> {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|r| r["id"] != json!(id.to_string()));
        anyhow::ensure!(rows.len() != before, "not found");
        Ok(())
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
    async fn list_as_json(&self, _l: Option<i32>, _o: Option<i32>) -> anyhow::Result<Vec<Value>> {
        Ok(self.rows.lock().unwrap().clone())
    }
}

fn host(with_bus: bool) -> Arc<ServerHost> {
    let store = Arc::new(Store::default());
    let mut entity = EntityDesc::new("invoice");
    entity.fields = vec![FieldDesc::new("number", FieldType::String)];
    let mut module = ModuleDescriptor::new("billing", "1.0.0");
    module.entities.push(entity.clone());
    let mut set = DescriptorSet::new();
    set.insert(module).unwrap();

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
        auth: None,
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(DescriptorEntity::new(
        entity,
        store.clone(),
        store.clone(),
    )));
    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert("invoice".into(), store.clone());
    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert("invoice".into(), store);
    let mut h = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        config,
        registry,
        fetchers,
        creators,
    )
    .unwrap()
    .with_descriptors(set);
    if with_bus {
        h = h.with_event_bus(EventBus::new(64));
    }
    h.authorize = false;
    Arc::new(h)
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

async fn rest(h: &Arc<ServerHost>, m: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    send(
        RestExposure::build_router(h.clone(), vec![]).unwrap(),
        m,
        uri,
        body,
    )
    .await
}

async fn gql(h: &Arc<ServerHost>, q: &str) -> Value {
    let app = GraphQLExposure::build_router_unenforced(h.clone()).unwrap();
    let out = send(app, "POST", "/graphql", json!({ "query": q })).await.1;
    assert!(out.get("errors").is_none(), "{out}");
    out
}

fn st(v: Value) -> prost_types::Struct {
    use prost_types::value::Kind;
    let Value::Object(map) = v else { panic!() };
    prost_types::Struct {
        fields: map
            .into_iter()
            .map(|(k, v)| {
                let Value::String(s) = v else { panic!() };
                (
                    k,
                    prost_types::Value {
                        kind: Some(Kind::StringValue(s)),
                    },
                )
            })
            .collect(),
    }
}

/// Everything published so far, as (kind, entity_type, id).
fn drain(
    rx: &mut Receiver<this::core::events::EventEnvelope>,
) -> Vec<(&'static str, String, Uuid)> {
    let mut out = vec![];
    while let Ok(env) = rx.try_recv() {
        if let FrameworkEvent::Entity(e) = env.event {
            out.push(match e {
                EntityEvent::Created {
                    entity_type,
                    entity_id,
                    ..
                } => ("created", entity_type, entity_id),
                EntityEvent::Updated {
                    entity_type,
                    entity_id,
                    ..
                } => ("updated", entity_type, entity_id),
                EntityEvent::Deleted {
                    entity_type,
                    entity_id,
                } => ("deleted", entity_type, entity_id),
            });
        }
    }
    out
}

fn id_of(v: &Value) -> Uuid {
    Uuid::parse_str(v["id"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn rest_write_emits_one_event_each() {
    let h = host(true);
    let mut rx = h.event_bus().unwrap().subscribe();

    let (s, row) = rest(&h, "POST", "/invoices", json!({"number": "A1"})).await;
    assert_eq!(s, StatusCode::CREATED, "{row}");
    let id = id_of(&row);
    assert_eq!(drain(&mut rx), vec![("created", "invoice".into(), id)]);

    let (s, _) = rest(
        &h,
        "PUT",
        &format!("/invoices/{id}"),
        json!({"number": "A2"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(drain(&mut rx), vec![("updated", "invoice".into(), id)]);

    let (s, _) = rest(&h, "DELETE", &format!("/invoices/{id}"), json!({})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(drain(&mut rx), vec![("deleted", "invoice".into(), id)]);

    // A refused write publishes nothing.
    let (s, _) = rest(
        &h,
        "DELETE",
        &format!("/invoices/{}", Uuid::new_v4()),
        json!({}),
    )
    .await;
    assert_ne!(s, StatusCode::NO_CONTENT);
    assert!(drain(&mut rx).is_empty());
}

#[tokio::test]
async fn graphql_write_emits_one_event_each() {
    let h = host(true);
    let mut rx = h.event_bus().unwrap().subscribe();

    let out = gql(
        &h,
        r#"mutation { createInvoice(data: {number: "A1"}) { id } }"#,
    )
    .await;
    let id = Uuid::parse_str(out["data"]["createInvoice"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(drain(&mut rx), vec![("created", "invoice".into(), id)]);

    gql(
        &h,
        &format!(r#"mutation {{ updateInvoice(id: "{id}", data: {{number: "A2"}}) {{ id }} }}"#),
    )
    .await;
    assert_eq!(drain(&mut rx), vec![("updated", "invoice".into(), id)]);

    gql(&h, &format!(r#"mutation {{ deleteInvoice(id: "{id}") }}"#)).await;
    assert_eq!(drain(&mut rx), vec![("deleted", "invoice".into(), id)]);
}

#[tokio::test]
async fn grpc_write_emits_one_event_each() {
    let h = host(true);
    let mut rx = h.event_bus().unwrap().subscribe();
    let svc = EntityServiceImpl::new(h.clone());

    let res = svc
        .create_entity(tonic::Request::new(CreateEntityRequest {
            entity_type: "invoice".into(),
            data: Some(st(json!({"number": "A1"}))),
        }))
        .await
        .unwrap()
        .into_inner();
    let id = {
        let f = res.data.unwrap().fields;
        let prost_types::value::Kind::StringValue(s) = f["id"].kind.clone().unwrap() else {
            panic!()
        };
        Uuid::parse_str(&s).unwrap()
    };
    assert_eq!(drain(&mut rx), vec![("created", "invoice".into(), id)]);

    svc.update_entity(tonic::Request::new(UpdateEntityRequest {
        entity_type: "invoice".into(),
        entity_id: id.to_string(),
        data: Some(st(json!({"number": "A2"}))),
    }))
    .await
    .unwrap();
    assert_eq!(drain(&mut rx), vec![("updated", "invoice".into(), id)]);

    svc.delete_entity(tonic::Request::new(DeleteEntityRequest {
        entity_type: "invoice".into(),
        entity_id: id.to_string(),
    }))
    .await
    .unwrap();
    assert_eq!(drain(&mut rx), vec![("deleted", "invoice".into(), id)]);
}

#[tokio::test]
async fn no_event_bus_changes_nothing() {
    let h = host(false);
    let (s, _) = rest(&h, "POST", "/invoices", json!({"number": "A1"})).await;
    assert_eq!(s, StatusCode::CREATED);
    gql(
        &h,
        r#"mutation { createInvoice(data: {number: "A1"}) { id } }"#,
    )
    .await;
}
