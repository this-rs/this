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

    /// Detect all possible link chains from the configuration (forward and reverse)
    ///
    /// Returns a list of chains like: (source_type, [(route_name, target_type), ...])
    /// Example: (order, [("invoices", invoice), ("payments", payment)]) for the chain:
    /// Order → Invoice → Payment
    pub fn detect_link_chains(&self, max_depth: usize) -> Vec<LinkChain> {
        let mut chains = Vec::new();

        // Pour chaque type d'entité, trouver toutes les chaînes possibles (forward)
        for entity_config in &self.config.entities {
            self.find_chains_from_entity(
                &entity_config.singular,
                &mut vec![LinkChainStep {
                    entity_type: entity_config.singular.clone(),
                    route_name: None,
                    direction: LinkDirection::Forward,
                }],
                &mut chains,
                max_depth,
                &mut std::collections::HashSet::new(),
            );
        }

        // Pour chaque type d'entité, trouver toutes les chaînes inverses (reverse)
        for entity_config in &self.config.entities {
            self.find_reverse_chains_from_entity(
                &entity_config.singular,
                &mut vec![LinkChainStep {
                    entity_type: entity_config.singular.clone(),
                    route_name: None,
                    direction: LinkDirection::Reverse,
                }],
                &mut chains,
                max_depth,
                &mut std::collections::HashSet::new(),
            );
        }

        chains
    }

    /// Helper to recursively find chains from an entity (forward direction)
    fn find_chains_from_entity(
        &self,
        entity_type: &str,
        current_chain: &mut Vec<LinkChainStep>,
        chains: &mut Vec<LinkChain>,
        remaining_depth: usize,
        visited: &mut std::collections::HashSet<String>,
    ) {
        if remaining_depth == 0 {
            return;
        }

        // Trouver tous les liens sortants de cette entité
        for link_def in &self.config.links {
            if link_def.source_type == entity_type {
                let edge = format!("{}->{}", link_def.source_type, link_def.target_type);

                // Éviter les cycles
                if visited.contains(&edge) {
                    continue;
                }

                visited.insert(edge.clone());

                // Ajouter cette étape à la chaîne
                let route_name = Some(link_def.forward_route_name.clone());

                current_chain.push(LinkChainStep {
                    entity_type: link_def.target_type.clone(),
                    route_name,
                    direction: LinkDirection::Forward,
                });

                // Si c'est une chaîne valide (au moins 2 steps), l'ajouter
                if current_chain.len() >= 2 {
                    chains.push(LinkChain {
                        steps: current_chain.clone(),
                        config: self.config.clone(),
                    });
                }

                // Continuer récursivement
                self.find_chains_from_entity(
                    &link_def.target_type,
                    current_chain,
                    chains,
                    remaining_depth - 1,
                    visited,
                );

                // Retirer cette étape
                visited.remove(&edge);
                current_chain.pop();
            }
        }
    }

    /// Helper to recursively find chains from an entity (reverse direction)
    fn find_reverse_chains_from_entity(
        &self,
        entity_type: &str,
        current_chain: &mut Vec<LinkChainStep>,
        chains: &mut Vec<LinkChain>,
        remaining_depth: usize,
        visited: &mut std::collections::HashSet<String>,
    ) {
        if remaining_depth == 0 {
            return;
        }

        // Trouver tous les liens entrants de cette entité
        for link_def in &self.config.links {
            if link_def.target_type == entity_type {
                let edge = format!("{}<-{}", link_def.source_type, link_def.target_type);

                // Éviter les cycles
                if visited.contains(&edge) {
                    continue;
                }

                visited.insert(edge.clone());

                // Ajouter cette étape à la chaîne (avec reverse route name)
                let route_name = Some(link_def.reverse_route_name.clone());

                current_chain.push(LinkChainStep {
                    entity_type: link_def.source_type.clone(),
                    route_name,
                    direction: LinkDirection::Reverse,
                });

                // Si c'est une chaîne valide (au moins 2 steps), l'ajouter
                if current_chain.len() >= 2 {
                    chains.push(LinkChain {
                        steps: current_chain.clone(),
                        config: self.config.clone(),
                    });
                }

                // Continuer récursivement
                self.find_reverse_chains_from_entity(
                    &link_def.source_type,
                    current_chain,
                    chains,
                    remaining_depth - 1,
                    visited,
                );

                // Retirer cette étape
                visited.remove(&edge);
                current_chain.pop();
            }
        }
    }
}

/// Une chaîne de liens détectée
#[derive(Debug, Clone)]
pub struct LinkChain {
    pub steps: Vec<LinkChainStep>,
    pub config: Arc<LinksConfig>,
}

/// Une étape dans une chaîne de liens
#[derive(Debug, Clone)]
pub struct LinkChainStep {
    pub entity_type: String,
    pub route_name: Option<String>,
    pub direction: LinkDirection,
}

impl LinkChain {
    /// Génère le pattern de route Axum pour cette chaîne
    ///
    /// Exemple forward: order → invoice → payment
    ///   "/orders/{order_id}/invoices/{invoice_id}/payments"
    ///
    /// Exemple reverse: payment ← invoice ← order
    ///   "/payments/{payment_id}/invoice/{invoice_id}/orders"
    pub fn to_route_pattern(&self) -> String {
        let mut pattern = String::new();
        let steps_count = self.steps.len();

        for (idx, step) in self.steps.iter().enumerate() {
            if step.route_name.is_none() {
                // Premier step: entité source
                let plural = self.get_plural(&step.entity_type);
                let param_name = format!("{}_id", step.entity_type);
                pattern.push_str(&format!("/{plural}/{{{}}}", param_name));
            } else if let Some(route_name) = &step.route_name {
                // Step intermédiaire avec route
                // Pour le dernier step, utiliser le pluriel au lieu du route_name
                let segment = if idx == steps_count - 1 {
                    // Dernier step: utiliser le pluriel
                    self.get_plural(&step.entity_type)
                } else {
                    // Step intermédiaire: utiliser le route_name
                    route_name.clone()
                };
                pattern.push_str(&format!("/{segment}"));

                // Ajouter le param ID pour ce step
                // SAUF si c'est le dernier step (pour la route de liste)
                if idx < steps_count - 1 {
                    let param_name = format!("{}_id", step.entity_type);
                    pattern.push_str(&format!("/{{{}}}", param_name));
                }
            }
        }

        pattern
    }

    /// Indique si cette chaîne est en sens inverse
    pub fn is_reverse(&self) -> bool {
        self.steps
            .first()
            .map(|s| s.direction == LinkDirection::Reverse)
            .unwrap_or(false)
    }

    fn get_plural(&self, singular: &str) -> String {
        self.config
            .entities
            .iter()
            .find(|e| e.singular == singular)
            .map(|e| e.plural.clone())
            .unwrap_or_else(|| format!("{}s", singular))
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
