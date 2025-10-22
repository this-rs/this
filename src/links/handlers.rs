//! HTTP handlers for link operations
//!
//! This module provides generic handlers that work with any entity types.
//! All handlers are completely entity-agnostic.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::config::LinksConfig;
use crate::core::extractors::{
    extract_tenant_id, DirectLinkExtractor, ExtractorError, LinkExtractor,
};
use crate::core::{EntityFetcher, EntityReference, Link, LinkDefinition, LinkService};
use crate::links::registry::{LinkDirection, LinkRouteRegistry};

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub link_service: Arc<dyn LinkService>,
    pub config: Arc<LinksConfig>,
    pub registry: Arc<LinkRouteRegistry>,
    /// Entity fetchers for enriching links with full entity data
    pub entity_fetchers: Arc<HashMap<String, Arc<dyn EntityFetcher>>>,
}

impl AppState {
    /// Get the authorization policy for a link operation
    ///
    /// Returns the link-specific auth policy if defined, otherwise returns None
    /// to indicate that entity-level permissions should be used.
    ///
    /// # Arguments
    /// * `link_definition` - The link definition to check
    /// * `operation` - The operation type: "list", "create", or "delete"
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
    pub links: Vec<Link>,
    pub count: usize,
    pub link_type: String,
    pub direction: String,
    pub description: Option<String>,
}

/// Link with full entity data instead of just references
///
/// This enriched version includes the complete source and target entities
/// as JSON, avoiding the need for additional API calls.
///
/// Depending on the context:
/// - From source route (e.g., /orders/{id}/invoices): only `target` is populated
/// - From target route (reverse): only `source` is populated
/// - Direct link access (e.g., /links/{id}): both `source` and `target` are populated
#[derive(Debug, Serialize)]
pub struct EnrichedLink {
    /// Unique identifier for this link
    pub id: Uuid,
    
    /// Tenant ID for multi-tenant isolation
    pub tenant_id: Uuid,
    
    /// The type of relationship (e.g., "has_invoice", "payment")
    pub link_type: String,
    
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

/// Request body for creating a link
#[derive(Debug, Deserialize)]
pub struct CreateLinkRequest {
    pub metadata: Option<serde_json::Value>,
}

/// Context for link enrichment
///
/// Determines which entities should be fetched and included in the response
#[derive(Debug, Clone, Copy)]
enum EnrichmentContext {
    /// Query from source entity (e.g., /orders/{id}/invoices)
    /// Only target entities are included
    FromSource,
    
    /// Query from target entity (reverse navigation)
    /// Only source entities are included
    FromTarget,
    
    /// Direct link access (e.g., /links/{id})
    /// Both source and target entities are included
    DirectLink,
}

/// List links using named routes (forward or reverse)
///
/// GET /{entity_type}/{entity_id}/{route_name}
///
/// Examples:
/// - GET /users/{id}/cars-owned  → Forward navigation
/// - GET /cars/{id}/users-owners → Reverse navigation
///
/// This endpoint automatically enriches links with full entity data.
pub async fn list_links(
    State(state): State<AppState>,
    Path((entity_type_plural, entity_id, route_name)): Path<(String, Uuid, String)>,
    headers: HeaderMap,
) -> Result<Json<EnrichedListLinksResponse>, ExtractorError> {
    let tenant_id = extract_tenant_id(&headers)?;

    let extractor = LinkExtractor::from_path_and_registry(
        (entity_type_plural, entity_id, route_name),
        &state.registry,
        &state.config,
        tenant_id,
    )?;

    // TODO: Check authorization - use link-specific auth if available, fallback to entity auth
    // if let Some(link_auth) = &extractor.link_definition.auth {
    //     check_auth_policy(&headers, &link_auth.list, &extractor)?;
    // } else {
    //     // Fallback to entity-level link permissions
    //     check_entity_link_auth(&headers, &extractor.entity_type, "list_links")?;
    // }

    // Query links based on direction
    let links = match extractor.direction {
        LinkDirection::Forward => {
            let source = EntityReference::new(extractor.entity_id, extractor.entity_type);
            state
                .link_service
                .find_by_source(
                    &tenant_id,
                    &extractor.entity_id,
                    &source.entity_type,
                    Some(&extractor.link_definition.link_type),
                    Some(&extractor.link_definition.target_type),
                )
                .await
                .map_err(|e| ExtractorError::JsonError(e.to_string()))?
        }
        LinkDirection::Reverse => {
            let target = EntityReference::new(extractor.entity_id, extractor.entity_type);
            state
                .link_service
                .find_by_target(
                    &tenant_id,
                    &extractor.entity_id,
                    &target.entity_type,
                    Some(&extractor.link_definition.link_type),
                    Some(&extractor.link_definition.source_type),
                )
                .await
                .map_err(|e| ExtractorError::JsonError(e.to_string()))?
        }
    };

    // Determine enrichment context based on direction
    let context = match extractor.direction {
        LinkDirection::Forward => EnrichmentContext::FromSource,
        LinkDirection::Reverse => EnrichmentContext::FromTarget,
    };

    // Enrich links with full entity data (only the relevant side)
    let enriched_links = enrich_links_with_entities(&state, links, &tenant_id, context).await?;

    Ok(Json(EnrichedListLinksResponse {
        count: enriched_links.len(),
        links: enriched_links,
        link_type: extractor.link_definition.link_type,
        direction: format!("{:?}", extractor.direction),
        description: extractor.link_definition.description,
    }))
}

/// Helper function to enrich links with full entity data
///
/// Depending on the context, only the necessary entities are fetched:
/// - FromSource: only target entities
/// - FromTarget: only source entities  
/// - DirectLink: both source and target entities
async fn enrich_links_with_entities(
    state: &AppState,
    links: Vec<Link>,
    tenant_id: &Uuid,
    context: EnrichmentContext,
) -> Result<Vec<EnrichedLink>, ExtractorError> {
    let mut enriched = Vec::new();

    for link in links {
        // Fetch source entity only if needed
        let source_entity = match context {
            EnrichmentContext::FromSource => None, // Already known from URL
            EnrichmentContext::FromTarget | EnrichmentContext::DirectLink => {
                Some(
                    fetch_entity_by_type(
                        state,
                        tenant_id,
                        &link.source.entity_type,
                        &link.source.id,
                    )
                    .await?,
                )
            }
        };

        // Fetch target entity only if needed
        let target_entity = match context {
            EnrichmentContext::FromTarget => None, // Already known from URL
            EnrichmentContext::FromSource | EnrichmentContext::DirectLink => {
                Some(
                    fetch_entity_by_type(
                        state,
                        tenant_id,
                        &link.target.entity_type,
                        &link.target.id,
                    )
                    .await?,
                )
            }
        };

        enriched.push(EnrichedLink {
            id: link.id,
            tenant_id: link.tenant_id,
            link_type: link.link_type,
            source: source_entity,
            target: target_entity,
            metadata: link.metadata,
            created_at: link.created_at,
            updated_at: link.updated_at,
        });
    }

    Ok(enriched)
}

/// Fetch an entity dynamically by type
async fn fetch_entity_by_type(
    state: &AppState,
    tenant_id: &Uuid,
    entity_type: &str,
    entity_id: &Uuid,
) -> Result<serde_json::Value, ExtractorError> {
    // Look up the fetcher for this entity type
    let fetcher = state
        .entity_fetchers
        .get(entity_type)
        .ok_or_else(|| {
            ExtractorError::JsonError(format!(
                "No entity fetcher registered for type: {}",
                entity_type
            ))
        })?;

    // Fetch the entity as JSON
    fetcher
        .fetch_as_json(tenant_id, entity_id)
        .await
        .map_err(|e| ExtractorError::JsonError(format!("Failed to fetch entity: {}", e)))
}

/// Get a specific link by ID
///
/// GET /links/{link_id}
///
/// Example:
/// - GET /links/abc-123-def-456
///
/// This endpoint returns the link enriched with BOTH source and target entities,
/// since the caller doesn't know which entities are involved.
pub async fn get_link(
    State(state): State<AppState>,
    Path(link_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ExtractorError> {
    let tenant_id = extract_tenant_id(&headers)?;

    // Get the link
    let link = state
        .link_service
        .get(&tenant_id, &link_id)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?
        .ok_or_else(|| ExtractorError::LinkNotFound)?;

    // Find the link definition to check permissions
    let _link_def = state
        .config
        .find_link_definition(&link.link_type, &link.source.entity_type, &link.target.entity_type);

    // TODO: Check authorization for getting a link
    // if let Some(def) = link_def {
    //     if let Some(link_auth) = &def.auth {
    //         check_auth_policy(&headers, &link_auth.get, &state)?;
    //     }
    // }

    // Enrich with both source and target entities (DirectLink context)
    let enriched_links = enrich_links_with_entities(
        &state,
        vec![link],
        &tenant_id,
        EnrichmentContext::DirectLink,
    )
    .await?;

    let enriched_link = enriched_links
        .into_iter()
        .next()
        .ok_or_else(|| ExtractorError::LinkNotFound)?;

    Ok(Json(enriched_link).into_response())
}

/// Create a link using direct path
///
/// POST /{source_type}/{source_id}/{link_type}/{target_type}/{target_id}
///
/// Example:
/// - POST /users/123.../owner/cars/456...
pub async fn create_link(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, link_type, target_type_plural, target_id)): Path<(
        String,
        Uuid,
        String,
        String,
        Uuid,
    )>,
    headers: HeaderMap,
    Json(payload): Json<CreateLinkRequest>,
) -> Result<Response, ExtractorError> {
    let tenant_id = extract_tenant_id(&headers)?;

    let extractor = DirectLinkExtractor::from_path(
        (
            source_type_plural,
            source_id,
            link_type.clone(),
            target_type_plural,
            target_id,
        ),
        &state.config,
        tenant_id,
    )?;

    // TODO: Check authorization for link creation
    // if let Some(link_def) = &extractor.link_definition {
    //     if let Some(link_auth) = &link_def.auth {
    //         check_auth_policy(&headers, &link_auth.create, &extractor)?;
    //     } else {
    //         // Fallback to entity-level link permissions
    //         check_entity_link_auth(&headers, &extractor.source.entity_type, "create_link")?;
    //     }
    // }

    // Validate the link definition exists
    if extractor.link_definition.is_none() {
        return Err(ExtractorError::RouteNotFound(format!(
            "No link definition found for {} -> {} via {}",
            extractor.source.entity_type, extractor.target.entity_type, link_type
        )));
    }

    // Create the link
    let link = state
        .link_service
        .create(
            &tenant_id,
            &link_type,
            extractor.source,
            extractor.target,
            payload.metadata,
        )
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(link)).into_response())
}

/// Update a link's metadata using direct path
///
/// PUT/PATCH /{source_type}/{source_id}/{link_type}/{target_type}/{target_id}
///
/// Example:
/// - PUT /users/123.../worker/companies/456...
/// - Body: { "metadata": { "role": "Senior Developer", "promotion_date": "2024-06-01" } }
pub async fn update_link(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, link_type, target_type_plural, target_id)): Path<(
        String,
        Uuid,
        String,
        String,
        Uuid,
    )>,
    headers: HeaderMap,
    Json(payload): Json<CreateLinkRequest>,
) -> Result<Response, ExtractorError> {
    let tenant_id = extract_tenant_id(&headers)?;

    let extractor = DirectLinkExtractor::from_path(
        (
            source_type_plural,
            source_id,
            link_type.clone(),
            target_type_plural,
            target_id,
        ),
        &state.config,
        tenant_id,
    )?;

    // TODO: Check authorization for link update
    // if let Some(link_def) = &extractor.link_definition {
    //     if let Some(link_auth) = &link_def.auth {
    //         check_auth_policy(&headers, &link_auth.update, &extractor)?;
    //     } else {
    //         // Fallback to entity-level link permissions
    //         check_entity_link_auth(&headers, &extractor.source.entity_type, "update_link")?;
    //     }
    // }

    // Find the existing link
    let existing_links = state
        .link_service
        .find_by_source(
            &tenant_id,
            &extractor.source.id,
            &extractor.source.entity_type,
            Some(&link_type),
            Some(&extractor.target.entity_type),
        )
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    let existing_link = existing_links
        .into_iter()
        .find(|link| link.target.id == extractor.target.id)
        .ok_or_else(|| ExtractorError::RouteNotFound("Link not found".to_string()))?;

    // Update the link
    let updated_link = state
        .link_service
        .update(&tenant_id, &existing_link.id, payload.metadata)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    Ok(Json(updated_link).into_response())
}

/// Delete a link using direct path
///
/// DELETE /{source_type}/{source_id}/{link_type}/{target_type}/{target_id}
///
/// Example:
/// - DELETE /users/123.../owner/cars/456...
pub async fn delete_link(
    State(state): State<AppState>,
    Path((source_type_plural, source_id, link_type, target_type_plural, target_id)): Path<(
        String,
        Uuid,
        String,
        String,
        Uuid,
    )>,
    headers: HeaderMap,
) -> Result<Response, ExtractorError> {
    let tenant_id = extract_tenant_id(&headers)?;

    let extractor = DirectLinkExtractor::from_path(
        (
            source_type_plural,
            source_id,
            link_type.clone(),
            target_type_plural,
            target_id,
        ),
        &state.config,
        tenant_id,
    )?;

    // TODO: Check authorization for link deletion
    // if let Some(link_def) = &extractor.link_definition {
    //     if let Some(link_auth) = &link_def.auth {
    //         check_auth_policy(&headers, &link_auth.delete, &extractor)?;
    //     } else {
    //         // Fallback to entity-level link permissions
    //         check_entity_link_auth(&headers, &extractor.source.entity_type, "delete_link")?;
    //     }
    // }

    // Delete the link
    state
        .link_service
        .delete(&tenant_id, &extractor.source.id)
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;

    Ok((StatusCode::NO_CONTENT, ()).into_response())
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
///
/// Example:
/// - GET /users/123.../links
pub async fn list_available_links(
    State(state): State<AppState>,
    Path((entity_type_plural, entity_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<IntrospectionResponse>, ExtractorError> {
    let _tenant_id = extract_tenant_id(&headers)?;

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
    use crate::links::service::InMemoryLinkService;

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
        }
    }

    #[test]
    fn test_state_creation() {
        let state = create_test_state();
        assert_eq!(state.config.entities.len(), 2);
        assert_eq!(state.config.links.len(), 1);
    }
}
