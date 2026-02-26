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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::LinkDefinition;
    use crate::links::registry::LinkRouteRegistry;
    use std::sync::Arc;
    use uuid::Uuid;

    /// Build a minimal LinksConfig + LinkRouteRegistry for testing.
    /// Entities: user (users), order (orders), invoice (invoices)
    /// Links: user->order (ownership), order->invoice (billing)
    fn test_config_and_registry() -> (Arc<LinksConfig>, LinkRouteRegistry) {
        let config = Arc::new(LinksConfig {
            entities: vec![
                EntityConfig {
                    singular: "user".to_string(),
                    plural: "users".to_string(),
                    auth: EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "order".to_string(),
                    plural: "orders".to_string(),
                    auth: EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "invoice".to_string(),
                    plural: "invoices".to_string(),
                    auth: EntityAuthConfig::default(),
                },
            ],
            links: vec![
                LinkDefinition {
                    link_type: "ownership".to_string(),
                    source_type: "user".to_string(),
                    target_type: "order".to_string(),
                    forward_route_name: "orders-owned".to_string(),
                    reverse_route_name: "owner".to_string(),
                    description: None,
                    required_fields: None,
                    auth: None,
                },
                LinkDefinition {
                    link_type: "billing".to_string(),
                    source_type: "order".to_string(),
                    target_type: "invoice".to_string(),
                    forward_route_name: "invoices".to_string(),
                    reverse_route_name: "order".to_string(),
                    description: None,
                    required_fields: None,
                    auth: None,
                },
            ],
            validation_rules: None,
        });
        let registry = LinkRouteRegistry::new(config.clone());
        (config, registry)
    }

    // === ExtractorError Display + IntoResponse ===

    #[test]
    fn test_extractor_error_display_invalid_path() {
        let err = ExtractorError::InvalidPath;
        assert_eq!(err.to_string(), "Invalid path format");
    }

    #[test]
    fn test_extractor_error_display_invalid_entity_id() {
        let err = ExtractorError::InvalidEntityId;
        assert_eq!(err.to_string(), "Invalid entity ID format");
    }

    #[test]
    fn test_extractor_error_display_route_not_found() {
        let err = ExtractorError::RouteNotFound("my-route".to_string());
        assert_eq!(err.to_string(), "Route not found: my-route");
    }

    #[test]
    fn test_extractor_error_display_link_not_found() {
        let err = ExtractorError::LinkNotFound;
        assert_eq!(err.to_string(), "Link not found");
    }

    #[test]
    fn test_extractor_error_display_json_error() {
        let err = ExtractorError::JsonError("bad json".to_string());
        assert_eq!(err.to_string(), "JSON error: bad json");
    }

    #[test]
    fn test_extractor_error_into_response_invalid_path_400() {
        let err = ExtractorError::InvalidPath;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_extractor_error_into_response_invalid_entity_id_400() {
        let err = ExtractorError::InvalidEntityId;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_extractor_error_into_response_route_not_found_404() {
        let err = ExtractorError::RouteNotFound("test".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_extractor_error_into_response_link_not_found_404() {
        let err = ExtractorError::LinkNotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_extractor_error_into_response_json_error_400() {
        let err = ExtractorError::JsonError("oops".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // === LinkExtractor ===

    #[test]
    fn test_link_extractor_forward_route() {
        let (config, registry) = test_config_and_registry();
        let id = Uuid::new_v4();
        let result = LinkExtractor::from_path_and_registry(
            ("users".to_string(), id, "orders-owned".to_string()),
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        assert_eq!(ext.entity_type, "user");
        assert_eq!(ext.entity_id, id);
        assert_eq!(ext.link_definition.link_type, "ownership");
        assert!(matches!(ext.direction, LinkDirection::Forward));
    }

    #[test]
    fn test_link_extractor_reverse_route() {
        let (config, registry) = test_config_and_registry();
        let id = Uuid::new_v4();
        let result = LinkExtractor::from_path_and_registry(
            ("orders".to_string(), id, "owner".to_string()),
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        assert_eq!(ext.entity_type, "order");
        assert!(matches!(ext.direction, LinkDirection::Reverse));
    }

    #[test]
    fn test_link_extractor_route_not_found() {
        let (config, registry) = test_config_and_registry();
        let id = Uuid::new_v4();
        let result = LinkExtractor::from_path_and_registry(
            ("users".to_string(), id, "nonexistent".to_string()),
            &registry,
            &config,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExtractorError::RouteNotFound(_)
        ));
    }

    #[test]
    fn test_link_extractor_plural_to_singular_conversion() {
        let (config, registry) = test_config_and_registry();
        let id = Uuid::new_v4();
        let result = LinkExtractor::from_path_and_registry(
            ("users".to_string(), id, "orders-owned".to_string()),
            &registry,
            &config,
        );
        let ext = result.expect("should succeed");
        // "users" converted to "user"
        assert_eq!(ext.entity_type, "user");
    }

    #[test]
    fn test_link_extractor_unknown_plural_used_as_is() {
        let (config, registry) = test_config_and_registry();
        let id = Uuid::new_v4();
        // "widgets" not in config → used as-is as entity_type
        let result = LinkExtractor::from_path_and_registry(
            ("widgets".to_string(), id, "orders-owned".to_string()),
            &registry,
            &config,
        );
        // Route resolution will likely fail since "widgets" is not a known entity
        assert!(result.is_err());
    }

    // === DirectLinkExtractor ===

    #[test]
    fn test_direct_link_extractor_forward() {
        let (config, registry) = test_config_and_registry();
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let result = DirectLinkExtractor::from_path(
            (
                "users".to_string(),
                source_id,
                "orders-owned".to_string(),
                target_id,
            ),
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        assert_eq!(ext.source_type, "user");
        assert_eq!(ext.source_id, source_id);
        assert_eq!(ext.target_id, target_id);
        assert_eq!(ext.target_type, "order"); // Forward → target_type
        assert!(matches!(ext.direction, LinkDirection::Forward));
    }

    #[test]
    fn test_direct_link_extractor_reverse() {
        let (config, registry) = test_config_and_registry();
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let result = DirectLinkExtractor::from_path(
            (
                "orders".to_string(),
                source_id,
                "owner".to_string(),
                target_id,
            ),
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        assert_eq!(ext.source_type, "order");
        assert_eq!(ext.target_type, "user"); // Reverse → source_type
        assert!(matches!(ext.direction, LinkDirection::Reverse));
    }

    #[test]
    fn test_direct_link_extractor_route_not_found() {
        let (config, registry) = test_config_and_registry();
        let result = DirectLinkExtractor::from_path(
            (
                "users".to_string(),
                Uuid::new_v4(),
                "nope".to_string(),
                Uuid::new_v4(),
            ),
            &registry,
            &config,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExtractorError::RouteNotFound(_)
        ));
    }

    // === RecursiveLinkExtractor ===

    #[test]
    fn test_recursive_too_few_segments_error() {
        let (config, registry) = test_config_and_registry();
        let result =
            RecursiveLinkExtractor::from_segments(vec!["users".to_string()], &registry, &config);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExtractorError::InvalidPath));
    }

    #[test]
    fn test_recursive_entity_type_and_id() {
        let (config, registry) = test_config_and_registry();
        let id = Uuid::new_v4();
        let result = RecursiveLinkExtractor::from_segments(
            vec!["users".to_string(), id.to_string()],
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        assert_eq!(ext.chain.len(), 1);
        assert_eq!(ext.chain[0].entity_type, "user");
        assert_eq!(ext.chain[0].entity_id, id);
    }

    #[test]
    fn test_recursive_invalid_uuid_error() {
        let (config, registry) = test_config_and_registry();
        let result = RecursiveLinkExtractor::from_segments(
            vec!["users".to_string(), "not-a-uuid".to_string()],
            &registry,
            &config,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExtractorError::InvalidEntityId
        ));
    }

    #[test]
    fn test_recursive_unknown_entity_type_error() {
        let (config, registry) = test_config_and_registry();
        let result = RecursiveLinkExtractor::from_segments(
            vec!["widgets".to_string(), Uuid::new_v4().to_string()],
            &registry,
            &config,
        );
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExtractorError::InvalidPath));
    }

    #[test]
    fn test_recursive_entity_id_route_forward() {
        let (config, registry) = test_config_and_registry();
        let user_id = Uuid::new_v4();
        let result = RecursiveLinkExtractor::from_segments(
            vec![
                "users".to_string(),
                user_id.to_string(),
                "orders-owned".to_string(),
            ],
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        // Chain: user(user_id, route=orders-owned) → order(nil, list)
        assert_eq!(ext.chain.len(), 2);
        assert_eq!(ext.chain[0].entity_type, "user");
        assert_eq!(ext.chain[0].entity_id, user_id);
        assert_eq!(ext.chain[0].route_name.as_deref(), Some("orders-owned"));
        assert_eq!(
            ext.chain[0]
                .link_definition
                .as_ref()
                .expect("should have link_def")
                .link_type,
            "ownership"
        );
        assert_eq!(ext.chain[1].entity_type, "order");
        assert!(ext.chain[1].entity_id.is_nil()); // list segment
    }

    #[test]
    fn test_recursive_multi_level_chain() {
        let (config, registry) = test_config_and_registry();
        let user_id = Uuid::new_v4();
        let order_id = Uuid::new_v4();
        let result = RecursiveLinkExtractor::from_segments(
            vec![
                "users".to_string(),
                user_id.to_string(),
                "orders-owned".to_string(),
                order_id.to_string(),
                "invoices".to_string(),
            ],
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        // Chain: user(user_id) → order(order_id) → invoice(nil, list)
        assert_eq!(ext.chain.len(), 3);
        assert_eq!(ext.chain[0].entity_type, "user");
        assert_eq!(ext.chain[0].entity_id, user_id);
        assert_eq!(ext.chain[1].entity_type, "order");
        assert_eq!(ext.chain[1].entity_id, order_id);
        assert_eq!(ext.chain[2].entity_type, "invoice");
        assert!(ext.is_list); // 5 segments → list
    }

    #[test]
    fn test_recursive_multi_level_specific_item() {
        let (config, registry) = test_config_and_registry();
        let user_id = Uuid::new_v4();
        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();
        let result = RecursiveLinkExtractor::from_segments(
            vec![
                "users".to_string(),
                user_id.to_string(),
                "orders-owned".to_string(),
                order_id.to_string(),
                "invoices".to_string(),
                invoice_id.to_string(),
            ],
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        assert_eq!(ext.chain.len(), 3);
        assert_eq!(ext.chain[2].entity_id, invoice_id);
        assert!(!ext.is_list); // 6 segments → specific item
    }

    #[test]
    fn test_recursive_route_not_found_mid_chain() {
        let (config, registry) = test_config_and_registry();
        let result = RecursiveLinkExtractor::from_segments(
            vec![
                "users".to_string(),
                Uuid::new_v4().to_string(),
                "nonexistent-route".to_string(),
            ],
            &registry,
            &config,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExtractorError::RouteNotFound(_)
        ));
    }

    #[test]
    fn test_recursive_reverse_direction_propagation() {
        let (config, registry) = test_config_and_registry();
        let order_id = Uuid::new_v4();
        // orders/{id}/owner → reverse → navigates to user
        let result = RecursiveLinkExtractor::from_segments(
            vec![
                "orders".to_string(),
                order_id.to_string(),
                "owner".to_string(),
            ],
            &registry,
            &config,
        );
        assert!(result.is_ok());
        let ext = result.expect("should succeed");
        assert_eq!(ext.chain.len(), 2);
        assert_eq!(ext.chain[0].entity_type, "order");
        assert!(matches!(
            ext.chain[0].link_direction,
            Some(LinkDirection::Reverse)
        ));
        // Reverse direction → target entity is source_type (user)
        assert_eq!(ext.chain[1].entity_type, "user");
    }

    // === final_target / final_link_def / penultimate_segment ===

    #[test]
    fn test_final_target_returns_last_segment() {
        let (config, registry) = test_config_and_registry();
        let user_id = Uuid::new_v4();
        let ext = RecursiveLinkExtractor::from_segments(
            vec![
                "users".to_string(),
                user_id.to_string(),
                "orders-owned".to_string(),
            ],
            &registry,
            &config,
        )
        .expect("should succeed");
        let (id, entity_type) = ext.final_target();
        assert_eq!(entity_type, "order");
        assert!(id.is_nil()); // list target
    }

    #[test]
    fn test_final_link_def_returns_penultimate_link() {
        let (config, registry) = test_config_and_registry();
        let user_id = Uuid::new_v4();
        let ext = RecursiveLinkExtractor::from_segments(
            vec![
                "users".to_string(),
                user_id.to_string(),
                "orders-owned".to_string(),
            ],
            &registry,
            &config,
        )
        .expect("should succeed");
        let link_def = ext.final_link_def();
        assert!(link_def.is_some());
        assert_eq!(link_def.expect("should have link").link_type, "ownership");
    }

    #[test]
    fn test_final_link_def_single_segment_returns_none() {
        let (config, registry) = test_config_and_registry();
        let ext = RecursiveLinkExtractor::from_segments(
            vec!["users".to_string(), Uuid::new_v4().to_string()],
            &registry,
            &config,
        )
        .expect("should succeed");
        assert!(ext.final_link_def().is_none());
    }

    #[test]
    fn test_penultimate_segment_returns_correct() {
        let (config, registry) = test_config_and_registry();
        let user_id = Uuid::new_v4();
        let ext = RecursiveLinkExtractor::from_segments(
            vec![
                "users".to_string(),
                user_id.to_string(),
                "orders-owned".to_string(),
            ],
            &registry,
            &config,
        )
        .expect("should succeed");
        let pen = ext.penultimate_segment();
        assert!(pen.is_some());
        assert_eq!(pen.expect("should exist").entity_type, "user");
        assert_eq!(pen.expect("should exist").entity_id, user_id);
    }

    #[test]
    fn test_penultimate_segment_single_segment_returns_none() {
        let (config, registry) = test_config_and_registry();
        let ext = RecursiveLinkExtractor::from_segments(
            vec!["users".to_string(), Uuid::new_v4().to_string()],
            &registry,
            &config,
        )
        .expect("should succeed");
        assert!(ext.penultimate_segment().is_none());
    }
}
