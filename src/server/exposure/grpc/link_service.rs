//! gRPC Link Service implementation
//!
//! Implements relationship management operations between entities.
//! Uses the `LinkService` trait from the `ServerHost` for all operations
//! and enriches link responses with entity data via `EntityFetcher`.

use super::convert::{json_to_struct, struct_to_json};
use super::proto::{
    CreateLinkRequest, DeleteLinkRequest, DeleteLinkResponse, FindLinksRequest, GetLinkRequest,
    LinkListResponse, LinkResponse, link_service_server::LinkService as LinkServiceTrait,
};
use crate::core::link::LinkEntity;
use crate::server::host::ServerHost;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// gRPC Link Service implementation
///
/// Delegates operations to the `ServerHost`'s link service and enriches
/// responses with entity data from entity fetchers.
pub struct LinkServiceImpl {
    host: Arc<ServerHost>,
}

impl LinkServiceImpl {
    /// Create a new LinkServiceImpl from a ServerHost
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self { host }
    }

    /// Convert a LinkEntity to a gRPC LinkResponse, optionally enriching with entity data
    async fn link_to_response(&self, link: &LinkEntity) -> LinkResponse {
        let metadata = link.metadata.as_ref().map(json_to_struct);

        // Try to fetch source and target entity data for enrichment
        let source_data = self.fetch_entity_data(&link.source_id).await;
        let target_data = self.fetch_entity_data(&link.target_id).await;

        LinkResponse {
            id: link.id.to_string(),
            link_type: link.link_type.clone(),
            source_id: link.source_id.to_string(),
            target_id: link.target_id.to_string(),
            metadata,
            created_at: link.created_at.to_rfc3339(),
            updated_at: link.updated_at.to_rfc3339(),
            source_data,
            target_data,
        }
    }

    /// Try to fetch entity data by ID across all registered fetchers
    async fn fetch_entity_data(&self, entity_id: &Uuid) -> Option<prost_types::Struct> {
        for fetcher in self.host.entity_fetchers.values() {
            if let Ok(json) = fetcher.fetch_as_json(entity_id).await {
                return Some(json_to_struct(&json));
            }
        }
        None
    }
}

#[tonic::async_trait]
impl LinkServiceTrait for LinkServiceImpl {
    async fn create_link(
        &self,
        request: Request<CreateLinkRequest>,
    ) -> Result<Response<LinkResponse>, Status> {
        let req = request.into_inner();

        let source_id = Uuid::parse_str(&req.source_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid source_id: {}", e)))?;

        let target_id = Uuid::parse_str(&req.target_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid target_id: {}", e)))?;

        let metadata = req.metadata.as_ref().map(struct_to_json);

        let link = LinkEntity::new(&req.link_type, source_id, target_id, metadata);

        let created = self
            .host
            .link_service
            .create(link)
            .await
            .map_err(|e| Status::internal(format!("Failed to create link: {}", e)))?;

        // Publish event if event bus is configured
        if let Some(ref bus) = self.host.event_bus {
            bus.publish(crate::core::events::FrameworkEvent::Link(
                crate::core::events::LinkEvent::Created {
                    link_type: req.link_type.clone(),
                    link_id: created.id,
                    source_id,
                    target_id,
                    metadata: created.metadata.clone(),
                },
            ));
        }

        let response = self.link_to_response(&created).await;
        Ok(Response::new(response))
    }

    async fn get_link(
        &self,
        request: Request<GetLinkRequest>,
    ) -> Result<Response<LinkResponse>, Status> {
        let req = request.into_inner();

        let link_id = Uuid::parse_str(&req.link_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid link_id: {}", e)))?;

        let link = self
            .host
            .link_service
            .get(&link_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to get link: {}", e)))?
            .ok_or_else(|| Status::not_found(format!("Link '{}' not found", req.link_id)))?;

        let response = self.link_to_response(&link).await;
        Ok(Response::new(response))
    }

    async fn find_links_by_source(
        &self,
        request: Request<FindLinksRequest>,
    ) -> Result<Response<LinkListResponse>, Status> {
        let req = request.into_inner();

        let entity_id = Uuid::parse_str(&req.entity_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid entity_id: {}", e)))?;

        let link_type = if req.link_type.is_empty() {
            None
        } else {
            Some(req.link_type.as_str())
        };

        let entity_type = if req.entity_type.is_empty() {
            None
        } else {
            Some(req.entity_type.as_str())
        };

        let links = self
            .host
            .link_service
            .find_by_source(&entity_id, link_type, entity_type)
            .await
            .map_err(|e| Status::internal(format!("Failed to find links: {}", e)))?;

        let mut responses = Vec::with_capacity(links.len());
        for link in &links {
            responses.push(self.link_to_response(link).await);
        }

        Ok(Response::new(LinkListResponse { links: responses }))
    }

    async fn find_links_by_target(
        &self,
        request: Request<FindLinksRequest>,
    ) -> Result<Response<LinkListResponse>, Status> {
        let req = request.into_inner();

        let entity_id = Uuid::parse_str(&req.entity_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid entity_id: {}", e)))?;

        let link_type = if req.link_type.is_empty() {
            None
        } else {
            Some(req.link_type.as_str())
        };

        let entity_type = if req.entity_type.is_empty() {
            None
        } else {
            Some(req.entity_type.as_str())
        };

        let links = self
            .host
            .link_service
            .find_by_target(&entity_id, link_type, entity_type)
            .await
            .map_err(|e| Status::internal(format!("Failed to find links: {}", e)))?;

        let mut responses = Vec::with_capacity(links.len());
        for link in &links {
            responses.push(self.link_to_response(link).await);
        }

        Ok(Response::new(LinkListResponse { links: responses }))
    }

    async fn delete_link(
        &self,
        request: Request<DeleteLinkRequest>,
    ) -> Result<Response<DeleteLinkResponse>, Status> {
        let req = request.into_inner();

        let link_id = Uuid::parse_str(&req.link_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid link_id: {}", e)))?;

        // Fetch link before deletion for event data
        let link = self
            .host
            .link_service
            .get(&link_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to get link: {}", e)))?;

        self.host
            .link_service
            .delete(&link_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to delete link: {}", e)))?;

        // Publish event if event bus is configured
        if let Some(ref bus) = self.host.event_bus
            && let Some(link) = link
        {
            bus.publish(crate::core::events::FrameworkEvent::Link(
                crate::core::events::LinkEvent::Deleted {
                    link_type: link.link_type.clone(),
                    link_id,
                    source_id: link.source_id,
                    target_id: link.target_id,
                },
            ));
        }

        Ok(Response::new(DeleteLinkResponse { success: true }))
    }
}

#[cfg(all(test, feature = "grpc"))]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::events::EventBus;
    use crate::core::module::{EntityCreator, EntityFetcher};
    use crate::core::service::LinkService;
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::RwLock;
    use tonic::Code;

    // -----------------------------------------------------------------------
    // Mock EntityFetcher (returns data for any ID to enable enrichment)
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
    }

    // Minimal creator mock (not used by LinkService but needed for host construction)
    struct StubCreator;

    #[async_trait::async_trait]
    impl EntityCreator for StubCreator {
        async fn create_from_json(
            &self,
            data: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            Ok(data)
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
        }
    }

    fn make_host(link_service: Arc<InMemoryLinkService>) -> Arc<ServerHost> {
        Arc::new(
            ServerHost::from_builder_components(
                link_service,
                test_config(),
                EntityRegistry::new(),
                HashMap::new(),
                HashMap::new(),
            )
            .expect("should build host"),
        )
    }

    fn make_host_with_fetcher(
        link_service: Arc<InMemoryLinkService>,
        fetcher: Arc<dyn EntityFetcher>,
    ) -> Arc<ServerHost> {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), fetcher);

        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), Arc::new(StubCreator));

        Arc::new(
            ServerHost::from_builder_components(
                link_service,
                test_config(),
                EntityRegistry::new(),
                fetchers,
                creators,
            )
            .expect("should build host"),
        )
    }

    fn make_host_with_event_bus(link_service: Arc<InMemoryLinkService>) -> Arc<ServerHost> {
        Arc::new(
            ServerHost::from_builder_components(
                link_service,
                test_config(),
                EntityRegistry::new(),
                HashMap::new(),
                HashMap::new(),
            )
            .expect("should build host")
            .with_event_bus(EventBus::new(16)),
        )
    }

    // -----------------------------------------------------------------------
    // create_link tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn create_link_valid_request_returns_link_response() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let host = make_host(link_svc);
        let svc = LinkServiceImpl::new(host);

        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let resp = svc
            .create_link(Request::new(CreateLinkRequest {
                link_type: "has_invoice".to_string(),
                source_id: source_id.to_string(),
                target_id: target_id.to_string(),
                metadata: None,
            }))
            .await
            .expect("create_link should succeed");

        let inner = resp.into_inner();
        assert_eq!(inner.link_type, "has_invoice");
        assert_eq!(inner.source_id, source_id.to_string());
        assert_eq!(inner.target_id, target_id.to_string());
        assert!(!inner.id.is_empty(), "link id should not be empty");
    }

    #[tokio::test]
    async fn create_link_invalid_source_id_returns_invalid_argument() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let svc = LinkServiceImpl::new(make_host(link_svc));

        let err = svc
            .create_link(Request::new(CreateLinkRequest {
                link_type: "has_invoice".to_string(),
                source_id: "not-a-uuid".to_string(),
                target_id: Uuid::new_v4().to_string(),
                metadata: None,
            }))
            .await
            .expect_err("should fail on invalid source_id");

        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(
            err.message().contains("Invalid source_id"),
            "error message should mention source_id"
        );
    }

    #[tokio::test]
    async fn create_link_invalid_target_id_returns_invalid_argument() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let svc = LinkServiceImpl::new(make_host(link_svc));

        let err = svc
            .create_link(Request::new(CreateLinkRequest {
                link_type: "has_invoice".to_string(),
                source_id: Uuid::new_v4().to_string(),
                target_id: "bad-target".to_string(),
                metadata: None,
            }))
            .await
            .expect_err("should fail on invalid target_id");

        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(
            err.message().contains("Invalid target_id"),
            "error message should mention target_id"
        );
    }

    #[tokio::test]
    async fn create_link_publishes_event() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let host = make_host_with_event_bus(link_svc);

        let bus = host.event_bus().expect("event bus should be configured");
        let mut rx = bus.subscribe();

        let svc = LinkServiceImpl::new(host);

        svc.create_link(Request::new(CreateLinkRequest {
            link_type: "has_invoice".to_string(),
            source_id: Uuid::new_v4().to_string(),
            target_id: Uuid::new_v4().to_string(),
            metadata: None,
        }))
        .await
        .expect("create_link should succeed");

        let envelope = rx
            .try_recv()
            .expect("should have received a link created event");
        assert_eq!(envelope.event.action(), "created");
        assert_eq!(envelope.event.event_kind(), "link");
    }

    // -----------------------------------------------------------------------
    // get_link tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_link_valid_request_returns_link() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let link = LinkEntity::new("has_invoice", source_id, target_id, None);
        let link_id = link.id;
        link_svc
            .create(link)
            .await
            .expect("should create link in store");

        let svc = LinkServiceImpl::new(make_host(link_svc));

        let resp = svc
            .get_link(Request::new(GetLinkRequest {
                link_id: link_id.to_string(),
            }))
            .await
            .expect("get_link should succeed");

        let inner = resp.into_inner();
        assert_eq!(inner.id, link_id.to_string());
        assert_eq!(inner.link_type, "has_invoice");
    }

    #[tokio::test]
    async fn get_link_not_found_returns_not_found() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let svc = LinkServiceImpl::new(make_host(link_svc));

        let err = svc
            .get_link(Request::new(GetLinkRequest {
                link_id: Uuid::new_v4().to_string(),
            }))
            .await
            .expect_err("should fail when link does not exist");

        assert_eq!(err.code(), Code::NotFound);
        assert!(
            err.message().contains("not found"),
            "error message should mention 'not found'"
        );
    }

    // -----------------------------------------------------------------------
    // find_links_by_source tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn find_links_by_source_returns_matching_links() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let source_id = Uuid::new_v4();

        link_svc
            .create(LinkEntity::new(
                "has_invoice",
                source_id,
                Uuid::new_v4(),
                None,
            ))
            .await
            .expect("create link 1");
        link_svc
            .create(LinkEntity::new(
                "has_payment",
                source_id,
                Uuid::new_v4(),
                None,
            ))
            .await
            .expect("create link 2");
        // Different source — should not appear
        link_svc
            .create(LinkEntity::new(
                "has_invoice",
                Uuid::new_v4(),
                Uuid::new_v4(),
                None,
            ))
            .await
            .expect("create link 3");

        let svc = LinkServiceImpl::new(make_host(link_svc));

        let resp = svc
            .find_links_by_source(Request::new(FindLinksRequest {
                entity_id: source_id.to_string(),
                link_type: String::new(),
                entity_type: String::new(),
            }))
            .await
            .expect("find_links_by_source should succeed");

        assert_eq!(resp.into_inner().links.len(), 2);
    }

    #[tokio::test]
    async fn find_links_by_source_with_link_type_filter() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let source_id = Uuid::new_v4();

        link_svc
            .create(LinkEntity::new(
                "has_invoice",
                source_id,
                Uuid::new_v4(),
                None,
            ))
            .await
            .expect("create link 1");
        link_svc
            .create(LinkEntity::new(
                "has_payment",
                source_id,
                Uuid::new_v4(),
                None,
            ))
            .await
            .expect("create link 2");

        let svc = LinkServiceImpl::new(make_host(link_svc));

        let resp = svc
            .find_links_by_source(Request::new(FindLinksRequest {
                entity_id: source_id.to_string(),
                link_type: "has_invoice".to_string(),
                entity_type: String::new(),
            }))
            .await
            .expect("find_links_by_source with filter should succeed");

        let links = resp.into_inner().links;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "has_invoice");
    }

    // -----------------------------------------------------------------------
    // find_links_by_target tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn find_links_by_target_returns_matching_links() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let target_id = Uuid::new_v4();

        link_svc
            .create(LinkEntity::new(
                "has_invoice",
                Uuid::new_v4(),
                target_id,
                None,
            ))
            .await
            .expect("create link 1");
        link_svc
            .create(LinkEntity::new(
                "has_payment",
                Uuid::new_v4(),
                target_id,
                None,
            ))
            .await
            .expect("create link 2");

        let svc = LinkServiceImpl::new(make_host(link_svc));

        let resp = svc
            .find_links_by_target(Request::new(FindLinksRequest {
                entity_id: target_id.to_string(),
                link_type: String::new(),
                entity_type: String::new(),
            }))
            .await
            .expect("find_links_by_target should succeed");

        assert_eq!(resp.into_inner().links.len(), 2);
    }

    #[tokio::test]
    async fn find_links_by_target_with_link_type_filter() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let target_id = Uuid::new_v4();

        link_svc
            .create(LinkEntity::new(
                "has_invoice",
                Uuid::new_v4(),
                target_id,
                None,
            ))
            .await
            .expect("create link 1");
        link_svc
            .create(LinkEntity::new(
                "has_payment",
                Uuid::new_v4(),
                target_id,
                None,
            ))
            .await
            .expect("create link 2");

        let svc = LinkServiceImpl::new(make_host(link_svc));

        let resp = svc
            .find_links_by_target(Request::new(FindLinksRequest {
                entity_id: target_id.to_string(),
                link_type: "has_payment".to_string(),
                entity_type: String::new(),
            }))
            .await
            .expect("find_links_by_target with filter should succeed");

        let links = resp.into_inner().links;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_type, "has_payment");
    }

    // -----------------------------------------------------------------------
    // delete_link tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn delete_link_valid_request_returns_success() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let link = LinkEntity::new("has_invoice", Uuid::new_v4(), Uuid::new_v4(), None);
        let link_id = link.id;
        link_svc
            .create(link)
            .await
            .expect("should create link in store");

        let svc = LinkServiceImpl::new(make_host(link_svc));

        let resp = svc
            .delete_link(Request::new(DeleteLinkRequest {
                link_id: link_id.to_string(),
            }))
            .await
            .expect("delete_link should succeed");

        assert!(resp.into_inner().success);
    }

    #[tokio::test]
    async fn delete_link_publishes_event() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let link = LinkEntity::new("has_invoice", Uuid::new_v4(), Uuid::new_v4(), None);
        let link_id = link.id;
        link_svc
            .create(link)
            .await
            .expect("should create link in store");

        let host = make_host_with_event_bus(link_svc);
        let bus = host.event_bus().expect("event bus should be configured");
        let mut rx = bus.subscribe();

        let svc = LinkServiceImpl::new(host);

        svc.delete_link(Request::new(DeleteLinkRequest {
            link_id: link_id.to_string(),
        }))
        .await
        .expect("delete_link should succeed");

        let envelope = rx
            .try_recv()
            .expect("should have received a link deleted event");
        assert_eq!(envelope.event.action(), "deleted");
        assert_eq!(envelope.event.event_kind(), "link");
    }

    // -----------------------------------------------------------------------
    // link_to_response enrichment test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn link_to_response_enriches_with_entity_data() {
        let link_svc = Arc::new(InMemoryLinkService::new());
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let fetcher = Arc::new(MockEntityFetcher::new());
        fetcher.insert(source_id, json!({"name": "Source Entity"}));
        fetcher.insert(target_id, json!({"name": "Target Entity"}));

        let host = make_host_with_fetcher(link_svc.clone(), fetcher);

        // Pre-create a link via the in-memory service
        let link = LinkEntity::new("has_invoice", source_id, target_id, None);
        let link_id = link.id;
        link_svc
            .create(link)
            .await
            .expect("should create link in store");

        let svc = LinkServiceImpl::new(host);

        let resp = svc
            .get_link(Request::new(GetLinkRequest {
                link_id: link_id.to_string(),
            }))
            .await
            .expect("get_link should succeed");

        let inner = resp.into_inner();
        assert!(
            inner.source_data.is_some(),
            "source_data should be enriched from fetcher"
        );
        assert!(
            inner.target_data.is_some(),
            "target_data should be enriched from fetcher"
        );
    }
}
