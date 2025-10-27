//! Axum extractors for entities and links
//!
//! This module provides HTTP extractors that automatically:
//! - Deserialize and validate entities from request bodies
//! - Parse link routes and resolve definitions

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::config::LinksConfig;
use crate::core::LinkDefinition;
use crate::links::registry::{LinkDirection, LinkRouteRegistry};

/// Errors that can occur during extraction
#[derive(Debug, Clone)]
pub enum ExtractorError {
    InvalidPath,
    InvalidEntityId,
    RouteNotFound(String),
    LinkNotFound,
    JsonError(String),
}

impl std::fmt::Display for ExtractorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
            entity_id,
            entity_type,
            link_definition,
            direction,
        })
    }
}

/// Extractor for direct link creation/deletion/update
///
/// Format: `/{source_type}/{source_id}/{route_name}/{target_id}`
/// Example: `/users/123.../cars-owned/456...`
///
/// This uses the route_name (e.g., "cars-owned") instead of link_type (e.g., "owner")
/// to provide more semantic and RESTful URLs.
#[derive(Debug, Clone)]
pub struct DirectLinkExtractor {
    pub source_id: Uuid,
    pub source_type: String,
    pub target_id: Uuid,
    pub target_type: String,
    pub link_definition: LinkDefinition,
    pub direction: LinkDirection,
}

impl DirectLinkExtractor {
    /// Parse a direct link path using route_name
    ///
    /// path_parts = (source_type_plural, source_id, route_name, target_id)
    ///
    /// The route_name is resolved to a link definition using the LinkRouteRegistry,
    /// which handles both forward and reverse navigation automatically.
    pub fn from_path(
        path_parts: (String, Uuid, String, Uuid),
        registry: &LinkRouteRegistry,
        config: &LinksConfig,
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

        Ok(Self {
            source_id,
            source_type,
            target_id,
            target_type,
            link_definition,
            direction,
        })
    }
}

/// Segment d'une chaîne de liens imbriqués
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkPathSegment {
    /// Type d'entité (singulier)
    pub entity_type: String,
    /// ID de l'entité
    pub entity_id: Uuid,
    /// Nom de la route (si présent)
    pub route_name: Option<String>,
    /// Définition du lien (si présent)
    pub link_definition: Option<LinkDefinition>,
    /// Direction du lien (Forward ou Reverse)
    #[serde(skip_serializing)]
    pub link_direction: Option<LinkDirection>,
}

/// Extractor pour chemins imbriqués de profondeur illimitée
///
/// Parse dynamiquement des chemins comme:
/// - /users/123/invoices/456/orders
/// - /users/123/invoices/456/orders/789/payments/101
#[derive(Debug, Clone)]
pub struct RecursiveLinkExtractor {
    pub chain: Vec<LinkPathSegment>,
    /// True si le chemin se termine par une route (liste)
    /// False si le chemin se termine par un ID (item spécifique)
    pub is_list: bool,
}

impl RecursiveLinkExtractor {
    /// Parse un chemin complet dynamiquement
    pub fn from_segments(
        segments: Vec<String>,
        registry: &LinkRouteRegistry,
        config: &LinksConfig,
    ) -> Result<Self, ExtractorError> {
        if segments.len() < 2 {
            return Err(ExtractorError::InvalidPath);
        }

        let mut chain = Vec::new();
        let mut i = 0;
        let mut current_entity_type: Option<String> = None;

        // Pattern attendu: type/id/route/id/route/id...
        // Premier segment: toujours un type d'entité
        while i < segments.len() {
            // 1. Type d'entité (soit depuis URL pour le 1er, soit depuis link_def pour les suivants)
            let entity_type_singular = if let Some(ref entity_type) = current_entity_type {
                // Type connu depuis la résolution précédente
                entity_type.clone()
            } else {
                // Premier segment: lire le type depuis l'URL
                let entity_type_plural = &segments[i];
                let singular = config
                    .entities
                    .iter()
                    .find(|e| e.plural == *entity_type_plural)
                    .map(|e| e.singular.clone())
                    .ok_or(ExtractorError::InvalidPath)?;
                i += 1;
                singular
            };

            // Reset pour la prochaine itération
            current_entity_type = None;

            // 2. ID de l'entité (peut ne pas exister si fin du chemin)
            let entity_id = if i < segments.len() {
                segments[i]
                    .parse::<Uuid>()
                    .map_err(|_| ExtractorError::InvalidEntityId)?
            } else {
                // Pas d'ID = liste finale
                chain.push(LinkPathSegment {
                    entity_type: entity_type_singular,
                    entity_id: Uuid::nil(),
                    route_name: None,
                    link_definition: None,
                    link_direction: None,
                });
                break;
            };
            i += 1;

            // 3. Nom de route (peut ne pas exister si fin du chemin)
            let route_name = if i < segments.len() {
                Some(segments[i].clone())
            } else {
                None
            };

            if route_name.is_some() {
                i += 1;
            }

            // Résoudre la définition du lien si on a une route
            let (link_def, link_dir) = if let Some(route_name) = &route_name {
                let (link_def, direction) = registry
                    .resolve_route(&entity_type_singular, route_name)
                    .map_err(|_| ExtractorError::RouteNotFound(route_name.clone()))?;

                // Préparer le type pour la prochaine itération
                // Pour Forward: on va vers target_type
                // Pour Reverse: on va vers source_type (car on remonte la chaîne)
                current_entity_type = Some(match direction {
                    crate::links::registry::LinkDirection::Forward => link_def.target_type.clone(),
                    crate::links::registry::LinkDirection::Reverse => link_def.source_type.clone(),
                });

                (Some(link_def), Some(direction))
            } else {
                (None, None)
            };

            chain.push(LinkPathSegment {
                entity_type: entity_type_singular,
                entity_id,
                route_name,
                link_definition: link_def,
                link_direction: link_dir,
            });
        }

        // Si current_entity_type est défini, cela signifie que le chemin se termine par une route
        // et qu'on doit ajouter un segment final pour l'entité cible (liste)
        if let Some(final_entity_type) = current_entity_type {
            chain.push(LinkPathSegment {
                entity_type: final_entity_type,
                entity_id: Uuid::nil(), // Pas d'ID spécifique = liste
                route_name: None,
                link_definition: None,
                link_direction: None,
            });
        }

        // Déterminer si c'est une liste ou un item spécifique
        // Format: type/id/route/id/route → 5 segments → liste
        // Format: type/id/route/id/route/id → 6 segments → item
        // Si impair ≥ 5: liste, si pair ≥ 6: item spécifique
        let is_list = (segments.len() % 2 == 1) && (segments.len() >= 5);

        Ok(Self { chain, is_list })
    }

    /// Obtenir l'ID final et le type pour la requête finale
    pub fn final_target(&self) -> (Uuid, String) {
        let last = self.chain.last().unwrap();
        (last.entity_id, last.entity_type.clone())
    }

    /// Obtenir la définition du dernier lien
    pub fn final_link_def(&self) -> Option<&LinkDefinition> {
        // Le dernier segment n'a pas de link_def, le pénultième oui
        if self.chain.len() >= 2 {
            self.chain
                .get(self.chain.len() - 2)
                .and_then(|s| s.link_definition.as_ref())
        } else {
            None
        }
    }

    /// Obtenir l'avant-dernier segment (celui qui a le lien)
    pub fn penultimate_segment(&self) -> Option<&LinkPathSegment> {
        if self.chain.len() >= 2 {
            self.chain.get(self.chain.len() - 2)
        } else {
            None
        }
    }
}
