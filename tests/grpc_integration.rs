//! Integration tests for the gRPC exposure
//!
//! These tests spin up a real HTTP/2 server with gRPC services and verify
//! the full request/response flow using generated tonic clients.
//!
//! Coverage:
//! - Entity CRUD via gRPC (Create, Get, List, Update, Delete)
//! - Link management via gRPC (Create, Get, FindBySource, FindByTarget, Delete)
//! - REST + gRPC cohabitation on the same server
//! - Proto export endpoint

#![cfg(feature = "grpc")]

use anyhow::Result;
use axum::Router;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use this::core::events::EventBus;
use this::core::{EntityCreator, EntityFetcher};
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::grpc::GrpcExposure;
use this::server::host::ServerHost;
use this::storage::InMemoryLinkService;
use tokio::net::TcpListener;
use uuid::Uuid;

// ============================================================================
// In-memory entity store for testing
// ============================================================================

/// A simple in-memory entity store that implements both EntityFetcher and EntityCreator
#[derive(Clone)]
struct TestEntityStore {
    entity_type: String,
    entities: Arc<tokio::sync::RwLock<HashMap<Uuid, serde_json::Value>>>,
}

impl TestEntityStore {
    fn new(entity_type: &str) -> Self {
        Self {
            entity_type: entity_type.to_string(),
            entities: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait::async_trait]
impl EntityFetcher for TestEntityStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let entities = self.entities.read().await;
        entities
            .get(entity_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{} not found: {}", self.entity_type, entity_id))
    }

    async fn get_sample_entity(&self) -> Result<serde_json::Value> {
        let entities = self.entities.read().await;
        entities
            .values()
            .next()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No sample entity available"))
    }

    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        let entities = self.entities.read().await;
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(50) as usize;

        Ok(entities
            .values()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }
}

#[async_trait::async_trait]
impl EntityCreator for TestEntityStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now().to_rfc3339();

        let mut data = entity_data.as_object().cloned().unwrap_or_default();
        data.insert("id".to_string(), json!(id.to_string()));
        data.insert("type".to_string(), json!(self.entity_type));
        data.insert("created_at".to_string(), json!(now));
        data.insert("updated_at".to_string(), json!(now));

        let value = serde_json::Value::Object(data);
        self.entities.write().await.insert(id, value.clone());
        Ok(value)
    }

    async fn update_from_json(
        &self,
        entity_id: &Uuid,
        entity_data: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut entities = self.entities.write().await;
        let existing = entities
            .get_mut(entity_id)
            .ok_or_else(|| anyhow::anyhow!("{} not found: {}", self.entity_type, entity_id))?;

        // Merge update data into existing entity
        if let (Some(existing_obj), Some(update_obj)) =
            (existing.as_object_mut(), entity_data.as_object())
        {
            for (key, value) in update_obj {
                existing_obj.insert(key.clone(), value.clone());
            }
            existing_obj.insert(
                "updated_at".to_string(),
                json!(chrono::Utc::now().to_rfc3339()),
            );
        }

        Ok(existing.clone())
    }

    async fn delete(&self, entity_id: &Uuid) -> Result<()> {
        let mut entities = self.entities.write().await;
        entities
            .remove(entity_id)
            .ok_or_else(|| anyhow::anyhow!("{} not found: {}", self.entity_type, entity_id))?;
        Ok(())
    }
}

/// Minimal EntityDescriptor for registering entity types in the registry
///
/// Only needed so `ProtoGenerator` can discover entity types via `host.entity_types()`.
struct TestEntityDescriptor {
    entity_type: String,
    plural: String,
}

impl TestEntityDescriptor {
    fn new(entity_type: &str, plural: &str) -> Self {
        Self {
            entity_type: entity_type.to_string(),
            plural: plural.to_string(),
        }
    }
}

impl EntityDescriptor for TestEntityDescriptor {
    fn entity_type(&self) -> &str {
        &self.entity_type
    }

    fn plural(&self) -> &str {
        &self.plural
    }

    fn build_routes(&self) -> Router {
        Router::new() // No REST routes needed for gRPC tests
    }
}

// ============================================================================
// Test helpers
// ============================================================================

/// Build a test host with entity stores for "order" and "invoice"
fn build_test_host() -> (Arc<ServerHost>, TestEntityStore, TestEntityStore) {
    use this::config::LinksConfig;

    let order_store = TestEntityStore::new("order");
    let invoice_store = TestEntityStore::new("invoice");

    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert(
        "order".to_string(),
        Arc::new(order_store.clone()) as Arc<dyn EntityFetcher>,
    );
    fetchers.insert(
        "invoice".to_string(),
        Arc::new(invoice_store.clone()) as Arc<dyn EntityFetcher>,
    );

    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert(
        "order".to_string(),
        Arc::new(order_store.clone()) as Arc<dyn EntityCreator>,
    );
    creators.insert(
        "invoice".to_string(),
        Arc::new(invoice_store.clone()) as Arc<dyn EntityCreator>,
    );

    let mut entity_registry = EntityRegistry::new();
    entity_registry.register(Box::new(TestEntityDescriptor::new("order", "orders")));
    entity_registry.register(Box::new(TestEntityDescriptor::new("invoice", "invoices")));

    let host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        LinksConfig::default_config(),
        entity_registry,
        fetchers,
        creators,
    )
    .unwrap()
    .with_event_bus(EventBus::new(256));

    (Arc::new(host), order_store, invoice_store)
}

/// Start a gRPC test server and return the address
async fn start_grpc_server() -> (
    SocketAddr,
    Arc<ServerHost>,
    TestEntityStore,
    TestEntityStore,
) {
    let (host, order_store, invoice_store) = build_test_host();

    let grpc_router = GrpcExposure::build_router(host.clone()).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, grpc_router).await.unwrap();
    });

    // Small delay to let the server start
    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, host, order_store, invoice_store)
}

/// Create a tonic EntityService client connected to the test server
async fn entity_client(
    addr: SocketAddr,
) -> this::server::exposure::grpc::proto::entity_service_client::EntityServiceClient<
    tonic::transport::Channel,
> {
    use this::server::exposure::grpc::proto::entity_service_client::EntityServiceClient;

    let url = format!("http://{}", addr);
    EntityServiceClient::connect(url).await.unwrap()
}

/// Create a tonic LinkService client connected to the test server
async fn link_client(
    addr: SocketAddr,
) -> this::server::exposure::grpc::proto::link_service_client::LinkServiceClient<
    tonic::transport::Channel,
> {
    use this::server::exposure::grpc::proto::link_service_client::LinkServiceClient;

    let url = format!("http://{}", addr);
    LinkServiceClient::connect(url).await.unwrap()
}

/// Helper: convert a JSON value to a prost_types::Struct
fn json_to_struct(json: &serde_json::Value) -> prost_types::Struct {
    match json {
        serde_json::Value::Object(map) => {
            let fields = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect();
            prost_types::Struct { fields }
        }
        _ => prost_types::Struct::default(),
    }
}

/// Helper: convert a JSON value to a prost_types::Value
fn json_to_value(json: &serde_json::Value) -> prost_types::Value {
    use prost_types::value::Kind;
    let kind = match json {
        serde_json::Value::Null => Some(Kind::NullValue(0)),
        serde_json::Value::Bool(b) => Some(Kind::BoolValue(*b)),
        serde_json::Value::Number(n) => Some(Kind::NumberValue(n.as_f64().unwrap_or(0.0))),
        serde_json::Value::String(s) => Some(Kind::StringValue(s.clone())),
        serde_json::Value::Array(arr) => Some(Kind::ListValue(prost_types::ListValue {
            values: arr.iter().map(json_to_value).collect(),
        })),
        serde_json::Value::Object(map) => Some(Kind::StructValue(prost_types::Struct {
            fields: map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect(),
        })),
    };
    prost_types::Value { kind }
}

/// Helper: extract a string field from a prost_types::Struct
fn get_string_field(s: &prost_types::Struct, key: &str) -> Option<String> {
    s.fields.get(key).and_then(|v| {
        if let Some(prost_types::value::Kind::StringValue(s)) = &v.kind {
            Some(s.clone())
        } else {
            None
        }
    })
}

/// Helper: extract a number field from a prost_types::Struct
fn get_number_field(s: &prost_types::Struct, key: &str) -> Option<f64> {
    s.fields.get(key).and_then(|v| {
        if let Some(prost_types::value::Kind::NumberValue(n)) = &v.kind {
            Some(*n)
        } else {
            None
        }
    })
}

// ============================================================================
// Entity CRUD Tests
// ============================================================================

#[tokio::test]
async fn test_grpc_create_entity() {
    use this::server::exposure::grpc::proto::CreateEntityRequest;

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    let data = json_to_struct(&json!({
        "number": "ORD-001",
        "status": "pending",
        "amount": 42.5
    }));

    let response = client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(data),
        })
        .await
        .unwrap()
        .into_inner();

    let entity_data = response.data.unwrap();
    assert_eq!(get_string_field(&entity_data, "type").unwrap(), "order");
    assert_eq!(get_string_field(&entity_data, "number").unwrap(), "ORD-001");
    assert_eq!(get_string_field(&entity_data, "status").unwrap(), "pending");
    assert!(get_string_field(&entity_data, "id").is_some());
    assert!(get_string_field(&entity_data, "created_at").is_some());
}

#[tokio::test]
async fn test_grpc_get_entity() {
    use this::server::exposure::grpc::proto::{CreateEntityRequest, GetEntityRequest};

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    // Create an entity first
    let data = json_to_struct(&json!({
        "number": "ORD-002",
        "status": "active"
    }));

    let created = client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(data),
        })
        .await
        .unwrap()
        .into_inner();

    let entity_id = get_string_field(created.data.as_ref().unwrap(), "id").unwrap();

    // Now fetch it
    let fetched = client
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id: entity_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    let fetched_data = fetched.data.unwrap();
    assert_eq!(get_string_field(&fetched_data, "id").unwrap(), entity_id);
    assert_eq!(
        get_string_field(&fetched_data, "number").unwrap(),
        "ORD-002"
    );
}

#[tokio::test]
async fn test_grpc_list_entities() {
    use this::server::exposure::grpc::proto::{CreateEntityRequest, ListEntitiesRequest};

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    // Create 3 orders
    for i in 1..=3 {
        let data = json_to_struct(&json!({
            "number": format!("ORD-{:03}", i),
            "status": "active"
        }));
        client
            .create_entity(CreateEntityRequest {
                entity_type: "order".to_string(),
                data: Some(data),
            })
            .await
            .unwrap();
    }

    // List all
    let response = client
        .list_entities(ListEntitiesRequest {
            entity_type: "order".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(response.entities.len(), 3);
    assert_eq!(response.total, 3);
}

#[tokio::test]
async fn test_grpc_list_entities_with_pagination() {
    use this::server::exposure::grpc::proto::{CreateEntityRequest, ListEntitiesRequest};

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    // Create 5 orders
    for i in 1..=5 {
        let data = json_to_struct(&json!({
            "number": format!("ORD-{:03}", i),
            "status": "active"
        }));
        client
            .create_entity(CreateEntityRequest {
                entity_type: "order".to_string(),
                data: Some(data),
            })
            .await
            .unwrap();
    }

    // List with limit 2
    let response = client
        .list_entities(ListEntitiesRequest {
            entity_type: "order".to_string(),
            limit: 2,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(response.entities.len(), 2);
}

#[tokio::test]
async fn test_grpc_update_entity() {
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, GetEntityRequest, UpdateEntityRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    // Create
    let data = json_to_struct(&json!({
        "number": "ORD-UPD",
        "status": "pending",
        "amount": 100.0
    }));

    let created = client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(data),
        })
        .await
        .unwrap()
        .into_inner();

    let entity_id = get_string_field(created.data.as_ref().unwrap(), "id").unwrap();

    // Update
    let update_data = json_to_struct(&json!({
        "status": "completed",
        "amount": 150.0
    }));

    let updated = client
        .update_entity(UpdateEntityRequest {
            entity_type: "order".to_string(),
            entity_id: entity_id.clone(),
            data: Some(update_data),
        })
        .await
        .unwrap()
        .into_inner();

    let updated_data = updated.data.unwrap();
    assert_eq!(
        get_string_field(&updated_data, "status").unwrap(),
        "completed"
    );
    assert_eq!(get_number_field(&updated_data, "amount").unwrap(), 150.0);

    // Verify via get
    let fetched = client
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id,
        })
        .await
        .unwrap()
        .into_inner();

    let fetched_data = fetched.data.unwrap();
    assert_eq!(
        get_string_field(&fetched_data, "status").unwrap(),
        "completed"
    );
}

#[tokio::test]
async fn test_grpc_delete_entity() {
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, DeleteEntityRequest, GetEntityRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    // Create
    let data = json_to_struct(&json!({
        "number": "ORD-DEL",
        "status": "active"
    }));

    let created = client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(data),
        })
        .await
        .unwrap()
        .into_inner();

    let entity_id = get_string_field(created.data.as_ref().unwrap(), "id").unwrap();

    // Delete
    let deleted = client
        .delete_entity(DeleteEntityRequest {
            entity_type: "order".to_string(),
            entity_id: entity_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    assert!(deleted.success);

    // Verify it's gone
    let result = client
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id,
        })
        .await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Internal);
}

#[tokio::test]
async fn test_grpc_get_nonexistent_entity() {
    use this::server::exposure::grpc::proto::GetEntityRequest;

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    let result = client
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4().to_string(),
        })
        .await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Internal);
}

#[tokio::test]
async fn test_grpc_unknown_entity_type() {
    use this::server::exposure::grpc::proto::GetEntityRequest;

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    let result = client
        .get_entity(GetEntityRequest {
            entity_type: "nonexistent_type".to_string(),
            entity_id: Uuid::new_v4().to_string(),
        })
        .await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn test_grpc_invalid_uuid() {
    use this::server::exposure::grpc::proto::GetEntityRequest;

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    let result = client
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id: "not-a-valid-uuid".to_string(),
        })
        .await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
}

// ============================================================================
// Link Service Tests
// ============================================================================

#[tokio::test]
async fn test_grpc_create_and_get_link() {
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, CreateLinkRequest, GetLinkRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut eclient = entity_client(addr).await;
    let mut lclient = link_client(addr).await;

    // Create two entities
    let order = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-LINK-1"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let invoice = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-001"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let order_id = get_string_field(order.data.as_ref().unwrap(), "id").unwrap();
    let invoice_id = get_string_field(invoice.data.as_ref().unwrap(), "id").unwrap();

    // Create a link between them
    let created_link = lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order_id.clone(),
            target_id: invoice_id.clone(),
            metadata: None,
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(created_link.link_type, "has_invoice");
    assert_eq!(created_link.source_id, order_id);
    assert_eq!(created_link.target_id, invoice_id);
    assert!(!created_link.id.is_empty());
    assert!(!created_link.created_at.is_empty());

    // Get the link by ID
    let fetched_link = lclient
        .get_link(GetLinkRequest {
            link_id: created_link.id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(fetched_link.id, created_link.id);
    assert_eq!(fetched_link.link_type, "has_invoice");
    assert_eq!(fetched_link.source_id, order_id);
    assert_eq!(fetched_link.target_id, invoice_id);
}

#[tokio::test]
async fn test_grpc_create_link_with_metadata() {
    use this::server::exposure::grpc::proto::{CreateEntityRequest, CreateLinkRequest};

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut eclient = entity_client(addr).await;
    let mut lclient = link_client(addr).await;

    // Create entities
    let order = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-META"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let invoice = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-META"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let order_id = get_string_field(order.data.as_ref().unwrap(), "id").unwrap();
    let invoice_id = get_string_field(invoice.data.as_ref().unwrap(), "id").unwrap();

    // Create link with metadata
    let metadata = json_to_struct(&json!({
        "priority": "high",
        "notes": "Urgent delivery"
    }));

    let created_link = lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order_id,
            target_id: invoice_id,
            metadata: Some(metadata),
        })
        .await
        .unwrap()
        .into_inner();

    // Verify metadata is present
    let meta = created_link.metadata.unwrap();
    assert_eq!(get_string_field(&meta, "priority").unwrap(), "high");
    assert_eq!(get_string_field(&meta, "notes").unwrap(), "Urgent delivery");
}

#[tokio::test]
async fn test_grpc_find_links_by_source() {
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, CreateLinkRequest, FindLinksRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut eclient = entity_client(addr).await;
    let mut lclient = link_client(addr).await;

    // Create one order and two invoices
    let order = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-SRC"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let invoice1 = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-SRC-1"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let invoice2 = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-SRC-2"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let order_id = get_string_field(order.data.as_ref().unwrap(), "id").unwrap();
    let invoice1_id = get_string_field(invoice1.data.as_ref().unwrap(), "id").unwrap();
    let invoice2_id = get_string_field(invoice2.data.as_ref().unwrap(), "id").unwrap();

    // Create two links from order to invoices
    lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order_id.clone(),
            target_id: invoice1_id,
            metadata: None,
        })
        .await
        .unwrap();

    lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order_id.clone(),
            target_id: invoice2_id,
            metadata: None,
        })
        .await
        .unwrap();

    // Find links by source
    let links = lclient
        .find_links_by_source(FindLinksRequest {
            entity_id: order_id,
            link_type: String::new(),
            entity_type: String::new(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(links.links.len(), 2);
    assert!(links.links.iter().all(|l| l.link_type == "has_invoice"));
}

#[tokio::test]
async fn test_grpc_find_links_by_target() {
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, CreateLinkRequest, FindLinksRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut eclient = entity_client(addr).await;
    let mut lclient = link_client(addr).await;

    // Create two orders and one invoice
    let order1 = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-TGT-1"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let order2 = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-TGT-2"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let invoice = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-TGT"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let order1_id = get_string_field(order1.data.as_ref().unwrap(), "id").unwrap();
    let order2_id = get_string_field(order2.data.as_ref().unwrap(), "id").unwrap();
    let invoice_id = get_string_field(invoice.data.as_ref().unwrap(), "id").unwrap();

    // Create links from both orders to the same invoice
    lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order1_id,
            target_id: invoice_id.clone(),
            metadata: None,
        })
        .await
        .unwrap();

    lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order2_id,
            target_id: invoice_id.clone(),
            metadata: None,
        })
        .await
        .unwrap();

    // Find links by target
    let links = lclient
        .find_links_by_target(FindLinksRequest {
            entity_id: invoice_id,
            link_type: String::new(),
            entity_type: String::new(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(links.links.len(), 2);
}

#[tokio::test]
async fn test_grpc_find_links_with_type_filter() {
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, CreateLinkRequest, FindLinksRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut eclient = entity_client(addr).await;
    let mut lclient = link_client(addr).await;

    let order = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-FLT"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let invoice = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-FLT"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let order_id = get_string_field(order.data.as_ref().unwrap(), "id").unwrap();
    let invoice_id = get_string_field(invoice.data.as_ref().unwrap(), "id").unwrap();

    // Create two links of different types
    lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order_id.clone(),
            target_id: invoice_id.clone(),
            metadata: None,
        })
        .await
        .unwrap();

    lclient
        .create_link(CreateLinkRequest {
            link_type: "paid_by".to_string(),
            source_id: order_id.clone(),
            target_id: invoice_id,
            metadata: None,
        })
        .await
        .unwrap();

    // Find only "has_invoice" links
    let links = lclient
        .find_links_by_source(FindLinksRequest {
            entity_id: order_id,
            link_type: "has_invoice".to_string(),
            entity_type: String::new(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(links.links.len(), 1);
    assert_eq!(links.links[0].link_type, "has_invoice");
}

#[tokio::test]
async fn test_grpc_delete_link() {
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, CreateLinkRequest, DeleteLinkRequest, GetLinkRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut eclient = entity_client(addr).await;
    let mut lclient = link_client(addr).await;

    // Create entities and link
    let order = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-DEL-LNK"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let invoice = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-DEL-LNK"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let order_id = get_string_field(order.data.as_ref().unwrap(), "id").unwrap();
    let invoice_id = get_string_field(invoice.data.as_ref().unwrap(), "id").unwrap();

    let link = lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order_id,
            target_id: invoice_id,
            metadata: None,
        })
        .await
        .unwrap()
        .into_inner();

    // Delete the link
    let deleted = lclient
        .delete_link(DeleteLinkRequest {
            link_id: link.id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    assert!(deleted.success);

    // Verify it's gone
    let result = lclient
        .get_link(GetLinkRequest {
            link_id: link.id.clone(),
        })
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_grpc_link_invalid_uuid() {
    use this::server::exposure::grpc::proto::CreateLinkRequest;

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut lclient = link_client(addr).await;

    let result = lclient
        .create_link(CreateLinkRequest {
            link_type: "test".to_string(),
            source_id: "not-a-uuid".to_string(),
            target_id: Uuid::new_v4().to_string(),
            metadata: None,
        })
        .await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
}

// ============================================================================
// Cohabitation Tests — REST + gRPC on the same server
// ============================================================================

/// Start a combined REST+gRPC server using build_router_no_fallback + merge
async fn start_rest_grpc_server() -> (
    SocketAddr,
    Arc<ServerHost>,
    TestEntityStore,
    TestEntityStore,
) {
    use this::server::exposure::rest::RestExposure;
    use this::server::router::combine_rest_and_grpc;

    let (host, order_store, invoice_store) = build_test_host();

    let rest_router = RestExposure::build_router(host.clone(), vec![]).unwrap();
    let grpc_router = GrpcExposure::build_router_no_fallback(host.clone()).unwrap();
    let app = combine_rest_and_grpc(rest_router, grpc_router);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, host, order_store, invoice_store)
}

#[tokio::test]
async fn test_grpc_and_rest_cohabitation() {
    use this::server::exposure::grpc::proto::{CreateEntityRequest, GetEntityRequest};

    let (addr, _host, _order_store, _invoice_store) = start_rest_grpc_server().await;

    // --- gRPC: create an entity ---
    let mut grpc_client = entity_client(addr).await;

    let data = json_to_struct(&json!({
        "number": "ORD-COHAB",
        "status": "active"
    }));

    let created = grpc_client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(data),
        })
        .await
        .unwrap()
        .into_inner();

    let entity_id = get_string_field(created.data.as_ref().unwrap(), "id").unwrap();

    // --- gRPC: verify via get ---
    let fetched = grpc_client
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id: entity_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(
        get_string_field(&fetched.data.unwrap(), "number").unwrap(),
        "ORD-COHAB"
    );

    // --- REST: health check works alongside gRPC ---
    let http_client = reqwest::Client::new();
    let resp = http_client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_no_fallback_router_can_merge_with_rest() {
    // Prove that build_router_no_fallback produces a router without fallback,
    // which can be merged with a router that has a fallback.
    use this::server::exposure::rest::RestExposure;
    use this::server::router::combine_rest_and_grpc;

    let (host, _order_store, _invoice_store) = build_test_host();

    let rest_router = RestExposure::build_router(host.clone(), vec![]).unwrap();
    let grpc_router = GrpcExposure::build_router_no_fallback(host).unwrap();

    // This MUST NOT panic — the whole point of this fix
    let _app = combine_rest_and_grpc(rest_router, grpc_router);
}

#[tokio::test]
#[should_panic(expected = "Cannot merge two `Router`s that both have a fallback")]
async fn test_build_router_with_fallback_panics_on_rest_merge() {
    // Prove that the OLD build_router (with fallback) panics when merged with REST
    use this::server::exposure::rest::RestExposure;

    let (host, _order_store, _invoice_store) = build_test_host();

    let rest_router = RestExposure::build_router(host.clone(), vec![]).unwrap();
    let grpc_router = GrpcExposure::build_router(host).unwrap();

    // This MUST panic — proves the problem still exists with build_router()
    let _app = rest_router.merge(grpc_router);
}

#[tokio::test]
async fn test_rest_nested_paths_with_grpc() {
    // Verify that REST's nested link path fallback still works when gRPC is merged
    let (addr, _host, _order_store, _invoice_store) = start_rest_grpc_server().await;

    let http_client = reqwest::Client::new();

    // Health check — basic REST route
    let resp = http_client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Deeply nested path — should hit the REST fallback handler, not 404
    // The fallback handles paths with 5+ segments for link traversal
    let resp = http_client
        .get(format!(
            "http://{}/orders/{}/invoices/{}/payments",
            addr,
            Uuid::new_v4(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .unwrap();
    // The response may be 400/404/500 depending on entity resolution,
    // but it should NOT be a 405 Method Not Allowed or connection error —
    // the request should reach the REST fallback handler.
    assert_ne!(resp.status(), 405);
}

#[tokio::test]
async fn test_grpc_services_work_in_combined_router() {
    // Full gRPC CRUD via the combined REST+gRPC router
    use this::server::exposure::grpc::proto::{
        CreateEntityRequest, CreateLinkRequest, FindLinksRequest, GetEntityRequest,
        ListEntitiesRequest,
    };

    let (addr, _host, _order_store, _invoice_store) = start_rest_grpc_server().await;
    let mut eclient = entity_client(addr).await;
    let mut lclient = link_client(addr).await;

    // Create order via gRPC
    let order = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-COMBINED"}))),
        })
        .await
        .unwrap()
        .into_inner();
    let order_id = get_string_field(order.data.as_ref().unwrap(), "id").unwrap();

    // Create invoice via gRPC
    let invoice = eclient
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-COMBINED"}))),
        })
        .await
        .unwrap()
        .into_inner();
    let invoice_id = get_string_field(invoice.data.as_ref().unwrap(), "id").unwrap();

    // Get entity via gRPC
    let fetched = eclient
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id: order_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        get_string_field(&fetched.data.unwrap(), "number").unwrap(),
        "ORD-COMBINED"
    );

    // List entities via gRPC
    let list = eclient
        .list_entities(ListEntitiesRequest {
            entity_type: "order".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.entities.len(), 1);

    // Create link via gRPC
    let link = lclient
        .create_link(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: order_id.clone(),
            target_id: invoice_id.clone(),
            metadata: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!link.id.is_empty());

    // Find links via gRPC
    let links = lclient
        .find_links_by_source(FindLinksRequest {
            entity_id: order_id,
            link_type: String::new(),
            entity_type: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(links.links.len(), 1);
    assert_eq!(links.links[0].link_type, "has_invoice");
}

#[tokio::test]
async fn test_grpc_proto_endpoint_in_combined_router() {
    // Verify /grpc/proto works in the combined REST+gRPC router
    let (addr, _host, _order_store, _invoice_store) = start_rest_grpc_server().await;

    let http_client = reqwest::Client::new();
    let resp = http_client
        .get(format!("http://{}/grpc/proto", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("syntax = \"proto3\""));
    assert!(body.contains("package this_api"));
    assert!(body.contains("service LinkService"));
}

#[tokio::test]
async fn test_build_with_grpc_convenience() {
    // Test the ServerBuilder::build_with_grpc() convenience method
    use this::config::LinksConfig;
    use this::core::module::Module;
    use this::server::ServerBuilder;
    use this::server::entity_registry::EntityRegistry;
    use this::storage::InMemoryLinkService;

    // Minimal module for testing
    struct MinimalModule;

    impl Module for MinimalModule {
        fn name(&self) -> &str {
            "test"
        }

        fn entity_types(&self) -> Vec<&str> {
            vec!["item"]
        }

        fn links_config(&self) -> Result<LinksConfig> {
            Ok(LinksConfig::default_config())
        }

        fn register_entities(&self, registry: &mut EntityRegistry) {
            registry.register(Box::new(TestEntityDescriptor::new("item", "items")));
        }

        fn get_entity_fetcher(
            &self,
            entity_type: &str,
        ) -> Option<Arc<dyn this::core::EntityFetcher>> {
            if entity_type == "item" {
                Some(Arc::new(TestEntityStore::new("item")))
            } else {
                None
            }
        }

        fn get_entity_creator(
            &self,
            entity_type: &str,
        ) -> Option<Arc<dyn this::core::EntityCreator>> {
            if entity_type == "item" {
                Some(Arc::new(TestEntityStore::new("item")))
            } else {
                None
            }
        }
    }

    // build_with_grpc() must not panic (no double-fallback)
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(MinimalModule)
        .unwrap()
        .build_with_grpc()
        .unwrap();

    // Verify the router works
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // REST health check
    let http_client = reqwest::Client::new();
    let resp = http_client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // gRPC proto export
    let resp = http_client
        .get(format!("http://{}/grpc/proto", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_grpc_standalone_still_works() {
    // Regression test: build_router() (with fallback) still works for standalone gRPC
    use this::server::exposure::grpc::proto::{CreateEntityRequest, GetEntityRequest};

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    // Create
    let created = client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-STANDALONE"}))),
        })
        .await
        .unwrap()
        .into_inner();

    let entity_id = get_string_field(created.data.as_ref().unwrap(), "id").unwrap();

    // Get
    let fetched = client
        .get_entity(GetEntityRequest {
            entity_type: "order".to_string(),
            entity_id,
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(
        get_string_field(&fetched.data.unwrap(), "number").unwrap(),
        "ORD-STANDALONE"
    );
}

#[tokio::test]
async fn test_grpc_proto_export_endpoint() {
    use axum_test::TestServer;

    let (host, _order_store, _invoice_store) = build_test_host();
    let grpc_router = GrpcExposure::build_router(host).unwrap();

    let server = TestServer::new(grpc_router).unwrap();

    // Fetch the proto export via HTTP GET
    let response = server.get("/grpc/proto").await;

    response.assert_status_ok();

    let body = response.text();
    assert!(body.contains("syntax = \"proto3\""));
    assert!(body.contains("package this_api"));
    // Should have typed services for our registered entity types
    assert!(body.contains("service LinkService"));
}

#[tokio::test]
async fn test_grpc_create_entities_of_different_types() {
    use this::server::exposure::grpc::proto::{CreateEntityRequest, ListEntitiesRequest};

    let (addr, _host, _order_store, _invoice_store) = start_grpc_server().await;
    let mut client = entity_client(addr).await;

    // Create an order
    client
        .create_entity(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"number": "ORD-MULTI"}))),
        })
        .await
        .unwrap();

    // Create an invoice
    client
        .create_entity(CreateEntityRequest {
            entity_type: "invoice".to_string(),
            data: Some(json_to_struct(&json!({"number": "INV-MULTI"}))),
        })
        .await
        .unwrap();

    // List orders — should only have 1
    let orders = client
        .list_entities(ListEntitiesRequest {
            entity_type: "order".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();

    // List invoices — should only have 1
    let invoices = client
        .list_entities(ListEntitiesRequest {
            entity_type: "invoice".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(orders.entities.len(), 1);
    assert_eq!(invoices.entities.len(), 1);
}
