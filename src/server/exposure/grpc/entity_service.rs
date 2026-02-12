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
use crate::server::host::ServerHost;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// gRPC Entity Service implementation
///
/// Delegates all operations to the `ServerHost`'s entity fetchers and creators.
/// The entity type is determined at runtime from the request message.
pub struct EntityServiceImpl {
    host: Arc<ServerHost>,
}

impl EntityServiceImpl {
    /// Create a new EntityServiceImpl from a ServerHost
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self { host }
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
        let req = request.into_inner();

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
        let req = request.into_inner();

        let fetcher = self.get_fetcher(&req.entity_type)?;

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
        let req = request.into_inner();

        let creator = self.get_creator(&req.entity_type)?;

        let data = req
            .data
            .as_ref()
            .map(struct_to_json)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let result = creator
            .create_from_json(data)
            .await
            .map_err(|e| Status::internal(format!("Failed to create entity: {}", e)))?;

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
        let req = request.into_inner();

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
            .map_err(|e| Status::internal(format!("Failed to update entity: {}", e)))?;

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
        let req = request.into_inner();

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
