//! HTTP handlers for link operations
//!
//! This module provides generic handlers that work with any entity types.
//! All handlers are completely entity-agnostic.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::config::LinksConfig;
use crate::core::extractors::{DirectLinkExtractor, ExtractorError, LinkExtractor};
use crate::core::{link::LinkEntity, EntityCreator, EntityFetcher, LinkDefinition, LinkService};
use crate::links::registry::{LinkDirection, LinkRouteRegistry};

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub link_service: Arc<dyn LinkService>,
    pub config: Arc<LinksConfig>,
    pub registry: Arc<LinkRouteRegistry>,
    /// Entity fetchers for enriching links with full entity data
    pub entity_fetchers: Arc<HashMap<String, Arc<dyn EntityFetcher>>>,
    /// Entity creators for creating new entities with automatic linking
    pub entity_creators: Arc<HashMap<String, Arc<dyn EntityCreator>>>,
}

impl AppState {
    /// Get the authorization policy for a link operation
    pub fn get_link_auth_policy(
        link_definition: &LinkDefinition,
        operation: &str,
    ) -> Option<String> {
        link_definition.auth.as_ref().map(|auth| match operation {
            "list" => auth.list.clone(),
            "get" => auth.get.clone(),
            "create" => auth.create.clone(),
            "update" => auth.update.clone(),
            "delete" => auth.delete.clone(),
            _ => "authenticated".to_string(),
        })
    }
}

/// Response for list links endpoint
#[derive(Debug, Serialize)]
pub struct ListLinksResponse {
    pub links: Vec<LinkEntity>,
    pub count: usize,
    pub link_type: String,
    pub direction: String,
    pub description: Option<String>,
}

/// Link with full entity data instead of just references
#[derive(Debug, Serialize)]
pub struct EnrichedLink {
    /// Unique identifier for this link
    pub id: Uuid,

    /// Entity type
    #[serde(rename = "type")]
    pub entity_type: String,

    /// The type of relationship (e.g., "has_invoice", "payment")
    pub link_type: String,

    /// Source entity ID
    pub source_id: Uuid,

    /// Target entity ID
    pub target_id: Uuid,

    /// Full source entity as JSON (omitted when querying from source)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,

    /// Full target entity as JSON (omitted when querying from target)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,

    /// Optional metadata for the relationship
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,

    /// When this link was created
    pub created_at: DateTime<Utc>,

    /// When this link was last updated
    pub updated_at: DateTime<Utc>,

    /// Status
    pub status: String,
}

/// Response for enriched list links endpoint
#[derive(Debug, Serialize)]
pub struct EnrichedListLinksResponse {
    pub links: Vec<EnrichedLink>,
    pub count: usize,
    pub link_type: String,
    pub direction: String,
    pub description: Option<String>,
}

/// Request body for creating a link between existing entities
#[derive(Debug, Deserialize)]
pub struct CreateLinkRequest {
    pub metadata: Option<serde_json::Value>,
}

/// Request body for creating a new linked entity
#[derive(Debug, Deserialize)]
pub struct CreateLinkedEntityRequest {
    pub entity: serde_json::Value,
    pub metadata: Option<serde_json::Value>,
}

/// Context for link enrichment
#[derive(Debug, Clone, Copy)]
enum EnrichmentContext {
    /// Query from source entity - only target entities are included
    FromSource,
    /// Query from target entity - only source entities are included
    FromTarget,
    /// Direct link access - both source and target entities are included
    DirectLink,
}

/// List links using named routes (forward or reverse)
///
/// GET /{entity_type}/{entity_id}/{route_name}
pub async fn list_links(
    State(state): State<AppState>,
    Path((entity_type_plural, entity_id, route_name)): Path<(String, Uuid, String)>,
) -> Result<Json<EnrichedListLinksResponse>, ExtractorError> {
    let extractor = LinkExtractor::from_path_and_registry(
        (entity_type_plural, entity_id, route_name),
        &state.registry,
        &state.config,
    )?;

    // Query links based on direction
    let links = match extractor.direction {
        LinkDirection::Forward => state
            .link_service
            .find_by_source(
                &extractor.entity_id,
                Some(&extractor.link_definition.link_type),
                Some(&extractor.link_definition.target_type),
            )
            .await
            .map_err(|e| ExtractorError::JsonError(e.to_string()))?,
        LinkDirection::Reverse => state
            .link_service
            .find_by_target(
                &extractor.entity_id,
                Some(&extractor.link_definition.link_type),
                Some(&extractor.link_definition.source_type),
            )
            .await
            .map_err(|e| ExtractorError::JsonError(e.to_string()))?,
    };

    // Determine enrichment context based on direction
    let context = match extractor.direction {
        LinkDirection::Forward => EnrichmentContext::FromSource,
        LinkDirection::Reverse => EnrichmentContext::FromTarget,
    };

    // Enrich links with full entity data
    let enriched_links =
        enrich_links_with_entities(&state, links, context, &extractor.link_definition).await?;

    Ok(Json(EnrichedListLinksResponse {
        count: enriched_links.len(),
        links: enriched_links,
        link_type: extractor.link_definition.link_type,
        direction: format!("{:?}", extractor.direction),
        description: extractor.link_definition.description,
    }))
}

/// Helper function to enrich links with full entity data
async fn enrich_links_with_entities(
    state: &AppState,
    links: Vec<LinkEntity>,
    context: EnrichmentContext,
    link_definition: &LinkDefinition,
) -> Result<Vec<EnrichedLink>, ExtractorError> {
    let mut enriched = Vec::new();

    for link in links {
        // Fetch source entity only if needed
        let source_entity = match context {
            EnrichmentContext::FromSource => None,
            EnrichmentContext::FromTarget | EnrichmentContext::DirectLink => {
                // Fetch source entity using the type from link definition
                fetch_entity_by_type(state, &link_definition.source_type, &link.source_id)
                    .await
                    .ok()
            }
        };

        // Fetch target entity only if needed
        let target_entity = match context {
            EnrichmentContext::FromTarget => None,
            EnrichmentContext::FromSource | EnrichmentContext::DirectLink => {
                // Fetch target entity using the type from link definition
                fetch_entity_by_type(state, &link_definition.target_type, &link.target_id)
                    .await
                    .ok()
            }
        };

        enriched.push(EnrichedLink {
            id: link.id,
            entity_type: link.entity_type,
            link_type: link.link_type,
            source_id: link.source_id,
            target_id: link.target_id,
            source: source_entity,
            target: target_entity,
            metadata: link.metadata,
            created_at: link.created_at,
            updated_at: link.updated_at,
            status: link.status,
        });
    }

    Ok(enriched)
}

/// Fetch an entity dynamically by type
async fn fetch_entity_by_type(
    state: &AppState,
    entity_type: &str,
    entity_id: &Uuid,
) -> Result<serde_json::Value, ExtractorError> {
    let fetcher = state.entity_fetchers.get(entity_type).ok_or_else(|| {
        ExtractorError::JsonError(format!(
            "No entity fetcher registered for type: {}",
            entity_type
        ))
    })?;

    fetcher
        .fetch_as_json(entity_id)
        .await
        .map_err(|e| ExtractorError::JsonError(format!("Failed to fetch entity: {}", e)))
}

/// Get a specific link by ID
///
/// GET /links/{link_id}
pub async fn get_link(
    State(state): State<AppState>,
    Path(link_id): Path<Uuid>,
) -> Result<Response, ExtractorError> {
    let link = state
        .link_service
        .get(&link_id)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?
        .ok_or(ExtractorError::LinkNotFound)?;

    // Find the link definition from config
    let link_definition = state
        .config
        .links
        .iter()
        .find(|def| def.link_type == link.link_type)
        .ok_or_else(|| {
            ExtractorError::JsonError(format!(
                "No link definition found for link_type: {}",
                link.link_type
            ))
        })?;

    // Enrich with both source and target entities
    let enriched_links = enrich_links_with_entities(
        &state,
        vec![link],
        EnrichmentContext::DirectLink,
        link_definition,
    )
    .await?;

    let enriched_link = enriched_links
        .into_iter()
        .next()
        .ok_or(ExtractorError::LinkNotFound)?;

    Ok(Json(enriched_link).into_response())
}

/// Get a specific link by source, route_name, and target
///
/// GET /{source_type}/{source_id}/{route_name}/{target_id}
pub async fn get_link_by_route(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, route_name, target_id)): Path<(
        String,
        Uuid,
        String,
        Uuid,
    )>,
) -> Result<Response, ExtractorError> {
    let extractor = DirectLinkExtractor::from_path(
        (source_type_plural, source_id, route_name, target_id),
        &state.registry,
        &state.config,
    )?;

    // Find the specific link
    let existing_links = state
        .link_service
        .find_by_source(
            &extractor.source_id,
            Some(&extractor.link_definition.link_type),
            Some(&extractor.target_type),
        )
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    let link = existing_links
        .into_iter()
        .find(|link| link.target_id == extractor.target_id)
        .ok_or(ExtractorError::LinkNotFound)?;

    // Enrich with both source and target entities
    let enriched_links = enrich_links_with_entities(
        &state,
        vec![link],
        EnrichmentContext::DirectLink,
        &extractor.link_definition,
    )
    .await?;

    let enriched_link = enriched_links
        .into_iter()
        .next()
        .ok_or(ExtractorError::LinkNotFound)?;

    Ok(Json(enriched_link).into_response())
}

/// Create a link between two existing entities
///
/// POST /{source_type}/{source_id}/{route_name}/{target_id}
/// Body: { "metadata": {...} }
pub async fn create_link(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, route_name, target_id)): Path<(
        String,
        Uuid,
        String,
        Uuid,
    )>,
    Json(payload): Json<CreateLinkRequest>,
) -> Result<Response, ExtractorError> {
    let extractor = DirectLinkExtractor::from_path(
        (source_type_plural, source_id, route_name, target_id),
        &state.registry,
        &state.config,
    )?;

    // Create the link between existing entities
    let link = LinkEntity::new(
        extractor.link_definition.link_type,
        extractor.source_id,
        extractor.target_id,
        payload.metadata,
    );

    let created_link = state
        .link_service
        .create(link)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(created_link)).into_response())
}

/// Create a new entity and link it to the source
///
/// POST /{source_type}/{source_id}/{route_name}
/// Body: { "entity": {...entity fields...}, "metadata": {...link metadata...} }
pub async fn create_linked_entity(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, route_name)): Path<(String, Uuid, String)>,
    Json(payload): Json<CreateLinkedEntityRequest>,
) -> Result<Response, ExtractorError> {
    let extractor = LinkExtractor::from_path_and_registry(
        (source_type_plural.clone(), source_id, route_name.clone()),
        &state.registry,
        &state.config,
    )?;

    // Determine source and target based on direction
    let (source_entity_id, target_entity_type) = match extractor.direction {
        LinkDirection::Forward => {
            // Forward: source is the entity in the URL, target is the new entity
            (extractor.entity_id, &extractor.link_definition.target_type)
        }
        LinkDirection::Reverse => {
            // Reverse: target is the entity in the URL, source is the new entity
            (extractor.entity_id, &extractor.link_definition.source_type)
        }
    };

    // Get the entity creator for the target type
    let entity_creator = state
        .entity_creators
        .get(target_entity_type)
        .ok_or_else(|| {
            ExtractorError::JsonError(format!(
                "No entity creator registered for type: {}",
                target_entity_type
            ))
        })?;

    // Create the new entity
    let created_entity = entity_creator
        .create_from_json(payload.entity)
        .await
        .map_err(|e| ExtractorError::JsonError(format!("Failed to create entity: {}", e)))?;

    // Extract the ID from the created entity
    let target_entity_id = created_entity["id"].as_str().ok_or_else(|| {
        ExtractorError::JsonError("Created entity missing 'id' field".to_string())
    })?;
    let target_entity_id = Uuid::parse_str(target_entity_id)
        .map_err(|e| ExtractorError::JsonError(format!("Invalid UUID in created entity: {}", e)))?;

    // Create the link based on direction
    let link = match extractor.direction {
        LinkDirection::Forward => {
            // Forward: source -> target (new entity)
            LinkEntity::new(
                extractor.link_definition.link_type,
                source_entity_id,
                target_entity_id,
                payload.metadata,
            )
        }
        LinkDirection::Reverse => {
            // Reverse: source (new entity) -> target
            LinkEntity::new(
                extractor.link_definition.link_type,
                target_entity_id,
                source_entity_id,
                payload.metadata,
            )
        }
    };

    let created_link = state
        .link_service
        .create(link)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    // Return both the created entity and the link
    let response = serde_json::json!({
        "entity": created_entity,
        "link": created_link,
    });

    Ok((StatusCode::CREATED, Json(response)).into_response())
}

/// Update a link's metadata using route name
///
/// PUT/PATCH /{source_type}/{source_id}/{route_name}/{target_id}
pub async fn update_link(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, route_name, target_id)): Path<(
        String,
        Uuid,
        String,
        Uuid,
    )>,
    Json(payload): Json<CreateLinkRequest>,
) -> Result<Response, ExtractorError> {
    let extractor = DirectLinkExtractor::from_path(
        (source_type_plural, source_id, route_name, target_id),
        &state.registry,
        &state.config,
    )?;

    // Find the existing link
    let existing_links = state
        .link_service
        .find_by_source(
            &extractor.source_id,
            Some(&extractor.link_definition.link_type),
            Some(&extractor.target_type),
        )
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    let mut existing_link = existing_links
        .into_iter()
        .find(|link| link.target_id == extractor.target_id)
        .ok_or_else(|| ExtractorError::RouteNotFound("Link not found".to_string()))?;

    // Update metadata
    existing_link.metadata = payload.metadata;
    existing_link.touch();

    // Save the updated link
    let link_id = existing_link.id;
    let updated_link = state
        .link_service
        .update(&link_id, existing_link)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    Ok(Json(updated_link).into_response())
}

/// Delete a link using route name
///
/// DELETE /{source_type}/{source_id}/{route_name}/{target_id}
pub async fn delete_link(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, route_name, target_id)): Path<(
        String,
        Uuid,
        String,
        Uuid,
    )>,
) -> Result<Response, ExtractorError> {
    let extractor = DirectLinkExtractor::from_path(
        (source_type_plural, source_id, route_name, target_id),
        &state.registry,
        &state.config,
    )?;

    // Find the existing link first
    let existing_links = state
        .link_service
        .find_by_source(
            &extractor.source_id,
            Some(&extractor.link_definition.link_type),
            Some(&extractor.target_type),
        )
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    let existing_link = existing_links
        .into_iter()
        .find(|link| link.target_id == extractor.target_id)
        .ok_or(ExtractorError::LinkNotFound)?;

    // Delete the link by its ID
    state
        .link_service
        .delete(&existing_link.id)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Response for introspection endpoint
#[derive(Debug, Serialize)]
pub struct IntrospectionResponse {
    pub entity_type: String,
    pub entity_id: Uuid,
    pub available_routes: Vec<RouteDescription>,
}

/// Description of an available route
#[derive(Debug, Serialize)]
pub struct RouteDescription {
    pub path: String,
    pub method: String,
    pub link_type: String,
    pub direction: String,
    pub connected_to: String,
    pub description: Option<String>,
}

/// Introspection: List all available link routes for an entity
///
/// GET /{entity_type}/{entity_id}/links
pub async fn list_available_links(
    State(state): State<AppState>,
    Path((entity_type_plural, entity_id)): Path<(String, Uuid)>,
) -> Result<Json<IntrospectionResponse>, ExtractorError> {
    // Convert plural to singular
    let entity_type = state
        .config
        .entities
        .iter()
        .find(|e| e.plural == entity_type_plural)
        .map(|e| e.singular.clone())
        .unwrap_or_else(|| entity_type_plural.clone());

    // Get all routes for this entity type
    let routes = state.registry.list_routes_for_entity(&entity_type);

    let available_routes = routes
        .iter()
        .map(|r| RouteDescription {
            path: format!("/{}/{}/{}", entity_type_plural, entity_id, r.route_name),
            method: "GET".to_string(),
            link_type: r.link_type.clone(),
            direction: format!("{:?}", r.direction),
            connected_to: r.connected_to.clone(),
            description: r.description.clone(),
        })
        .collect();

    Ok(Json(IntrospectionResponse {
        entity_type,
        entity_id,
        available_routes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EntityConfig;
    use crate::core::LinkDefinition;
    use crate::storage::InMemoryLinkService;

    fn create_test_state() -> AppState {
        let config = Arc::new(LinksConfig {
            entities: vec![
                EntityConfig {
                    singular: "user".to_string(),
                    plural: "users".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "car".to_string(),
                    plural: "cars".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
            ],
            links: vec![LinkDefinition {
                link_type: "owner".to_string(),
                source_type: "user".to_string(),
                target_type: "car".to_string(),
                forward_route_name: "cars-owned".to_string(),
                reverse_route_name: "users-owners".to_string(),
                description: Some("User owns a car".to_string()),
                required_fields: None,
                auth: None,
            }],
            validation_rules: None,
        });

        let registry = Arc::new(LinkRouteRegistry::new(config.clone()));
        let link_service: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());

        AppState {
            link_service,
            config,
            registry,
            entity_fetchers: Arc::new(HashMap::new()),
            entity_creators: Arc::new(HashMap::new()),
        }
    }

    #[test]
    fn test_state_creation() {
        let state = create_test_state();
        assert_eq!(state.config.entities.len(), 2);
        assert_eq!(state.config.links.len(), 1);
    }
}
