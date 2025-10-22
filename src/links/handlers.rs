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
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::config::LinksConfig;
use crate::core::extractors::{
    extract_tenant_id, DirectLinkExtractor, ExtractorError, LinkExtractor,
};
use crate::core::{EntityReference, Link, LinkService};
use crate::links::registry::{LinkDirection, LinkRouteRegistry};

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub link_service: Arc<dyn LinkService>,
    pub config: Arc<LinksConfig>,
    pub registry: Arc<LinkRouteRegistry>,
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

/// Request body for creating a link
#[derive(Debug, Deserialize)]
pub struct CreateLinkRequest {
    pub metadata: Option<serde_json::Value>,
}

/// List links using named routes (forward or reverse)
///
/// GET /{entity_type}/{entity_id}/{route_name}
///
/// Examples:
/// - GET /users/{id}/cars-owned  → Forward navigation
/// - GET /cars/{id}/users-owners → Reverse navigation
pub async fn list_links(
    State(state): State<AppState>,
    Path((entity_type_plural, entity_id, route_name)): Path<(String, Uuid, String)>,
    headers: HeaderMap,
) -> Result<Json<ListLinksResponse>, ExtractorError> {
    let tenant_id = extract_tenant_id(&headers)?;

    let extractor = LinkExtractor::from_path_and_registry(
        (entity_type_plural, entity_id, route_name),
        &state.registry,
        &state.config,
        tenant_id,
    )?;

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

    Ok(Json(ListLinksResponse {
        count: links.len(),
        links,
        link_type: extractor.link_definition.link_type,
        direction: format!("{:?}", extractor.direction),
        description: extractor.link_definition.description,
    }))
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
                },
                EntityConfig {
                    singular: "car".to_string(),
                    plural: "cars".to_string(),
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
