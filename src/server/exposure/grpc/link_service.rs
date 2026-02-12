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
