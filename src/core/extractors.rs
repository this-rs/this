//! Axum extractors for entities and links
//!
//! This module provides HTTP extractors that automatically:
//! - Deserialize and validate entities from request bodies
//! - Extract tenant IDs from headers
//! - Parse link routes and resolve definitions

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use uuid::Uuid;

use crate::config::LinksConfig;
use crate::core::{EntityReference, LinkDefinition};
use crate::links::registry::{LinkDirection, LinkRouteRegistry};

/// Extract tenant ID from request headers
///
/// Expected header: `X-Tenant-ID: <uuid>`
pub fn extract_tenant_id(headers: &axum::http::HeaderMap) -> Result<Uuid, ExtractorError> {
    let tenant_id_str = headers
        .get("X-Tenant-ID")
        .ok_or(ExtractorError::MissingTenantId)?
        .to_str()
        .map_err(|_| ExtractorError::InvalidTenantId)?;

    Uuid::parse_str(tenant_id_str).map_err(|_| ExtractorError::InvalidTenantId)
}

/// Errors that can occur during extraction
#[derive(Debug, Clone)]
pub enum ExtractorError {
    MissingTenantId,
    InvalidTenantId,
    InvalidPath,
    InvalidEntityId,
    RouteNotFound(String),
    LinkNotFound,
    JsonError(String),
}

impl std::fmt::Display for ExtractorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractorError::MissingTenantId => write!(f, "Missing X-Tenant-ID header"),
            ExtractorError::InvalidTenantId => write!(f, "Invalid tenant ID format"),
            ExtractorError::InvalidPath => write!(f, "Invalid path format"),
            ExtractorError::InvalidEntityId => write!(f, "Invalid entity ID format"),
            ExtractorError::RouteNotFound(route) => write!(f, "Route not found: {}", route),
            ExtractorError::LinkNotFound => write!(f, "Link not found"),
            ExtractorError::JsonError(msg) => write!(f, "JSON error: {}", msg),
        }
    }
}

impl std::error::Error for ExtractorError {}

impl IntoResponse for ExtractorError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ExtractorError::MissingTenantId => (StatusCode::BAD_REQUEST, self.to_string()),
            ExtractorError::InvalidTenantId => (StatusCode::BAD_REQUEST, self.to_string()),
            ExtractorError::InvalidPath => (StatusCode::BAD_REQUEST, self.to_string()),
            ExtractorError::InvalidEntityId => (StatusCode::BAD_REQUEST, self.to_string()),
            ExtractorError::RouteNotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            ExtractorError::LinkNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            ExtractorError::JsonError(_) => (StatusCode::BAD_REQUEST, self.to_string()),
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Extractor for link information from path
///
/// Automatically parses the path and resolves link definitions.
/// Supports both forward and reverse navigation.
#[derive(Debug, Clone)]
pub struct LinkExtractor {
    pub tenant_id: Uuid,
    pub entity_id: Uuid,
    pub entity_type: String,
    pub link_definition: LinkDefinition,
    pub direction: LinkDirection,
}

impl LinkExtractor {
    /// Parse a link route path
    ///
    /// Expected format: `/{entity_type}/{entity_id}/{route_name}`
    /// Example: `/users/123.../cars-owned`
    pub fn from_path_and_registry(
        path_parts: (String, Uuid, String),
        registry: &LinkRouteRegistry,
        config: &LinksConfig,
        tenant_id: Uuid,
    ) -> Result<Self, ExtractorError> {
        let (entity_type_plural, entity_id, route_name) = path_parts;

        // Convert plural to singular
        let entity_type = config
            .entities
            .iter()
            .find(|e| e.plural == entity_type_plural)
            .map(|e| e.singular.clone())
            .unwrap_or(entity_type_plural);

        // Resolve the route
        let (link_definition, direction) = registry
            .resolve_route(&entity_type, &route_name)
            .map_err(|_| ExtractorError::RouteNotFound(route_name.clone()))?;

        Ok(Self {
            tenant_id,
            entity_id,
            entity_type,
            link_definition,
            direction,
        })
    }
}

/// Extractor for direct link creation/deletion/update
///
/// NEW Format: `/{source_type}/{source_id}/{route_name}/{target_id}`
/// Example: `/users/123.../cars-owned/456...`
///
/// This uses the route_name (e.g., "cars-owned") instead of link_type (e.g., "owner")
/// to provide more semantic and RESTful URLs.
#[derive(Debug, Clone)]
pub struct DirectLinkExtractor {
    pub tenant_id: Uuid,
    pub source: EntityReference,
    pub target: EntityReference,
    pub link_definition: LinkDefinition,
    pub direction: LinkDirection,
}

impl DirectLinkExtractor {
    /// Parse a direct link path using route_name
    ///
    /// NEW: path_parts = (source_type_plural, source_id, route_name, target_id)
    ///
    /// The route_name is resolved to a link definition using the LinkRouteRegistry,
    /// which handles both forward and reverse navigation automatically.
    pub fn from_path(
        path_parts: (String, Uuid, String, Uuid),
        registry: &LinkRouteRegistry,
        config: &LinksConfig,
        tenant_id: Uuid,
    ) -> Result<Self, ExtractorError> {
        let (source_type_plural, source_id, route_name, target_id) = path_parts;

        // Convert plural to singular
        let source_type = config
            .entities
            .iter()
            .find(|e| e.plural == source_type_plural)
            .map(|e| e.singular.clone())
            .unwrap_or(source_type_plural);

        // Resolve the route to get link definition and direction
        let (link_definition, direction) = registry
            .resolve_route(&source_type, &route_name)
            .map_err(|_| ExtractorError::RouteNotFound(route_name.clone()))?;

        // Determine target type based on direction
        let target_type = match direction {
            LinkDirection::Forward => link_definition.target_type.clone(),
            LinkDirection::Reverse => link_definition.source_type.clone(),
        };

        let source = EntityReference::new(source_id, source_type);
        let target = EntityReference::new(target_id, target_type);

        Ok(Self {
            tenant_id,
            source,
            target,
            link_definition,
            direction,
        })
    }
}
