//! gRPC Entity Service implementation
//!
//! Implements generic CRUD operations for any registered entity type.
//! Uses `EntityFetcher` and `EntityCreator` from the `ServerHost` to
//! resolve operations dynamically.

use super::convert::{json_to_struct, struct_to_json};
use super::proto::{
    CreateEntityRequest, DeleteEntityRequest, DeleteEntityResponse, EntityResponse,
    GetEntityRequest, ListEntitiesRequest, ListEntitiesResponse, UpdateEntityRequest,
    entity_service_server::EntityService,
};
use crate::core::errors::to_status;
use crate::descriptor::Operation;
use crate::server::exposure::guard::SurfaceGuard;
use crate::server::host::ServerHost;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// Request metadata carrying the list filter (REST `?filter=` grammar, JSON object).
/// Non-ASCII values go in the binary variant `this-filter-bin` (same for `this-sort-bin`).
pub const FILTER_METADATA: &str = "this-filter";
/// Request metadata carrying the list sort (REST `?sort=` grammar).
pub const SORT_METADATA: &str = "this-sort";

/// Read a list-query metadata entry, FAIL-CLOSED: `key` (ASCII) or `key-bin` (binary,
/// base64 on the wire, for UTF-8 filter values such as `{"name": "Zoé"}`).
/// - absent → `Ok(None)` (no filter / no sort, historical path);
/// - present but unreadable (non visible-ASCII in `key`, invalid UTF-8 in `key-bin`), or
///   given under both keys → `INVALID_ARGUMENT` `this.transport.bad_request`. Reading an
///   unreadable value as "absent" would silently list EVERYTHING.
fn query_metadata(md: &tonic::metadata::MetadataMap, key: &str) -> Result<Option<String>, Status> {
    let bad = |reason: String| {
        crate::core::errors::to_status(
            crate::descriptor::ContractError::framework(
                crate::descriptor::framework::codes::TRANSPORT_BAD_REQUEST,
            )
            .with_param("reason", reason),
        )
    };
    let ascii = md
        .get(key)
        .map(|v| {
            v.to_str()
                .map(str::to_string)
                .map_err(|_| bad(format!("`{key}` is not visible ASCII: use `{key}-bin`")))
        })
        .transpose()?;
    let bin_key = format!("{key}-bin");
    let binary = md
        .get_bin(bin_key.as_str())
        .map(|v| {
            v.to_bytes()
                .map_err(|_| bad(format!("`{bin_key}` is not valid base64")))
                .and_then(|b| {
                    String::from_utf8(b.to_vec())
                        .map_err(|_| bad(format!("`{bin_key}` is not valid UTF-8")))
                })
        })
        .transpose()?;
    match (ascii, binary) {
        (Some(_), Some(_)) => Err(bad(format!("both `{key}` and `{bin_key}` given"))),
        (a, b) => Ok(a.or(b)),
    }
}

/// A write failure: a `ContractError` (validation, business code) keeps its code and kind
/// (`this.validation.failed` → `INVALID_ARGUMENT`); anything else stays the historical
/// `INTERNAL` with its message.
fn write_error(e: &anyhow::Error, op: &str) -> Status {
    if e.chain().any(|c| {
        c.downcast_ref::<crate::descriptor::ContractError>()
            .is_some()
    }) {
        crate::core::errors::to_status(crate::core::errors::from_anyhow(e))
    } else {
        Status::internal(format!("Failed to {op} entity: {e}"))
    }
}

/// gRPC Entity Service implementation
///
/// Delegates all operations to the `ServerHost`'s entity fetchers and creators.
/// The entity type is determined at runtime from the request message.
pub struct EntityServiceImpl {
    host: Arc<ServerHost>,
    /// Apply the declared policies per call (armed routers). See `identity::guard_of`.
    enforce: bool,
}

impl EntityServiceImpl {
    /// Create a new EntityServiceImpl from a ServerHost.
    ///
    /// **Fail-closed**: policies are applied per call whenever the host has them armed
    /// (`ServerHost::authorize`).
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self {
            host,
            enforce: true,
        }
    }

    /// Service that does NOT apply the policies (explicit opt-in of the
    /// `*_unenforced` routers).
    pub fn unenforced(host: Arc<ServerHost>) -> Self {
        Self {
            host,
            enforce: false,
        }
    }

    async fn guard<T>(&self, request: &Request<T>) -> Result<SurfaceGuard, Status> {
        super::identity::guard_of(&self.host, request, self.enforce).await
    }

    /// Get the entity fetcher for the given type, or return a gRPC NOT_FOUND status
    fn get_fetcher(
        &self,
        entity_type: &str,
    ) -> Result<Arc<dyn crate::core::EntityFetcher>, Status> {
        self.host
            .entity_fetchers
            .get(entity_type)
            .cloned()
            .ok_or_else(|| {
                Status::not_found(format!("Entity type '{}' not registered", entity_type))
            })
    }

    /// Get the entity creator for the given type, or return a gRPC NOT_FOUND status
    fn get_creator(
        &self,
        entity_type: &str,
    ) -> Result<Arc<dyn crate::core::EntityCreator>, Status> {
        self.host
            .entity_creators
            .get(entity_type)
            .cloned()
            .ok_or_else(|| {
                Status::not_found(format!("Entity type '{}' not registered", entity_type))
            })
    }
}

#[tonic::async_trait]
impl EntityService for EntityServiceImpl {
    async fn get_entity(
        &self,
        request: Request<GetEntityRequest>,
    ) -> Result<Response<EntityResponse>, Status> {
        let guard = self.guard(&request).await?;
        let req = request.into_inner();
        guard
            .check(
                &req.entity_type,
                Operation::Get,
                Uuid::parse_str(&req.entity_id).ok().as_ref(),
            )
            .map_err(to_status)?;

        let entity_id = Uuid::parse_str(&req.entity_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid entity_id: {}", e)))?;

        let fetcher = self.get_fetcher(&req.entity_type)?;

        let json = fetcher
            .fetch_as_json(&entity_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to fetch entity: {}", e)))?;

        Ok(Response::new(EntityResponse {
            data: Some(json_to_struct(&json)),
        }))
    }

    async fn list_entities(
        &self,
        request: Request<ListEntitiesRequest>,
    ) -> Result<Response<ListEntitiesResponse>, Status> {
        let guard = self.guard(&request).await?;
        // Typed filters (validation stream): `this-filter` / `this-sort` metadata, same
        // grammar as REST `?filter=` / `?sort=`. Present → checked against the descriptor
        // (strict without one), applied BEFORE pagination; malformed → INVALID_ARGUMENT.
        let filter = query_metadata(request.metadata(), FILTER_METADATA)?;
        let sort = query_metadata(request.metadata(), SORT_METADATA)?;
        let req = request.into_inner();
        guard
            .check(&req.entity_type, Operation::List, None)
            .map_err(to_status)?;

        let fetcher = self.get_fetcher(&req.entity_type)?;

        if filter.is_some() || sort.is_some() {
            let descriptors = self.host.descriptors();
            let desc = descriptors.entity(&req.entity_type).map(|(_, d)| d);
            let q = crate::core::query::query_from_args(
                desc,
                filter.map(serde_json::Value::String),
                sort.as_deref(),
                u32::try_from(req.limit).ok(),
                u32::try_from(req.offset).ok(),
            )
            .map_err(crate::core::errors::to_status)?;
            let (rows, total) = fetcher
                .list_query(&q)
                .await
                .map_err(|e| write_error(&e, "list"))?;
            return Ok(Response::new(ListEntitiesResponse {
                entities: rows.iter().map(json_to_struct).collect(),
                total: i32::try_from(total).unwrap_or(i32::MAX),
            }));
        }

        let limit = if req.limit > 0 { Some(req.limit) } else { None };
        let offset = if req.offset > 0 {
            Some(req.offset)
        } else {
            None
        };

        let entities = fetcher
            .list_as_json(limit, offset)
            .await
            .map_err(|e| Status::internal(format!("Failed to list entities: {}", e)))?;

        let total = entities.len() as i32;
        let proto_entities = entities.iter().map(json_to_struct).collect();

        Ok(Response::new(ListEntitiesResponse {
            entities: proto_entities,
            total,
        }))
    }

    async fn create_entity(
        &self,
        request: Request<CreateEntityRequest>,
    ) -> Result<Response<EntityResponse>, Status> {
        let guard = self.guard(&request).await?;
        let req = request.into_inner();
        guard
            .check(&req.entity_type, Operation::Create, None)
            .map_err(to_status)?;

        let creator = self.get_creator(&req.entity_type)?;

        let data = req
            .data
            .as_ref()
            .map(struct_to_json)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let result = creator
            .create_from_json(data)
            .await
            .map_err(|e| write_error(&e, "create"))?;

        // Publish event if event bus is configured
        if let Some(ref bus) = self.host.event_bus
            && let Some(id) = result.get("id").and_then(|v| v.as_str())
            && let Ok(entity_id) = Uuid::parse_str(id)
        {
            bus.publish(crate::core::events::FrameworkEvent::Entity(
                crate::core::events::EntityEvent::Created {
                    entity_type: req.entity_type.clone(),
                    entity_id,
                    data: result.clone(),
                },
            ));
        }

        Ok(Response::new(EntityResponse {
            data: Some(json_to_struct(&result)),
        }))
    }

    async fn update_entity(
        &self,
        request: Request<UpdateEntityRequest>,
    ) -> Result<Response<EntityResponse>, Status> {
        let guard = self.guard(&request).await?;
        let req = request.into_inner();
        guard
            .check(
                &req.entity_type,
                Operation::Update,
                Uuid::parse_str(&req.entity_id).ok().as_ref(),
            )
            .map_err(to_status)?;

        let entity_id = Uuid::parse_str(&req.entity_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid entity_id: {}", e)))?;

        let creator = self.get_creator(&req.entity_type)?;

        let data = req
            .data
            .as_ref()
            .map(struct_to_json)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let result = creator
            .update_from_json(&entity_id, data)
            .await
            .map_err(|e| write_error(&e, "update"))?;

        // Publish event if event bus is configured
        if let Some(ref bus) = self.host.event_bus {
            bus.publish(crate::core::events::FrameworkEvent::Entity(
                crate::core::events::EntityEvent::Updated {
                    entity_type: req.entity_type.clone(),
                    entity_id,
                    data: result.clone(),
                },
            ));
        }

        Ok(Response::new(EntityResponse {
            data: Some(json_to_struct(&result)),
        }))
    }

    async fn delete_entity(
        &self,
        request: Request<DeleteEntityRequest>,
    ) -> Result<Response<DeleteEntityResponse>, Status> {
        let guard = self.guard(&request).await?;
        let req = request.into_inner();
        guard
            .check(
                &req.entity_type,
                Operation::Delete,
                Uuid::parse_str(&req.entity_id).ok().as_ref(),
            )
            .map_err(to_status)?;

        let entity_id = Uuid::parse_str(&req.entity_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid entity_id: {}", e)))?;

        let creator = self.get_creator(&req.entity_type)?;

        creator
            .delete(&entity_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to delete entity: {}", e)))?;

        // Publish event if event bus is configured
        if let Some(ref bus) = self.host.event_bus {
            bus.publish(crate::core::events::FrameworkEvent::Entity(
                crate::core::events::EntityEvent::Deleted {
                    entity_type: req.entity_type.clone(),
                    entity_id,
                },
            ));
        }

        Ok(Response::new(DeleteEntityResponse { success: true }))
    }
}

#[cfg(all(test, feature = "grpc"))]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::events::EventBus;
    use crate::core::module::{EntityCreator, EntityFetcher};
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    use tonic::Code;

    // -----------------------------------------------------------------------
    // Mock EntityFetcher
    // -----------------------------------------------------------------------

    struct MockEntityFetcher {
        entities: Arc<RwLock<HashMap<Uuid, serde_json::Value>>>,
    }

    impl MockEntityFetcher {
        fn new() -> Self {
            Self {
                entities: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        fn insert(&self, id: Uuid, data: serde_json::Value) {
            self.entities
                .write()
                .expect("lock poisoned")
                .insert(id, data);
        }
    }

    #[async_trait::async_trait]
    impl EntityFetcher for MockEntityFetcher {
        async fn fetch_as_json(&self, entity_id: &Uuid) -> anyhow::Result<serde_json::Value> {
            let store = self.entities.read().expect("lock poisoned");
            store
                .get(entity_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Entity not found: {}", entity_id))
        }

        async fn list_as_json(
            &self,
            limit: Option<i32>,
            offset: Option<i32>,
        ) -> anyhow::Result<Vec<serde_json::Value>> {
            let store = self.entities.read().expect("lock poisoned");
            let mut items: Vec<_> = store.values().cloned().collect();
            if let Some(off) = offset {
                items = items.into_iter().skip(off as usize).collect();
            }
            if let Some(lim) = limit {
                items.truncate(lim as usize);
            }
            Ok(items)
        }
    }

    // -----------------------------------------------------------------------
    // Mock EntityCreator
    // -----------------------------------------------------------------------

    struct MockEntityCreator {
        entities: Arc<RwLock<HashMap<Uuid, serde_json::Value>>>,
    }

    impl MockEntityCreator {
        fn new() -> Self {
            Self {
                entities: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        fn with_store(entities: Arc<RwLock<HashMap<Uuid, serde_json::Value>>>) -> Self {
            Self { entities }
        }
    }

    #[async_trait::async_trait]
    impl EntityCreator for MockEntityCreator {
        async fn create_from_json(
            &self,
            entity_data: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            let id = Uuid::new_v4();
            let mut result = entity_data;
            if let serde_json::Value::Object(ref mut map) = result {
                map.insert("id".to_string(), json!(id.to_string()));
            }
            self.entities
                .write()
                .expect("lock poisoned")
                .insert(id, result.clone());
            Ok(result)
        }

        async fn update_from_json(
            &self,
            entity_id: &Uuid,
            entity_data: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            let mut store = self.entities.write().expect("lock poisoned");
            store.insert(*entity_id, entity_data.clone());
            Ok(entity_data)
        }

        async fn delete(&self, entity_id: &Uuid) -> anyhow::Result<()> {
            self.entities
                .write()
                .expect("lock poisoned")
                .remove(entity_id);
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_config() -> LinksConfig {
        LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        }
    }

    fn make_host_with_mocks(
        fetcher: Arc<dyn EntityFetcher>,
        creator: Arc<dyn EntityCreator>,
    ) -> Arc<ServerHost> {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), fetcher);

        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), creator);

        Arc::new(
            ServerHost::from_builder_components(
                Arc::new(InMemoryLinkService::new()),
                test_config(),
                EntityRegistry::new(),
                fetchers,
                creators,
            )
            .expect("should build host"),
        )
    }

    fn make_host_with_event_bus(
        fetcher: Arc<dyn EntityFetcher>,
        creator: Arc<dyn EntityCreator>,
    ) -> Arc<ServerHost> {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), fetcher);

        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), creator);

        Arc::new(
            ServerHost::from_builder_components(
                Arc::new(InMemoryLinkService::new()),
                test_config(),
                EntityRegistry::new(),
                fetchers,
                creators,
            )
            .expect("should build host")
            .with_event_bus(EventBus::new(16)),
        )
    }

    // -----------------------------------------------------------------------
    // get_entity tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_entity_valid_request_returns_data() {
        let fetcher = Arc::new(MockEntityFetcher::new());
        let id = Uuid::new_v4();
        fetcher.insert(id, json!({"id": id.to_string(), "name": "Order #1"}));

        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            fetcher,
            Arc::new(MockEntityCreator::new()),
        ));

        let resp = svc
            .get_entity(Request::new(GetEntityRequest {
                entity_type: "order".to_string(),
                entity_id: id.to_string(),
            }))
            .await
            .expect("get_entity should succeed");

        let data = resp.into_inner().data.expect("response should have data");
        assert!(
            data.fields.contains_key("name"),
            "response data should contain 'name' field"
        );
    }

    #[tokio::test]
    async fn get_entity_invalid_uuid_returns_invalid_argument() {
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            Arc::new(MockEntityCreator::new()),
        ));

        let err = svc
            .get_entity(Request::new(GetEntityRequest {
                entity_type: "order".to_string(),
                entity_id: "not-a-uuid".to_string(),
            }))
            .await
            .expect_err("should fail on invalid UUID");

        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(
            err.message().contains("Invalid entity_id"),
            "error message should mention invalid entity_id"
        );
    }

    #[tokio::test]
    async fn get_entity_unregistered_type_returns_not_found() {
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            Arc::new(MockEntityCreator::new()),
        ));

        let err = svc
            .get_entity(Request::new(GetEntityRequest {
                entity_type: "unknown_type".to_string(),
                entity_id: Uuid::new_v4().to_string(),
            }))
            .await
            .expect_err("should fail on unregistered type");

        assert_eq!(err.code(), Code::NotFound);
        assert!(
            err.message().contains("not registered"),
            "error message should mention 'not registered'"
        );
    }

    // -----------------------------------------------------------------------
    // list_entities tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_entities_valid_request_returns_items() {
        let fetcher = Arc::new(MockEntityFetcher::new());
        fetcher.insert(Uuid::new_v4(), json!({"name": "Order #1"}));
        fetcher.insert(Uuid::new_v4(), json!({"name": "Order #2"}));

        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            fetcher,
            Arc::new(MockEntityCreator::new()),
        ));

        let resp = svc
            .list_entities(Request::new(ListEntitiesRequest {
                entity_type: "order".to_string(),
                limit: 0,
                offset: 0,
            }))
            .await
            .expect("list_entities should succeed");

        let inner = resp.into_inner();
        assert_eq!(inner.total, 2);
        assert_eq!(inner.entities.len(), 2);
    }

    #[tokio::test]
    async fn list_entities_with_limit_and_offset() {
        let fetcher = Arc::new(MockEntityFetcher::new());
        for i in 0..5 {
            fetcher.insert(Uuid::new_v4(), json!({"name": format!("Order #{}", i)}));
        }

        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            fetcher,
            Arc::new(MockEntityCreator::new()),
        ));

        let resp = svc
            .list_entities(Request::new(ListEntitiesRequest {
                entity_type: "order".to_string(),
                limit: 2,
                offset: 1,
            }))
            .await
            .expect("list_entities should succeed with pagination");

        let inner = resp.into_inner();
        // Offset skips 1, limit takes at most 2
        assert!(
            inner.total <= 4,
            "total should reflect paginated result set"
        );
    }

    #[tokio::test]
    async fn list_entities_unknown_type_returns_not_found() {
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            Arc::new(MockEntityCreator::new()),
        ));

        let err = svc
            .list_entities(Request::new(ListEntitiesRequest {
                entity_type: "widget".to_string(),
                limit: 0,
                offset: 0,
            }))
            .await
            .expect_err("should fail on unregistered type");

        assert_eq!(err.code(), Code::NotFound);
    }

    // -----------------------------------------------------------------------
    // create_entity tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn create_entity_valid_request_returns_data() {
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            Arc::new(MockEntityCreator::new()),
        ));

        let data = super::super::convert::json_to_struct(&json!({"name": "New Order"}));
        let resp = svc
            .create_entity(Request::new(CreateEntityRequest {
                entity_type: "order".to_string(),
                data: Some(data),
            }))
            .await
            .expect("create_entity should succeed");

        let inner = resp.into_inner().data.expect("response should have data");
        assert!(
            inner.fields.contains_key("id"),
            "created entity should have an 'id' field"
        );
        assert!(
            inner.fields.contains_key("name"),
            "created entity should have a 'name' field"
        );
    }

    #[tokio::test]
    async fn create_entity_publishes_event() {
        let creator = Arc::new(MockEntityCreator::new());
        let host = make_host_with_event_bus(Arc::new(MockEntityFetcher::new()), creator);

        let bus = host.event_bus().expect("event bus should be configured");
        let mut rx = bus.subscribe();

        let svc = EntityServiceImpl::unenforced(host);

        let data = super::super::convert::json_to_struct(&json!({"name": "Evented Order"}));
        svc.create_entity(Request::new(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(data),
        }))
        .await
        .expect("create_entity should succeed");

        let envelope = rx.try_recv().expect("should have received a create event");
        assert_eq!(envelope.event.action(), "created");
        assert_eq!(envelope.event.entity_type(), Some("order"));
    }

    #[tokio::test]
    async fn create_entity_no_data_uses_empty_map() {
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            Arc::new(MockEntityCreator::new()),
        ));

        let resp = svc
            .create_entity(Request::new(CreateEntityRequest {
                entity_type: "order".to_string(),
                data: None,
            }))
            .await
            .expect("create_entity with no data should succeed");

        let inner = resp.into_inner().data.expect("response should have data");
        assert!(
            inner.fields.contains_key("id"),
            "created entity should still have an id"
        );
    }

    // -----------------------------------------------------------------------
    // update_entity tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn update_entity_valid_request_returns_updated_data() {
        let store = Arc::new(RwLock::new(HashMap::new()));
        let id = Uuid::new_v4();
        store
            .write()
            .expect("lock poisoned")
            .insert(id, json!({"id": id.to_string(), "name": "Old"}));

        let creator = Arc::new(MockEntityCreator::with_store(store));
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            creator,
        ));

        let data = super::super::convert::json_to_struct(&json!({"name": "Updated"}));
        let resp = svc
            .update_entity(Request::new(UpdateEntityRequest {
                entity_type: "order".to_string(),
                entity_id: id.to_string(),
                data: Some(data),
            }))
            .await
            .expect("update_entity should succeed");

        let inner = resp.into_inner().data.expect("response should have data");
        assert!(inner.fields.contains_key("name"));
    }

    #[tokio::test]
    async fn update_entity_invalid_uuid_returns_invalid_argument() {
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            Arc::new(MockEntityCreator::new()),
        ));

        let err = svc
            .update_entity(Request::new(UpdateEntityRequest {
                entity_type: "order".to_string(),
                entity_id: "bad-uuid".to_string(),
                data: None,
            }))
            .await
            .expect_err("should fail on invalid UUID");

        assert_eq!(err.code(), Code::InvalidArgument);
    }

    // -----------------------------------------------------------------------
    // delete_entity tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn delete_entity_valid_request_returns_success() {
        let store = Arc::new(RwLock::new(HashMap::new()));
        let id = Uuid::new_v4();
        store
            .write()
            .expect("lock poisoned")
            .insert(id, json!({"id": id.to_string()}));

        let creator = Arc::new(MockEntityCreator::with_store(store));
        let svc = EntityServiceImpl::unenforced(make_host_with_mocks(
            Arc::new(MockEntityFetcher::new()),
            creator,
        ));

        let resp = svc
            .delete_entity(Request::new(DeleteEntityRequest {
                entity_type: "order".to_string(),
                entity_id: id.to_string(),
            }))
            .await
            .expect("delete_entity should succeed");

        assert!(resp.into_inner().success);
    }

    #[tokio::test]
    async fn delete_entity_publishes_event() {
        let creator = Arc::new(MockEntityCreator::new());
        let host = make_host_with_event_bus(Arc::new(MockEntityFetcher::new()), creator);

        let bus = host.event_bus().expect("event bus should be configured");
        let mut rx = bus.subscribe();

        let svc = EntityServiceImpl::unenforced(host);
        let id = Uuid::new_v4();

        svc.delete_entity(Request::new(DeleteEntityRequest {
            entity_type: "order".to_string(),
            entity_id: id.to_string(),
        }))
        .await
        .expect("delete_entity should succeed");

        let envelope = rx.try_recv().expect("should have received a delete event");
        assert_eq!(envelope.event.action(), "deleted");
        assert_eq!(envelope.event.entity_type(), Some("order"));
    }

    // -----------------------------------------------------------------------
    // Per-operation policies (armed host)
    // -----------------------------------------------------------------------

    fn armed_host(create: &str) -> Arc<ServerHost> {
        let mut config = test_config();
        config.entities[0].auth.create = create.to_string();
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockEntityFetcher::new()));
        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), Arc::new(MockEntityCreator::new()));
        Arc::new(
            ServerHost::from_builder_components(
                Arc::new(InMemoryLinkService::new()),
                config,
                EntityRegistry::new(),
                fetchers,
                creators,
            )
            .expect("should build host"),
        )
    }

    fn create_as(ctx: Option<crate::core::auth::AuthContext>) -> Request<CreateEntityRequest> {
        let mut req = Request::new(CreateEntityRequest {
            entity_type: "order".to_string(),
            data: Some(json_to_struct(&json!({"name": "x"}))),
        });
        if let Some(ctx) = ctx {
            req.extensions_mut().insert(ctx);
        }
        req
    }

    #[tokio::test]
    async fn grpc_create_denied_without_role() {
        use crate::core::auth::AuthContext;
        let svc = EntityServiceImpl::new(armed_host("role:admin"));

        let anon = svc.create_entity(create_as(None)).await.unwrap_err();
        assert_eq!(anon.code(), Code::PermissionDenied);
        assert!(anon.message().contains("order"), "{}", anon.message());

        let plain = AuthContext::User {
            user_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            roles: vec!["reader".into()],
        };
        let st = svc.create_entity(create_as(Some(plain))).await.unwrap_err();
        assert_eq!(st.code(), Code::PermissionDenied);
        // The JSON envelope of the contract error travels in the details.
        let details: serde_json::Value = serde_json::from_slice(st.details()).unwrap();
        assert_eq!(details["code"], "this.auth.forbidden");

        let admin = AuthContext::User {
            user_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            roles: vec!["admin".into()],
        };
        assert!(svc.create_entity(create_as(Some(admin))).await.is_ok());

        // Default policy (`authenticated`) still guards reads.
        let list = svc
            .list_entities(Request::new(ListEntitiesRequest {
                entity_type: "order".to_string(),
                limit: 0,
                offset: 0,
            }))
            .await
            .unwrap_err();
        assert_eq!(list.code(), Code::PermissionDenied);

        // Explicit opt-out keeps the historical behavior.
        let open = EntityServiceImpl::unenforced(armed_host("role:admin"));
        assert!(open.create_entity(create_as(None)).await.is_ok());
    }
}
