//! HTTP handlers for link operations
//!
//! This module provides generic handlers that work with any entity types.
//! All handlers are completely entity-agnostic.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::config::LinksConfig;
use crate::core::events::{EventBus, FrameworkEvent, LinkEvent};
use crate::core::extractors::{
    DirectLinkExtractor, ExtractorError, LinkExtractor, RecursiveLinkExtractor,
};
use crate::core::{
    EntityCreator, EntityFetcher, LinkDefinition, LinkService,
    link::LinkEntity,
    query::{PaginationMeta, QueryParams},
};
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
    /// Optional event bus for publishing real-time events
    pub event_bus: Option<Arc<EventBus>>,
}

impl AppState {
    /// Publish an event to the event bus (if configured)
    ///
    /// This is non-blocking and fire-and-forget. If there are no subscribers
    /// or no event bus configured, the event is silently dropped.
    pub fn publish_event(&self, event: FrameworkEvent) {
        if let Some(ref bus) = self.event_bus {
            bus.publish(event);
        }
    }

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

/// Response for enriched list links endpoint (legacy, without pagination)
#[derive(Debug, Serialize)]
pub struct EnrichedListLinksResponse {
    pub links: Vec<EnrichedLink>,
    pub count: usize,
    pub link_type: String,
    pub direction: String,
    pub description: Option<String>,
}

/// Paginated response for enriched list links endpoint
#[derive(Debug, Serialize)]
pub struct PaginatedEnrichedLinksResponse {
    pub data: Vec<EnrichedLink>,
    pub pagination: PaginationMeta,
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
pub enum EnrichmentContext {
    /// Query from source entity - only target entities are included
    FromSource,
    /// Query from target entity - only source entities are included
    FromTarget,
    /// Direct link access - both source and target entities are included
    DirectLink,
}

/// List links using named routes (forward or reverse) - WITH PAGINATION
///
/// GET /{entity_type}/{entity_id}/{route_name}
pub async fn list_links(
    State(state): State<AppState>,
    Path((entity_type_plural, entity_id, route_name)): Path<(String, Uuid, String)>,
    Query(params): Query<QueryParams>,
) -> Result<Json<PaginatedEnrichedLinksResponse>, ExtractorError> {
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

    // Enrich ALL links with full entity data first
    let mut all_enriched =
        enrich_links_with_entities(&state, links, context, &extractor.link_definition).await?;

    // Apply filters if provided
    if let Some(filter_value) = params.filter_value() {
        all_enriched = apply_link_filters(all_enriched, &filter_value);
    }

    let total = all_enriched.len();

    // Apply pagination (ALWAYS paginate for links)
    let page = params.page();
    let limit = params.limit();
    let start = (page - 1) * limit;

    let paginated_links: Vec<EnrichedLink> =
        all_enriched.into_iter().skip(start).take(limit).collect();

    Ok(Json(PaginatedEnrichedLinksResponse {
        data: paginated_links,
        pagination: PaginationMeta::new(page, limit, total),
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

/// Apply filtering to enriched links based on query parameters
///
/// Supports filtering on:
/// - link fields (id, link_type, source_id, target_id, status, metadata)
/// - nested entity fields (source.*, target.*)
fn apply_link_filters(enriched_links: Vec<EnrichedLink>, filter: &Value) -> Vec<EnrichedLink> {
    if filter.is_null() || !filter.is_object() {
        return enriched_links;
    }

    let filter_obj = filter.as_object().unwrap();

    enriched_links
        .into_iter()
        .filter(|link| {
            let mut matches = true;

            // Convert link to JSON for easy filtering
            let link_json = match serde_json::to_value(link) {
                Ok(v) => v,
                Err(_) => return false,
            };

            for (key, value) in filter_obj.iter() {
                // Check if the field exists in the link or in nested entities
                let field_value = get_nested_value(&link_json, key);

                match field_value {
                    Some(field_val) => {
                        // Simple equality match for now
                        if field_val != *value {
                            matches = false;
                            break;
                        }
                    }
                    None => {
                        matches = false;
                        break;
                    }
                }
            }

            matches
        })
        .collect()
}

/// Get a nested value from JSON using dot notation
/// E.g., "source.name" or "target.amount"
fn get_nested_value(json: &Value, key: &str) -> Option<Value> {
    let parts: Vec<&str> = key.split('.').collect();

    match parts.len() {
        1 => json.get(key).cloned(),
        2 => {
            let (parent, child) = (parts[0], parts[1]);
            json.get(parent).and_then(|v| v.get(child)).cloned()
        }
        _ => None,
    }
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

    // Find the specific link based on direction
    let existing_links = match extractor.direction {
        LinkDirection::Forward => {
            // Forward: search by source_id in URL
            state
                .link_service
                .find_by_source(
                    &extractor.source_id,
                    Some(&extractor.link_definition.link_type),
                    Some(&extractor.target_type),
                )
                .await
                .map_err(|e| ExtractorError::JsonError(e.to_string()))?
        }
        LinkDirection::Reverse => {
            // Reverse: search by target_id in URL (which is the actual source in DB)
            state
                .link_service
                .find_by_source(
                    &extractor.target_id,
                    Some(&extractor.link_definition.link_type),
                    Some(&extractor.source_type),
                )
                .await
                .map_err(|e| ExtractorError::JsonError(e.to_string()))?
        }
    };

    let link = existing_links
        .into_iter()
        .find(|link| match extractor.direction {
            LinkDirection::Forward => link.target_id == extractor.target_id,
            LinkDirection::Reverse => link.target_id == extractor.source_id,
        })
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

    // Emit link created event
    state.publish_event(FrameworkEvent::Link(LinkEvent::Created {
        link_type: created_link.link_type.clone(),
        link_id: created_link.id,
        source_id: created_link.source_id,
        target_id: created_link.target_id,
        metadata: created_link.metadata.clone(),
    }));

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

    // Emit entity created event
    state.publish_event(FrameworkEvent::Entity(
        crate::core::events::EntityEvent::Created {
            entity_type: target_entity_type.clone(),
            entity_id: target_entity_id,
            data: created_entity.clone(),
        },
    ));

    // Emit link created event
    state.publish_event(FrameworkEvent::Link(LinkEvent::Created {
        link_type: created_link.link_type.clone(),
        link_id: created_link.id,
        source_id: created_link.source_id,
        target_id: created_link.target_id,
        metadata: created_link.metadata.clone(),
    }));

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

    // Emit link deleted event
    state.publish_event(FrameworkEvent::Link(LinkEvent::Deleted {
        link_type: existing_link.link_type.clone(),
        link_id: existing_link.id,
        source_id: existing_link.source_id,
        target_id: existing_link.target_id,
    }));

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

/// Handler générique pour GET sur chemins imbriqués illimités
///
/// Supporte des chemins comme:
/// - GET /users/123/invoices/456/orders (liste les orders)
/// - GET /users/123/invoices/456/orders/789 (get un order spécifique)
pub async fn handle_nested_path_get(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(params): Query<QueryParams>,
) -> Result<Json<serde_json::Value>, ExtractorError> {
    // Parser le path en segments
    let segments: Vec<String> = path
        .trim_matches('/')
        .split('/')
        .map(|s| s.to_string())
        .collect();

    // Cette route ne gère QUE les chemins imbriqués à 3+ niveaux (5+ segments)
    // Les chemins à 2 niveaux sont gérés par les routes spécifiques
    if segments.len() < 5 {
        return Err(ExtractorError::InvalidPath);
    }

    // Utiliser l'extracteur récursif
    let extractor =
        RecursiveLinkExtractor::from_segments(segments, &state.registry, &state.config)?;

    // Si is_list, récupérer les liens depuis la dernière entité
    if extractor.is_list {
        // Valider toute la chaîne de liens avant de retourner les résultats
        // Pour chaque segment avec un link_definition, vérifier que le lien existe
        // SAUF pour le dernier segment si c'est une liste (ID = Uuid::nil())

        use crate::links::registry::LinkDirection;

        // VALIDATION COMPLÈTE DE LA CHAÎNE
        for i in 0..extractor.chain.len() - 1 {
            let current = &extractor.chain[i];
            let next = &extractor.chain[i + 1];

            // Si next.entity_id est Uuid::nil(), c'est une liste finale, on ne valide pas ce lien
            if next.entity_id.is_nil() {
                continue;
            }

            // Cas 1: Le segment a un link_definition → validation normale
            if let Some(link_def) = &current.link_definition {
                let link_exists = match current.link_direction {
                    Some(LinkDirection::Forward) => {
                        // Forward: current est la source, next est le target
                        let links = state
                            .link_service
                            .find_by_source(
                                &current.entity_id,
                                Some(&link_def.link_type),
                                Some(&link_def.target_type),
                            )
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.target_id == next.entity_id)
                    }
                    Some(LinkDirection::Reverse) => {
                        // Reverse: current est le target, next est la source
                        let links = state
                            .link_service
                            .find_by_target(&current.entity_id, None, Some(&link_def.link_type))
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.source_id == next.entity_id)
                    }
                    None => {
                        return Err(ExtractorError::InvalidPath);
                    }
                };

                if !link_exists {
                    return Err(ExtractorError::LinkNotFound);
                }
            }
            // Cas 2: Premier segment sans link_definition mais next a un link_definition
            // → C'est le début d'une chaîne, on doit vérifier que current est lié à next
            else if let Some(next_link_def) = &next.link_definition {
                let link_exists = match next.link_direction {
                    Some(LinkDirection::Forward) => {
                        // Forward depuis current: current → next
                        let links = state
                            .link_service
                            .find_by_source(
                                &current.entity_id,
                                Some(&next_link_def.link_type),
                                Some(&next_link_def.target_type),
                            )
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.target_id == next.entity_id)
                    }
                    Some(LinkDirection::Reverse) => {
                        // Reverse depuis current: current ← next (donc next est source)
                        let links = state
                            .link_service
                            .find_by_target(
                                &current.entity_id,
                                None,
                                Some(&next_link_def.link_type),
                            )
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.source_id == next.entity_id)
                    }
                    None => {
                        return Err(ExtractorError::InvalidPath);
                    }
                };

                if !link_exists {
                    return Err(ExtractorError::LinkNotFound);
                }
            }
        }

        // Toute la chaîne est valide, récupérer les liens finaux
        if let Some(link_def) = extractor.final_link_def() {
            // Pour une liste, on veut l'ID du segment pénultième (celui qui a le lien)
            let penultimate = extractor
                .penultimate_segment()
                .ok_or(ExtractorError::InvalidPath)?;
            let entity_id = penultimate.entity_id;

            use crate::links::registry::LinkDirection;

            // Récupérer les liens selon la direction
            let (links, enrichment_context) = match penultimate.link_direction {
                Some(LinkDirection::Forward) => {
                    // Forward: entity_id est la source
                    let links = state
                        .link_service
                        .find_by_source(
                            &entity_id,
                            Some(&link_def.link_type),
                            Some(&link_def.target_type),
                        )
                        .await
                        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                    (links, EnrichmentContext::FromSource)
                }
                Some(LinkDirection::Reverse) => {
                    // Reverse: entity_id est le target, on cherche les sources
                    let links = state
                        .link_service
                        .find_by_target(&entity_id, None, Some(&link_def.link_type))
                        .await
                        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                    (links, EnrichmentContext::FromTarget)
                }
                None => {
                    return Err(ExtractorError::InvalidPath);
                }
            };

            // Enrichir TOUS les liens
            let mut all_enriched =
                enrich_links_with_entities(&state, links, enrichment_context, link_def).await?;

            // Apply filters if provided
            if let Some(filter_value) = params.filter_value() {
                all_enriched = apply_link_filters(all_enriched, &filter_value);
            }

            let total = all_enriched.len();

            // Apply pagination (ALWAYS paginate for nested links too)
            let page = params.page();
            let limit = params.limit();
            let start = (page - 1) * limit;

            let paginated_links: Vec<EnrichedLink> =
                all_enriched.into_iter().skip(start).take(limit).collect();

            Ok(Json(serde_json::json!({
                "data": paginated_links,
                "pagination": {
                    "page": page,
                    "limit": limit,
                    "total": total,
                    "total_pages": PaginationMeta::new(page, limit, total).total_pages,
                    "has_next": PaginationMeta::new(page, limit, total).has_next,
                    "has_prev": PaginationMeta::new(page, limit, total).has_prev
                },
                "link_type": link_def.link_type,
                "direction": format!("{:?}", penultimate.link_direction),
                "description": link_def.description
            })))
        } else {
            Err(ExtractorError::InvalidPath)
        }
    } else {
        // Item spécifique - récupérer le lien spécifique

        use crate::links::registry::LinkDirection;

        // VALIDATION COMPLÈTE DE LA CHAÎNE (aussi pour items spécifiques)
        for i in 0..extractor.chain.len() - 1 {
            let current = &extractor.chain[i];
            let next = &extractor.chain[i + 1];

            // Cas 1: Le segment a un link_definition → validation normale
            if let Some(link_def) = &current.link_definition {
                let link_exists = match current.link_direction {
                    Some(LinkDirection::Forward) => {
                        let links = state
                            .link_service
                            .find_by_source(
                                &current.entity_id,
                                Some(&link_def.link_type),
                                Some(&link_def.target_type),
                            )
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.target_id == next.entity_id)
                    }
                    Some(LinkDirection::Reverse) => {
                        let links = state
                            .link_service
                            .find_by_target(&current.entity_id, None, Some(&link_def.link_type))
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.source_id == next.entity_id)
                    }
                    None => {
                        return Err(ExtractorError::InvalidPath);
                    }
                };

                if !link_exists {
                    return Err(ExtractorError::LinkNotFound);
                }
            }
            // Cas 2: Premier segment sans link_definition mais next a un link_definition
            else if let Some(next_link_def) = &next.link_definition {
                let link_exists = match next.link_direction {
                    Some(LinkDirection::Forward) => {
                        let links = state
                            .link_service
                            .find_by_source(
                                &current.entity_id,
                                Some(&next_link_def.link_type),
                                Some(&next_link_def.target_type),
                            )
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.target_id == next.entity_id)
                    }
                    Some(LinkDirection::Reverse) => {
                        let links = state
                            .link_service
                            .find_by_target(
                                &current.entity_id,
                                None,
                                Some(&next_link_def.link_type),
                            )
                            .await
                            .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
                        links.iter().any(|l| l.source_id == next.entity_id)
                    }
                    None => {
                        return Err(ExtractorError::InvalidPath);
                    }
                };

                if !link_exists {
                    return Err(ExtractorError::LinkNotFound);
                }
            }
        }

        // Toute la chaîne est validée, récupérer le lien final
        if let Some(link_def) = extractor.final_link_def() {
            let (target_id, _) = extractor.final_target();
            let penultimate = extractor.penultimate_segment().unwrap();

            // Récupérer le lien selon la direction
            let link = match penultimate.link_direction {
                Some(LinkDirection::Forward) => {
                    // Forward: penultimate est la source, target_id est le target
                    let links = state
                        .link_service
                        .find_by_source(
                            &penultimate.entity_id,
                            Some(&link_def.link_type),
                            Some(&link_def.target_type),
                        )
                        .await
                        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

                    links
                        .into_iter()
                        .find(|l| l.target_id == target_id)
                        .ok_or(ExtractorError::LinkNotFound)?
                }
                Some(LinkDirection::Reverse) => {
                    // Reverse: penultimate est le target, target_id est la source
                    let links = state
                        .link_service
                        .find_by_target(&penultimate.entity_id, None, Some(&link_def.link_type))
                        .await
                        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

                    links
                        .into_iter()
                        .find(|l| l.source_id == target_id)
                        .ok_or(ExtractorError::LinkNotFound)?
                }
                None => {
                    return Err(ExtractorError::InvalidPath);
                }
            };

            // Enrichir le lien
            let enriched = enrich_links_with_entities(
                &state,
                vec![link],
                EnrichmentContext::DirectLink,
                link_def,
            )
            .await?;

            Ok(Json(serde_json::json!({
                "link": enriched.first()
            })))
        } else {
            Err(ExtractorError::InvalidPath)
        }
    }
}

/// Handler générique pour POST sur chemins imbriqués
///
/// Supporte des chemins comme:
/// - POST /users/123/invoices/456/orders (crée un nouvel order + link)
pub async fn handle_nested_path_post(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Json(payload): Json<CreateLinkedEntityRequest>,
) -> Result<Response, ExtractorError> {
    let segments: Vec<String> = path
        .trim_matches('/')
        .split('/')
        .map(|s| s.to_string())
        .collect();

    // Cette route ne gère QUE les chemins imbriqués à 3+ niveaux (5+ segments)
    // Les chemins à 2 niveaux sont gérés par les routes spécifiques
    if segments.len() < 5 {
        return Err(ExtractorError::InvalidPath);
    }

    let extractor =
        RecursiveLinkExtractor::from_segments(segments, &state.registry, &state.config)?;

    // Récupérer le dernier lien
    let link_def = extractor
        .final_link_def()
        .ok_or(ExtractorError::InvalidPath)?;

    let (source_id, _) = extractor.final_target();
    let target_entity_type = &link_def.target_type;

    // Récupérer le creator pour l'entité target
    let entity_creator = state
        .entity_creators
        .get(target_entity_type)
        .ok_or_else(|| {
            ExtractorError::JsonError(format!(
                "No entity creator registered for type: {}",
                target_entity_type
            ))
        })?;

    // Créer la nouvelle entité
    let created_entity = entity_creator
        .create_from_json(payload.entity)
        .await
        .map_err(|e| ExtractorError::JsonError(format!("Failed to create entity: {}", e)))?;

    // Extraire l'ID de l'entité créée
    let target_entity_id = created_entity["id"].as_str().ok_or_else(|| {
        ExtractorError::JsonError("Created entity missing 'id' field".to_string())
    })?;
    let target_entity_id = Uuid::parse_str(target_entity_id)
        .map_err(|e| ExtractorError::JsonError(format!("Invalid UUID in created entity: {}", e)))?;

    // Créer le lien
    let link = LinkEntity::new(
        link_def.link_type.clone(),
        source_id,
        target_entity_id,
        payload.metadata,
    );

    let created_link = state
        .link_service
        .create(link)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    // Emit entity created event
    state.publish_event(FrameworkEvent::Entity(
        crate::core::events::EntityEvent::Created {
            entity_type: target_entity_type.clone(),
            entity_id: target_entity_id,
            data: created_entity.clone(),
        },
    ));

    // Emit link created event
    state.publish_event(FrameworkEvent::Link(LinkEvent::Created {
        link_type: created_link.link_type.clone(),
        link_id: created_link.id,
        source_id: created_link.source_id,
        target_id: created_link.target_id,
        metadata: created_link.metadata.clone(),
    }));

    let response = serde_json::json!({
        "entity": created_entity,
        "link": created_link,
    });

    Ok((StatusCode::CREATED, Json(response)).into_response())
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
            event_bus: None,
        }
    }

    #[test]
    fn test_state_creation() {
        let state = create_test_state();
        assert_eq!(state.config.entities.len(), 2);
        assert_eq!(state.config.links.len(), 1);
    }

    // ======================================================================
    // Phase 1: Pure helper function tests
    // ======================================================================

    // ------------------------------------------------------------------
    // get_nested_value
    // ------------------------------------------------------------------

    #[test]
    fn test_get_nested_value_top_level_key() {
        let json = serde_json::json!({
            "name": "Alice",
            "age": 30
        });
        let result = get_nested_value(&json, "name");
        assert_eq!(
            result,
            Some(serde_json::Value::String("Alice".to_string())),
            "should retrieve top-level string value"
        );
    }

    #[test]
    fn test_get_nested_value_top_level_number() {
        let json = serde_json::json!({ "count": 42 });
        let result = get_nested_value(&json, "count");
        assert_eq!(
            result,
            Some(serde_json::json!(42)),
            "should retrieve top-level numeric value"
        );
    }

    #[test]
    fn test_get_nested_value_missing_top_level_key() {
        let json = serde_json::json!({ "name": "Alice" });
        let result = get_nested_value(&json, "missing");
        assert_eq!(result, None, "missing top-level key should return None");
    }

    #[test]
    fn test_get_nested_value_two_level_path() {
        let json = serde_json::json!({
            "source": { "name": "Alice", "email": "alice@example.com" },
            "target": { "amount": 100 }
        });
        let result = get_nested_value(&json, "source.name");
        assert_eq!(
            result,
            Some(serde_json::Value::String("Alice".to_string())),
            "should navigate two-level dot path"
        );
    }

    #[test]
    fn test_get_nested_value_two_level_numeric() {
        let json = serde_json::json!({
            "target": { "amount": 99.5 }
        });
        let result = get_nested_value(&json, "target.amount");
        assert_eq!(
            result,
            Some(serde_json::json!(99.5)),
            "should retrieve nested numeric value"
        );
    }

    #[test]
    fn test_get_nested_value_missing_parent() {
        let json = serde_json::json!({ "name": "Alice" });
        let result = get_nested_value(&json, "source.name");
        assert_eq!(result, None, "missing parent should return None");
    }

    #[test]
    fn test_get_nested_value_missing_child() {
        let json = serde_json::json!({ "source": { "name": "Alice" } });
        let result = get_nested_value(&json, "source.missing");
        assert_eq!(result, None, "missing child key should return None");
    }

    #[test]
    fn test_get_nested_value_three_levels_returns_none() {
        let json = serde_json::json!({
            "a": { "b": { "c": "deep" } }
        });
        let result = get_nested_value(&json, "a.b.c");
        assert_eq!(
            result, None,
            "three-level dot path should return None (only 1 or 2 levels supported)"
        );
    }

    #[test]
    fn test_get_nested_value_null_value() {
        let json = serde_json::json!({ "field": null });
        let result = get_nested_value(&json, "field");
        assert_eq!(
            result,
            Some(serde_json::Value::Null),
            "null values should be returned as Some(Null)"
        );
    }

    #[test]
    fn test_get_nested_value_boolean() {
        let json = serde_json::json!({ "active": true });
        let result = get_nested_value(&json, "active");
        assert_eq!(
            result,
            Some(serde_json::json!(true)),
            "should retrieve boolean value"
        );
    }

    // ------------------------------------------------------------------
    // apply_link_filters
    // ------------------------------------------------------------------

    /// Helper to create an EnrichedLink with configurable fields
    fn make_enriched_link(
        link_type: &str,
        status: &str,
        target: Option<serde_json::Value>,
        source: Option<serde_json::Value>,
        metadata: Option<serde_json::Value>,
    ) -> EnrichedLink {
        EnrichedLink {
            id: Uuid::new_v4(),
            entity_type: "link".to_string(),
            link_type: link_type.to_string(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            source,
            target,
            metadata,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            status: status.to_string(),
        }
    }

    #[test]
    fn test_apply_link_filters_null_filter_returns_all() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
            make_enriched_link("driver", "active", None, None, None),
        ];
        let result = apply_link_filters(links, &serde_json::Value::Null);
        assert_eq!(result.len(), 2, "null filter should return all links");
    }

    #[test]
    fn test_apply_link_filters_non_object_filter_returns_all() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
        ];
        let result = apply_link_filters(links, &serde_json::json!("not an object"));
        assert_eq!(result.len(), 1, "non-object filter should return all links");
    }

    #[test]
    fn test_apply_link_filters_empty_object_returns_all() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
            make_enriched_link("driver", "inactive", None, None, None),
        ];
        let result = apply_link_filters(links, &serde_json::json!({}));
        assert_eq!(result.len(), 2, "empty object filter should return all links");
    }

    #[test]
    fn test_apply_link_filters_by_status() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
            make_enriched_link("driver", "inactive", None, None, None),
            make_enriched_link("owner", "active", None, None, None),
        ];
        let filter = serde_json::json!({ "status": "active" });
        let result = apply_link_filters(links, &filter);
        assert_eq!(result.len(), 2, "should filter to only active links");
        for link in &result {
            assert_eq!(link.status, "active");
        }
    }

    #[test]
    fn test_apply_link_filters_by_link_type() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
            make_enriched_link("driver", "active", None, None, None),
            make_enriched_link("owner", "active", None, None, None),
        ];
        let filter = serde_json::json!({ "link_type": "owner" });
        let result = apply_link_filters(links, &filter);
        assert_eq!(result.len(), 2, "should filter to only 'owner' links");
    }

    #[test]
    fn test_apply_link_filters_by_nested_target_field() {
        let links = vec![
            make_enriched_link(
                "owner",
                "active",
                Some(serde_json::json!({ "name": "Car A" })),
                None,
                None,
            ),
            make_enriched_link(
                "owner",
                "active",
                Some(serde_json::json!({ "name": "Car B" })),
                None,
                None,
            ),
        ];
        let filter = serde_json::json!({ "target.name": "Car A" });
        let result = apply_link_filters(links, &filter);
        assert_eq!(result.len(), 1, "should filter by nested target.name");
    }

    #[test]
    fn test_apply_link_filters_by_nested_source_field() {
        let links = vec![
            make_enriched_link(
                "owner",
                "active",
                None,
                Some(serde_json::json!({ "email": "alice@test.com" })),
                None,
            ),
            make_enriched_link(
                "owner",
                "active",
                None,
                Some(serde_json::json!({ "email": "bob@test.com" })),
                None,
            ),
        ];
        let filter = serde_json::json!({ "source.email": "bob@test.com" });
        let result = apply_link_filters(links, &filter);
        assert_eq!(result.len(), 1, "should filter by nested source.email");
    }

    #[test]
    fn test_apply_link_filters_multiple_criteria() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
            make_enriched_link("owner", "inactive", None, None, None),
            make_enriched_link("driver", "active", None, None, None),
        ];
        let filter = serde_json::json!({ "link_type": "owner", "status": "active" });
        let result = apply_link_filters(links, &filter);
        assert_eq!(
            result.len(),
            1,
            "should filter by both link_type AND status"
        );
        assert_eq!(result[0].link_type, "owner");
        assert_eq!(result[0].status, "active");
    }

    #[test]
    fn test_apply_link_filters_no_match_returns_empty() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
        ];
        let filter = serde_json::json!({ "status": "deleted" });
        let result = apply_link_filters(links, &filter);
        assert!(result.is_empty(), "non-matching filter should return empty vec");
    }

    #[test]
    fn test_apply_link_filters_missing_field_excludes_link() {
        let links = vec![
            make_enriched_link("owner", "active", None, None, None),
        ];
        // "nonexistent_field" does not exist on EnrichedLink serialization
        let filter = serde_json::json!({ "nonexistent_field": "value" });
        let result = apply_link_filters(links, &filter);
        assert!(
            result.is_empty(),
            "filtering by a missing field should exclude the link"
        );
    }

    // ------------------------------------------------------------------
    // get_link_auth_policy
    // ------------------------------------------------------------------

    fn make_link_def_with_auth() -> LinkDefinition {
        use crate::core::link::LinkAuthConfig;
        LinkDefinition {
            link_type: "test".to_string(),
            source_type: "a".to_string(),
            target_type: "b".to_string(),
            forward_route_name: "bs".to_string(),
            reverse_route_name: "as".to_string(),
            description: None,
            required_fields: None,
            auth: Some(LinkAuthConfig {
                list: "public".to_string(),
                get: "authenticated".to_string(),
                create: "admin_only".to_string(),
                update: "owner".to_string(),
                delete: "service_only".to_string(),
            }),
        }
    }

    #[test]
    fn test_get_link_auth_policy_list() {
        let def = make_link_def_with_auth();
        let result = AppState::get_link_auth_policy(&def, "list");
        assert_eq!(
            result,
            Some("public".to_string()),
            "list operation should return 'public' policy"
        );
    }

    #[test]
    fn test_get_link_auth_policy_get() {
        let def = make_link_def_with_auth();
        let result = AppState::get_link_auth_policy(&def, "get");
        assert_eq!(result, Some("authenticated".to_string()));
    }

    #[test]
    fn test_get_link_auth_policy_create() {
        let def = make_link_def_with_auth();
        let result = AppState::get_link_auth_policy(&def, "create");
        assert_eq!(result, Some("admin_only".to_string()));
    }

    #[test]
    fn test_get_link_auth_policy_update() {
        let def = make_link_def_with_auth();
        let result = AppState::get_link_auth_policy(&def, "update");
        assert_eq!(result, Some("owner".to_string()));
    }

    #[test]
    fn test_get_link_auth_policy_delete() {
        let def = make_link_def_with_auth();
        let result = AppState::get_link_auth_policy(&def, "delete");
        assert_eq!(result, Some("service_only".to_string()));
    }

    #[test]
    fn test_get_link_auth_policy_unknown_operation() {
        let def = make_link_def_with_auth();
        let result = AppState::get_link_auth_policy(&def, "unknown_op");
        assert_eq!(
            result,
            Some("authenticated".to_string()),
            "unknown operations should default to 'authenticated'"
        );
    }

    #[test]
    fn test_get_link_auth_policy_no_auth_config() {
        let def = LinkDefinition {
            link_type: "test".to_string(),
            source_type: "a".to_string(),
            target_type: "b".to_string(),
            forward_route_name: "bs".to_string(),
            reverse_route_name: "as".to_string(),
            description: None,
            required_fields: None,
            auth: None,
        };
        let result = AppState::get_link_auth_policy(&def, "list");
        assert_eq!(
            result, None,
            "should return None when no auth config is set"
        );
    }

    // ------------------------------------------------------------------
    // publish_event (fire-and-forget, no event bus)
    // ------------------------------------------------------------------

    #[test]
    fn test_publish_event_no_event_bus_does_not_panic() {
        let state = create_test_state();
        // event_bus is None — should silently drop
        state.publish_event(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "owner".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        }));
        // If we reach here without panic, the test passes
    }

    #[test]
    fn test_publish_event_with_event_bus() {
        let bus = Arc::new(EventBus::new(16));
        let mut state = create_test_state();
        state.event_bus = Some(bus.clone());

        let mut rx = bus.subscribe();

        state.publish_event(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "owner".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        }));

        // The event should be receivable
        let envelope = rx.try_recv().expect("should receive published event");
        assert!(
            matches!(envelope.event, FrameworkEvent::Link(LinkEvent::Created { .. })),
            "received event should be a Link::Created"
        );
    }

    // ======================================================================
    // Phase 2: Handler tests with InMemoryLinkService
    // ======================================================================

    /// Extended test config with order -> invoice -> payment chain
    fn create_chain_test_state() -> AppState {
        let config = Arc::new(LinksConfig {
            entities: vec![
                EntityConfig {
                    singular: "order".to_string(),
                    plural: "orders".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "invoice".to_string(),
                    plural: "invoices".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "payment".to_string(),
                    plural: "payments".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
            ],
            links: vec![
                LinkDefinition {
                    link_type: "billing".to_string(),
                    source_type: "order".to_string(),
                    target_type: "invoice".to_string(),
                    forward_route_name: "invoices".to_string(),
                    reverse_route_name: "order".to_string(),
                    description: Some("Order has invoices".to_string()),
                    required_fields: None,
                    auth: None,
                },
                LinkDefinition {
                    link_type: "payment".to_string(),
                    source_type: "invoice".to_string(),
                    target_type: "payment".to_string(),
                    forward_route_name: "payments".to_string(),
                    reverse_route_name: "invoice".to_string(),
                    description: None,
                    required_fields: None,
                    auth: None,
                },
            ],
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
            event_bus: None,
        }
    }

    /// Simple EntityFetcher for tests that returns a JSON object with id and name
    struct MockEntityFetcher {
        entities: std::sync::RwLock<HashMap<Uuid, serde_json::Value>>,
    }

    impl MockEntityFetcher {
        fn new() -> Self {
            Self {
                entities: std::sync::RwLock::new(HashMap::new()),
            }
        }

        fn insert(&self, id: Uuid, data: serde_json::Value) {
            self.entities
                .write()
                .expect("lock should not be poisoned")
                .insert(id, data);
        }
    }

    #[async_trait::async_trait]
    impl crate::core::EntityFetcher for MockEntityFetcher {
        async fn fetch_as_json(&self, entity_id: &Uuid) -> anyhow::Result<serde_json::Value> {
            let entities = self
                .entities
                .read()
                .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
            entities
                .get(entity_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Entity not found: {}", entity_id))
        }
    }

    /// Simple EntityCreator for tests that returns the input with a generated id
    struct MockEntityCreator;

    #[async_trait::async_trait]
    impl crate::core::EntityCreator for MockEntityCreator {
        async fn create_from_json(
            &self,
            entity_data: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            let mut data = entity_data;
            if data.get("id").is_none() {
                data["id"] = serde_json::json!(Uuid::new_v4().to_string());
            }
            Ok(data)
        }
    }

    // ------------------------------------------------------------------
    // enrich_links_with_entities
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_enrich_links_from_source_omits_source() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();
        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);

        let link_def = &state.config.links[0];
        let enriched = enrich_links_with_entities(
            &state,
            vec![link],
            EnrichmentContext::FromSource,
            link_def,
        )
        .await
        .expect("enrichment should succeed");

        assert_eq!(enriched.len(), 1);
        assert!(
            enriched[0].source.is_none(),
            "FromSource context should omit source entity"
        );
        // No fetcher registered, so target will also be None (fetcher not found)
        assert!(enriched[0].target.is_none());
    }

    #[tokio::test]
    async fn test_enrich_links_from_target_omits_target() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();
        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);

        let link_def = &state.config.links[0];
        let enriched = enrich_links_with_entities(
            &state,
            vec![link],
            EnrichmentContext::FromTarget,
            link_def,
        )
        .await
        .expect("enrichment should succeed");

        assert_eq!(enriched.len(), 1);
        assert!(
            enriched[0].target.is_none(),
            "FromTarget context should omit target entity"
        );
    }

    #[tokio::test]
    async fn test_enrich_links_with_fetcher_includes_entity() {
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let car_fetcher = Arc::new(MockEntityFetcher::new());
        car_fetcher.insert(car_id, serde_json::json!({ "id": car_id.to_string(), "model": "Tesla" }));

        let mut fetchers: HashMap<String, Arc<dyn crate::core::EntityFetcher>> = HashMap::new();
        fetchers.insert("car".to_string(), car_fetcher);

        let mut state = create_test_state();
        state.entity_fetchers = Arc::new(fetchers);

        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        let link_def = &state.config.links[0];

        let enriched = enrich_links_with_entities(
            &state,
            vec![link],
            EnrichmentContext::FromSource,
            link_def,
        )
        .await
        .expect("enrichment should succeed");

        assert_eq!(enriched.len(), 1);
        let target = enriched[0]
            .target
            .as_ref()
            .expect("target entity should be fetched");
        assert_eq!(target["model"], "Tesla");
    }

    #[tokio::test]
    async fn test_enrich_links_direct_link_context() {
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let user_fetcher = Arc::new(MockEntityFetcher::new());
        user_fetcher.insert(user_id, serde_json::json!({ "id": user_id.to_string(), "name": "Alice" }));

        let car_fetcher = Arc::new(MockEntityFetcher::new());
        car_fetcher.insert(car_id, serde_json::json!({ "id": car_id.to_string(), "model": "BMW" }));

        let mut fetchers: HashMap<String, Arc<dyn crate::core::EntityFetcher>> = HashMap::new();
        fetchers.insert("user".to_string(), user_fetcher);
        fetchers.insert("car".to_string(), car_fetcher);

        let mut state = create_test_state();
        state.entity_fetchers = Arc::new(fetchers);

        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        let link_def = &state.config.links[0];

        let enriched = enrich_links_with_entities(
            &state,
            vec![link],
            EnrichmentContext::DirectLink,
            link_def,
        )
        .await
        .expect("enrichment should succeed");

        assert_eq!(enriched.len(), 1);
        assert!(
            enriched[0].source.is_some(),
            "DirectLink context should include source"
        );
        assert!(
            enriched[0].target.is_some(),
            "DirectLink context should include target"
        );
        assert_eq!(enriched[0].source.as_ref().expect("source")["name"], "Alice");
        assert_eq!(enriched[0].target.as_ref().expect("target")["model"], "BMW");
    }

    #[tokio::test]
    async fn test_enrich_links_preserves_metadata() {
        let state = create_test_state();
        let metadata = serde_json::json!({ "role": "primary" });
        let link = crate::core::link::LinkEntity::new(
            "owner",
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some(metadata.clone()),
        );

        let link_def = &state.config.links[0];
        let enriched = enrich_links_with_entities(
            &state,
            vec![link],
            EnrichmentContext::FromSource,
            link_def,
        )
        .await
        .expect("enrichment should succeed");

        assert_eq!(enriched[0].metadata, Some(metadata));
    }

    #[tokio::test]
    async fn test_enrich_links_empty_input() {
        let state = create_test_state();
        let link_def = &state.config.links[0];
        let enriched = enrich_links_with_entities(
            &state,
            vec![],
            EnrichmentContext::FromSource,
            link_def,
        )
        .await
        .expect("enrichment should succeed");
        assert!(enriched.is_empty(), "enriching empty vec should return empty vec");
    }

    // ------------------------------------------------------------------
    // fetch_entity_by_type
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_fetch_entity_by_type_no_fetcher_registered() {
        let state = create_test_state();
        let result = fetch_entity_by_type(&state, "unknown_type", &Uuid::new_v4()).await;
        assert!(result.is_err(), "should error when no fetcher is registered");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("No entity fetcher registered"),
            "error should mention missing fetcher, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_fetch_entity_by_type_entity_not_found() {
        let fetcher = Arc::new(MockEntityFetcher::new());
        let mut fetchers: HashMap<String, Arc<dyn crate::core::EntityFetcher>> = HashMap::new();
        fetchers.insert("car".to_string(), fetcher);

        let mut state = create_test_state();
        state.entity_fetchers = Arc::new(fetchers);

        let result = fetch_entity_by_type(&state, "car", &Uuid::new_v4()).await;
        assert!(result.is_err(), "should error when entity is not found");
    }

    #[tokio::test]
    async fn test_fetch_entity_by_type_success() {
        let car_id = Uuid::new_v4();
        let fetcher = Arc::new(MockEntityFetcher::new());
        fetcher.insert(car_id, serde_json::json!({ "id": car_id.to_string(), "model": "Audi" }));

        let mut fetchers: HashMap<String, Arc<dyn crate::core::EntityFetcher>> = HashMap::new();
        fetchers.insert("car".to_string(), fetcher);

        let mut state = create_test_state();
        state.entity_fetchers = Arc::new(fetchers);

        let result = fetch_entity_by_type(&state, "car", &car_id)
            .await
            .expect("should succeed");
        assert_eq!(result["model"], "Audi");
    }

    // ------------------------------------------------------------------
    // Handler: list_links
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_links_forward_empty() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();

        let result = list_links(
            State(state),
            Path(("users".to_string(), user_id, "cars-owned".to_string())),
            Query(crate::core::query::QueryParams::default()),
        )
        .await
        .expect("handler should succeed");

        let resp = result.0;
        assert_eq!(resp.data.len(), 0);
        assert_eq!(resp.pagination.total, 0);
        assert_eq!(resp.link_type, "owner");
        assert_eq!(resp.direction, "Forward");
    }

    #[tokio::test]
    async fn test_list_links_forward_with_links() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        // Create two links
        let link1 = crate::core::link::LinkEntity::new("owner", user_id, car1_id, None);
        let link2 = crate::core::link::LinkEntity::new("owner", user_id, car2_id, None);
        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");
        state
            .link_service
            .create(link2)
            .await
            .expect("create should succeed");

        let result = list_links(
            State(state),
            Path(("users".to_string(), user_id, "cars-owned".to_string())),
            Query(crate::core::query::QueryParams::default()),
        )
        .await
        .expect("handler should succeed");

        let resp = result.0;
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.pagination.total, 2);
    }

    #[tokio::test]
    async fn test_list_links_reverse() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        state
            .link_service
            .create(link)
            .await
            .expect("create should succeed");

        let result = list_links(
            State(state),
            Path(("cars".to_string(), car_id, "users-owners".to_string())),
            Query(crate::core::query::QueryParams::default()),
        )
        .await
        .expect("handler should succeed");

        let resp = result.0;
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.direction, "Reverse");
    }

    #[tokio::test]
    async fn test_list_links_invalid_route() {
        let state = create_test_state();
        let result = list_links(
            State(state),
            Path((
                "users".to_string(),
                Uuid::new_v4(),
                "nonexistent".to_string(),
            )),
            Query(crate::core::query::QueryParams::default()),
        )
        .await;

        assert!(result.is_err(), "should fail with invalid route");
    }

    #[tokio::test]
    async fn test_list_links_pagination() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();

        // Create 5 links
        for _ in 0..5 {
            let car_id = Uuid::new_v4();
            let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
            state
                .link_service
                .create(link)
                .await
                .expect("create should succeed");
        }

        let params = crate::core::query::QueryParams {
            page: 1,
            limit: 2,
            filter: None,
            sort: None,
        };

        let result = list_links(
            State(state),
            Path(("users".to_string(), user_id, "cars-owned".to_string())),
            Query(params),
        )
        .await
        .expect("handler should succeed");

        let resp = result.0;
        assert_eq!(resp.data.len(), 2, "page 1 should have 2 items");
        assert_eq!(resp.pagination.total, 5);
        assert_eq!(resp.pagination.total_pages, 3);
        assert!(resp.pagination.has_next);
        assert!(!resp.pagination.has_prev);
    }

    #[tokio::test]
    async fn test_list_links_with_filter() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();

        // Create links: two "owner" links from the same user
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();
        let link1 = crate::core::link::LinkEntity::new("owner", user_id, car1_id, None);
        let mut link2 = crate::core::link::LinkEntity::new("owner", user_id, car2_id, None);
        link2.status = "inactive".to_string();

        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");
        state
            .link_service
            .create(link2)
            .await
            .expect("create should succeed");

        let params = crate::core::query::QueryParams {
            page: 1,
            limit: 20,
            filter: Some(r#"{"status": "active"}"#.to_string()),
            sort: None,
        };

        let result = list_links(
            State(state),
            Path(("users".to_string(), user_id, "cars-owned".to_string())),
            Query(params),
        )
        .await
        .expect("handler should succeed");

        let resp = result.0;
        assert_eq!(resp.data.len(), 1, "filter should return only active links");
        assert_eq!(resp.data[0].status, "active");
    }

    // ------------------------------------------------------------------
    // Handler: get_link
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_link_not_found() {
        let state = create_test_state();
        let result = get_link(State(state), Path(Uuid::new_v4())).await;
        assert!(result.is_err(), "should fail for nonexistent link");
    }

    #[tokio::test]
    async fn test_get_link_success() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();
        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        let link_id = link.id;

        state
            .link_service
            .create(link)
            .await
            .expect("create should succeed");

        let result = get_link(State(state), Path(link_id)).await;
        assert!(result.is_ok(), "should succeed for existing link");
    }

    // ------------------------------------------------------------------
    // Handler: create_link
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_link_success() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let result = create_link(
            State(state.clone()),
            Path((
                "users".to_string(),
                user_id,
                "cars-owned".to_string(),
                car_id,
            )),
            Json(CreateLinkRequest { metadata: None }),
        )
        .await;

        assert!(result.is_ok(), "create_link should succeed");
        let response = result.expect("should be ok");
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify the link exists in the service
        let links = state
            .link_service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .expect("find_by_source should succeed");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_id, user_id);
        assert_eq!(links[0].target_id, car_id);
    }

    #[tokio::test]
    async fn test_create_link_with_metadata() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let metadata = serde_json::json!({ "primary_owner": true });

        let result = create_link(
            State(state.clone()),
            Path((
                "users".to_string(),
                user_id,
                "cars-owned".to_string(),
                car_id,
            )),
            Json(CreateLinkRequest {
                metadata: Some(metadata.clone()),
            }),
        )
        .await;

        assert!(result.is_ok());

        let links = state
            .link_service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .expect("find_by_source should succeed");
        assert_eq!(links[0].metadata, Some(metadata));
    }

    #[tokio::test]
    async fn test_create_link_invalid_route() {
        let state = create_test_state();
        let result = create_link(
            State(state),
            Path((
                "users".to_string(),
                Uuid::new_v4(),
                "nonexistent".to_string(),
                Uuid::new_v4(),
            )),
            Json(CreateLinkRequest { metadata: None }),
        )
        .await;

        assert!(result.is_err(), "should fail with invalid route");
    }

    // ------------------------------------------------------------------
    // Handler: delete_link
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_delete_link_success() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        // First create a link
        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        state
            .link_service
            .create(link)
            .await
            .expect("create should succeed");

        // Delete it
        let result = delete_link(
            State(state.clone()),
            Path((
                "users".to_string(),
                user_id,
                "cars-owned".to_string(),
                car_id,
            )),
        )
        .await;

        assert!(result.is_ok(), "delete_link should succeed");
        let response = result.expect("should be ok");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Verify link is gone
        let links = state
            .link_service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .expect("find_by_source should succeed");
        assert!(links.is_empty(), "link should be deleted");
    }

    #[tokio::test]
    async fn test_delete_link_not_found() {
        let state = create_test_state();
        let result = delete_link(
            State(state),
            Path((
                "users".to_string(),
                Uuid::new_v4(),
                "cars-owned".to_string(),
                Uuid::new_v4(),
            )),
        )
        .await;

        assert!(result.is_err(), "should fail when link does not exist");
    }

    // ------------------------------------------------------------------
    // Handler: create_linked_entity
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_linked_entity_success() {
        let mut state = create_test_state();
        let user_id = Uuid::new_v4();

        // Register a mock creator for "car"
        let mut creators: HashMap<String, Arc<dyn crate::core::EntityCreator>> = HashMap::new();
        creators.insert("car".to_string(), Arc::new(MockEntityCreator));
        state.entity_creators = Arc::new(creators);

        let entity_data = serde_json::json!({ "model": "Tesla Model 3", "year": 2024 });

        let result = create_linked_entity(
            State(state.clone()),
            Path(("users".to_string(), user_id, "cars-owned".to_string())),
            Json(CreateLinkedEntityRequest {
                entity: entity_data,
                metadata: None,
            }),
        )
        .await;

        assert!(result.is_ok(), "create_linked_entity should succeed");
        let response = result.expect("should be ok");
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify link was created
        let links = state
            .link_service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .expect("find_by_source should succeed");
        assert_eq!(links.len(), 1, "a link should have been created");
    }

    #[tokio::test]
    async fn test_create_linked_entity_no_creator_registered() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();

        let result = create_linked_entity(
            State(state),
            Path(("users".to_string(), user_id, "cars-owned".to_string())),
            Json(CreateLinkedEntityRequest {
                entity: serde_json::json!({}),
                metadata: None,
            }),
        )
        .await;

        assert!(
            result.is_err(),
            "should fail when no entity creator is registered"
        );
    }

    // ------------------------------------------------------------------
    // Handler: update_link
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_update_link_success() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        state
            .link_service
            .create(link)
            .await
            .expect("create should succeed");

        let new_metadata = serde_json::json!({ "insured": true });

        let result = update_link(
            State(state.clone()),
            Path((
                "users".to_string(),
                user_id,
                "cars-owned".to_string(),
                car_id,
            )),
            Json(CreateLinkRequest {
                metadata: Some(new_metadata.clone()),
            }),
        )
        .await;

        assert!(result.is_ok(), "update_link should succeed");

        // Verify metadata updated
        let links = state
            .link_service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .expect("find_by_source should succeed");
        assert_eq!(links[0].metadata, Some(new_metadata));
    }

    #[tokio::test]
    async fn test_update_link_not_found() {
        let state = create_test_state();
        let result = update_link(
            State(state),
            Path((
                "users".to_string(),
                Uuid::new_v4(),
                "cars-owned".to_string(),
                Uuid::new_v4(),
            )),
            Json(CreateLinkRequest { metadata: None }),
        )
        .await;

        assert!(result.is_err(), "should fail when link does not exist");
    }

    // ------------------------------------------------------------------
    // Handler: get_link_by_route
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_link_by_route_forward_success() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        state
            .link_service
            .create(link)
            .await
            .expect("create should succeed");

        let result = get_link_by_route(
            State(state),
            Path((
                "users".to_string(),
                user_id,
                "cars-owned".to_string(),
                car_id,
            )),
        )
        .await;

        assert!(result.is_ok(), "get_link_by_route should succeed");
    }

    #[tokio::test]
    async fn test_get_link_by_route_not_found() {
        let state = create_test_state();
        let result = get_link_by_route(
            State(state),
            Path((
                "users".to_string(),
                Uuid::new_v4(),
                "cars-owned".to_string(),
                Uuid::new_v4(),
            )),
        )
        .await;

        assert!(result.is_err(), "should fail when link does not exist");
    }

    // ------------------------------------------------------------------
    // Handler: list_available_links
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_available_links_known_entity() {
        let state = create_test_state();
        let user_id = Uuid::new_v4();

        let result = list_available_links(
            State(state),
            Path(("users".to_string(), user_id)),
        )
        .await
        .expect("handler should succeed");

        let resp = result.0;
        assert_eq!(resp.entity_type, "user");
        assert_eq!(resp.entity_id, user_id);
        // user has "cars-owned" forward route
        assert!(
            !resp.available_routes.is_empty(),
            "user should have available routes"
        );
    }

    #[tokio::test]
    async fn test_list_available_links_car_has_reverse_routes() {
        let state = create_test_state();
        let car_id = Uuid::new_v4();

        let result = list_available_links(
            State(state),
            Path(("cars".to_string(), car_id)),
        )
        .await
        .expect("handler should succeed");

        let resp = result.0;
        assert_eq!(resp.entity_type, "car");
        assert!(
            !resp.available_routes.is_empty(),
            "car should have reverse routes"
        );
        // Should contain "users-owners" reverse route
        let route_names: Vec<&str> = resp
            .available_routes
            .iter()
            .map(|r| r.path.as_str())
            .collect();
        let has_owners = route_names
            .iter()
            .any(|p| p.contains("users-owners"));
        assert!(has_owners, "car should have users-owners route");
    }

    // ======================================================================
    // Phase 3: Nested path handler tests
    // ======================================================================

    #[tokio::test]
    async fn test_handle_nested_path_get_too_few_segments() {
        let state = create_chain_test_state();
        // Only 3 segments: orders/{id}/invoices — less than 5 segments
        let result = handle_nested_path_get(
            State(state),
            Path("orders/abc/invoices".to_string()),
            Query(crate::core::query::QueryParams::default()),
        )
        .await;

        assert!(result.is_err(), "should fail with fewer than 5 segments");
    }

    #[tokio::test]
    async fn test_handle_nested_path_get_list_returns_paginated() {
        let state = create_chain_test_state();

        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();
        let payment_id = Uuid::new_v4();

        // Create the chain: order -> invoice -> payment
        let link1 =
            crate::core::link::LinkEntity::new("billing", order_id, invoice_id, None);
        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");

        let link2 =
            crate::core::link::LinkEntity::new("payment", invoice_id, payment_id, None);
        state
            .link_service
            .create(link2)
            .await
            .expect("create should succeed");

        // GET /orders/{order_id}/invoices/{invoice_id}/payments (5 segments -> list)
        let path = format!(
            "orders/{}/invoices/{}/payments",
            order_id, invoice_id
        );
        let result = handle_nested_path_get(
            State(state),
            Path(path),
            Query(crate::core::query::QueryParams::default()),
        )
        .await
        .expect("handler should succeed");

        let json = result.0;
        assert!(
            json.get("data").is_some(),
            "response should contain 'data' field"
        );
        assert!(
            json.get("pagination").is_some(),
            "response should contain 'pagination' field"
        );
        let data = json["data"].as_array().expect("data should be an array");
        assert_eq!(data.len(), 1, "should find one payment link");
    }

    #[tokio::test]
    async fn test_handle_nested_path_get_specific_item() {
        let state = create_chain_test_state();

        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();
        let payment_id = Uuid::new_v4();

        let link1 =
            crate::core::link::LinkEntity::new("billing", order_id, invoice_id, None);
        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");

        let link2 =
            crate::core::link::LinkEntity::new("payment", invoice_id, payment_id, None);
        state
            .link_service
            .create(link2)
            .await
            .expect("create should succeed");

        // GET /orders/{order_id}/invoices/{invoice_id}/payments/{payment_id} (6 segments -> item)
        let path = format!(
            "orders/{}/invoices/{}/payments/{}",
            order_id, invoice_id, payment_id
        );
        let result = handle_nested_path_get(
            State(state),
            Path(path),
            Query(crate::core::query::QueryParams::default()),
        )
        .await
        .expect("handler should succeed");

        let json = result.0;
        assert!(
            json.get("link").is_some(),
            "response should contain 'link' field for specific item"
        );
    }

    #[tokio::test]
    async fn test_handle_nested_path_get_broken_chain() {
        let state = create_chain_test_state();

        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        // Only create order->invoice link, but NOT invoice->payment
        let link1 =
            crate::core::link::LinkEntity::new("billing", order_id, invoice_id, None);
        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");

        // Try to get a specific payment through the chain — chain validation should fail
        // because order->invoice exists but invoice->payment doesn't for this specific item
        let fake_payment_id = Uuid::new_v4();
        let path = format!(
            "orders/{}/invoices/{}/payments/{}",
            order_id, invoice_id, fake_payment_id
        );
        let result = handle_nested_path_get(
            State(state),
            Path(path),
            Query(crate::core::query::QueryParams::default()),
        )
        .await;

        assert!(
            result.is_err(),
            "should fail when link chain is broken (no payment link)"
        );
    }

    #[tokio::test]
    async fn test_handle_nested_path_get_invalid_chain_first_link() {
        let state = create_chain_test_state();

        let order_id = Uuid::new_v4();
        let wrong_invoice_id = Uuid::new_v4();
        let payment_id = Uuid::new_v4();

        // Create link from DIFFERENT order, not from order_id
        let other_order_id = Uuid::new_v4();
        let link1 = crate::core::link::LinkEntity::new(
            "billing",
            other_order_id,
            wrong_invoice_id,
            None,
        );
        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");

        let link2 = crate::core::link::LinkEntity::new(
            "payment",
            wrong_invoice_id,
            payment_id,
            None,
        );
        state
            .link_service
            .create(link2)
            .await
            .expect("create should succeed");

        // Try to traverse: orders/{order_id}/invoices/{wrong_invoice_id}/payments
        // The first link (order_id -> wrong_invoice_id) does not exist
        let path = format!(
            "orders/{}/invoices/{}/payments",
            order_id, wrong_invoice_id
        );
        let result = handle_nested_path_get(
            State(state),
            Path(path),
            Query(crate::core::query::QueryParams::default()),
        )
        .await;

        assert!(
            result.is_err(),
            "should fail when first link in chain does not exist"
        );
    }

    // ------------------------------------------------------------------
    // Handler: handle_nested_path_post
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_handle_nested_path_post_too_few_segments() {
        let state = create_chain_test_state();
        let result = handle_nested_path_post(
            State(state),
            Path("orders/abc/invoices".to_string()),
            Json(CreateLinkedEntityRequest {
                entity: serde_json::json!({}),
                metadata: None,
            }),
        )
        .await;

        assert!(result.is_err(), "should fail with fewer than 5 segments");
    }

    #[tokio::test]
    async fn test_handle_nested_path_post_success() {
        let mut state = create_chain_test_state();

        // Register mock creator for payment
        let mut creators: HashMap<String, Arc<dyn crate::core::EntityCreator>> = HashMap::new();
        creators.insert("payment".to_string(), Arc::new(MockEntityCreator));
        state.entity_creators = Arc::new(creators);

        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        // Create the prerequisite chain
        let link1 =
            crate::core::link::LinkEntity::new("billing", order_id, invoice_id, None);
        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");

        // POST /orders/{order_id}/invoices/{invoice_id}/payments
        let path = format!("orders/{}/invoices/{}/payments", order_id, invoice_id);
        let result = handle_nested_path_post(
            State(state.clone()),
            Path(path),
            Json(CreateLinkedEntityRequest {
                entity: serde_json::json!({ "amount": 100.0 }),
                metadata: None,
            }),
        )
        .await;

        assert!(result.is_ok(), "handle_nested_path_post should succeed");
        let response = result.expect("should be ok");
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_handle_nested_path_post_no_creator() {
        let state = create_chain_test_state();

        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        let link1 =
            crate::core::link::LinkEntity::new("billing", order_id, invoice_id, None);
        state
            .link_service
            .create(link1)
            .await
            .expect("create should succeed");

        let path = format!("orders/{}/invoices/{}/payments", order_id, invoice_id);
        let result = handle_nested_path_post(
            State(state),
            Path(path),
            Json(CreateLinkedEntityRequest {
                entity: serde_json::json!({}),
                metadata: None,
            }),
        )
        .await;

        assert!(
            result.is_err(),
            "should fail when no entity creator is registered for target type"
        );
    }

    // ------------------------------------------------------------------
    // EnrichedLink serialization
    // ------------------------------------------------------------------

    #[test]
    fn test_enriched_link_skips_none_source_and_target() {
        let link = make_enriched_link("owner", "active", None, None, None);
        let json = serde_json::to_value(&link).expect("serialization should succeed");
        assert!(
            json.get("source").is_none(),
            "None source should be skipped in serialization"
        );
        assert!(
            json.get("target").is_none(),
            "None target should be skipped in serialization"
        );
        assert!(
            json.get("metadata").is_none(),
            "None metadata should be skipped in serialization"
        );
    }

    #[test]
    fn test_enriched_link_includes_present_fields() {
        let link = make_enriched_link(
            "owner",
            "active",
            Some(serde_json::json!({ "name": "Car" })),
            Some(serde_json::json!({ "name": "User" })),
            Some(serde_json::json!({ "priority": 1 })),
        );
        let json = serde_json::to_value(&link).expect("serialization should succeed");
        assert!(json.get("source").is_some());
        assert!(json.get("target").is_some());
        assert!(json.get("metadata").is_some());
        assert_eq!(json["type"], "link");
    }

    // ------------------------------------------------------------------
    // AppState with event bus integration
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_link_emits_event() {
        let bus = Arc::new(EventBus::new(16));
        let mut state = create_test_state();
        state.event_bus = Some(bus.clone());

        let mut rx = bus.subscribe();

        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let _result = create_link(
            State(state),
            Path((
                "users".to_string(),
                user_id,
                "cars-owned".to_string(),
                car_id,
            )),
            Json(CreateLinkRequest { metadata: None }),
        )
        .await
        .expect("create should succeed");

        let envelope = rx.try_recv().expect("should receive link created event");
        match envelope.event {
            FrameworkEvent::Link(LinkEvent::Created {
                link_type,
                source_id,
                target_id,
                ..
            }) => {
                assert_eq!(link_type, "owner");
                assert_eq!(source_id, user_id);
                assert_eq!(target_id, car_id);
            }
            other => panic!("expected Link::Created event, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_delete_link_emits_event() {
        let bus = Arc::new(EventBus::new(16));
        let mut state = create_test_state();
        state.event_bus = Some(bus.clone());

        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();
        let link = crate::core::link::LinkEntity::new("owner", user_id, car_id, None);
        state
            .link_service
            .create(link)
            .await
            .expect("create should succeed");

        let mut rx = bus.subscribe();

        delete_link(
            State(state),
            Path((
                "users".to_string(),
                user_id,
                "cars-owned".to_string(),
                car_id,
            )),
        )
        .await
        .expect("delete should succeed");

        let envelope = rx.try_recv().expect("should receive link deleted event");
        match envelope.event {
            FrameworkEvent::Link(LinkEvent::Deleted {
                link_type,
                source_id,
                target_id,
                ..
            }) => {
                assert_eq!(link_type, "owner");
                assert_eq!(source_id, user_id);
                assert_eq!(target_id, car_id);
            }
            other => panic!("expected Link::Deleted event, got: {:?}", other),
        }
    }
}
