//! Route registry for link navigation
//!
//! Provides resolution of route names to link definitions and handles
//! bidirectional navigation (forward and reverse)

use crate::config::LinksConfig;
use crate::core::LinkDefinition;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;

/// Direction of link navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDirection {
    /// From source to target
    Forward,
    /// From target to source
    Reverse,
}

/// Registry for resolving route names to link definitions
///
/// This allows the framework to map URL paths like "/users/{id}/cars-owned"
/// to the appropriate link definition and direction.
pub struct LinkRouteRegistry {
    config: Arc<LinksConfig>,
    /// Maps (entity_type, route_name) -> (LinkDefinition, LinkDirection)
    routes: HashMap<(String, String), (LinkDefinition, LinkDirection)>,
}

impl LinkRouteRegistry {
    /// Create a new registry from a links configuration
    pub fn new(config: Arc<LinksConfig>) -> Self {
        let mut routes = HashMap::new();

        // Build the routing table
        for link_def in &config.links {
            // Forward route: source -> target
            let forward_key = (
                link_def.source_type.clone(),
                link_def.forward_route_name.clone(),
            );
            routes.insert(forward_key, (link_def.clone(), LinkDirection::Forward));

            // Reverse route: target -> source
            let reverse_key = (
                link_def.target_type.clone(),
                link_def.reverse_route_name.clone(),
            );
            routes.insert(reverse_key, (link_def.clone(), LinkDirection::Reverse));
        }

        Self { config, routes }
    }

    /// Resolve a route name for a given entity type
    ///
    /// Returns the link definition and the direction of navigation
    pub fn resolve_route(
        &self,
        entity_type: &str,
        route_name: &str,
    ) -> Result<(LinkDefinition, LinkDirection)> {
        let key = (entity_type.to_string(), route_name.to_string());

        self.routes.get(&key).cloned().ok_or_else(|| {
            anyhow!(
                "No route '{}' found for entity type '{}'",
                route_name,
                entity_type
            )
        })
    }

    /// List all available routes for a given entity type
    pub fn list_routes_for_entity(&self, entity_type: &str) -> Vec<RouteInfo> {
        self.routes
            .iter()
            .filter(|((etype, _), _)| etype == entity_type)
            .map(|((_, route_name), (link_def, direction))| {
                let connected_to = match direction {
                    LinkDirection::Forward => &link_def.target_type,
                    LinkDirection::Reverse => &link_def.source_type,
                };

                RouteInfo {
                    route_name: route_name.clone(),
                    link_type: link_def.link_type.clone(),
                    direction: *direction,
                    connected_to: connected_to.clone(),
                    description: link_def.description.clone(),
                }
            })
            .collect()
    }

    /// Get the underlying configuration
    pub fn config(&self) -> &LinksConfig {
        &self.config
    }
}

/// Information about a route available for an entity
#[derive(Debug, Clone)]
pub struct RouteInfo {
    /// The route name (e.g., "cars-owned")
    pub route_name: String,

    /// The type of link (e.g., "owner")
    pub link_type: String,

    /// Direction of the relationship
    pub direction: LinkDirection,

    /// The entity type this route connects to
    pub connected_to: String,

    /// Optional description
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EntityConfig;

    fn create_test_config() -> LinksConfig {
        LinksConfig {
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
            links: vec![
                LinkDefinition {
                    link_type: "owner".to_string(),
                    source_type: "user".to_string(),
                    target_type: "car".to_string(),
                    forward_route_name: "cars-owned".to_string(),
                    reverse_route_name: "users-owners".to_string(),
                    description: Some("User owns a car".to_string()),
                    required_fields: None,
                    auth: None,
                },
                LinkDefinition {
                    link_type: "driver".to_string(),
                    source_type: "user".to_string(),
                    target_type: "car".to_string(),
                    forward_route_name: "cars-driven".to_string(),
                    reverse_route_name: "users-drivers".to_string(),
                    description: Some("User drives a car".to_string()),
                    required_fields: None,
                    auth: None,
                },
            ],
            validation_rules: None,
        }
    }

    #[test]
    fn test_resolve_forward_route() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        let (def, direction) = registry.resolve_route("user", "cars-owned").unwrap();

        assert_eq!(def.link_type, "owner");
        assert_eq!(def.source_type, "user");
        assert_eq!(def.target_type, "car");
        assert_eq!(direction, LinkDirection::Forward);
    }

    #[test]
    fn test_resolve_reverse_route() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        let (def, direction) = registry.resolve_route("car", "users-owners").unwrap();

        assert_eq!(def.link_type, "owner");
        assert_eq!(def.source_type, "user");
        assert_eq!(def.target_type, "car");
        assert_eq!(direction, LinkDirection::Reverse);
    }

    #[test]
    fn test_list_routes_for_entity() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        let routes = registry.list_routes_for_entity("user");

        assert_eq!(routes.len(), 2);

        let route_names: Vec<_> = routes.iter().map(|r| r.route_name.as_str()).collect();
        assert!(route_names.contains(&"cars-owned"));
        assert!(route_names.contains(&"cars-driven"));
    }

    #[test]
    fn test_no_route_conflicts() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        let user_routes = registry.list_routes_for_entity("user");
        let route_names: Vec<_> = user_routes.iter().map(|r| &r.route_name).collect();

        let unique_names: std::collections::HashSet<_> = route_names.iter().collect();
        assert_eq!(
            route_names.len(),
            unique_names.len(),
            "Route names must be unique"
        );
    }
}
