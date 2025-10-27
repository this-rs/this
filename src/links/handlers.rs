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
        }
    }

    #[test]
    fn test_state_creation() {
        let state = create_test_state();
        assert_eq!(state.config.entities.len(), 2);
        assert_eq!(state.config.links.len(), 1);
    }
}
