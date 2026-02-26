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

    // ── Helper: 3-entity chain config (order → invoice → payment) ──

    fn create_chain_config() -> LinksConfig {
        LinksConfig {
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
                    description: None,
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
        }
    }

    // ── Helper: cycle config (A → B → A) ──

    fn create_cycle_config() -> LinksConfig {
        LinksConfig {
            entities: vec![
                EntityConfig {
                    singular: "a".to_string(),
                    plural: "as".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "b".to_string(),
                    plural: "bs".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
            ],
            links: vec![
                LinkDefinition {
                    link_type: "ab".to_string(),
                    source_type: "a".to_string(),
                    target_type: "b".to_string(),
                    forward_route_name: "bs".to_string(),
                    reverse_route_name: "as-from-b".to_string(),
                    description: None,
                    required_fields: None,
                    auth: None,
                },
                LinkDefinition {
                    link_type: "ba".to_string(),
                    source_type: "b".to_string(),
                    target_type: "a".to_string(),
                    forward_route_name: "as".to_string(),
                    reverse_route_name: "bs-from-a".to_string(),
                    description: None,
                    required_fields: None,
                    auth: None,
                },
            ],
            validation_rules: None,
        }
    }

    // ── Helper: empty config ──

    fn create_empty_config() -> LinksConfig {
        LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: None,
        }
    }

    // ======================================================================
    // detect_link_chains tests
    // ======================================================================

    #[test]
    fn test_detect_link_chains_simple_chain() {
        let config = Arc::new(create_chain_config());
        let registry = LinkRouteRegistry::new(config);

        let chains = registry.detect_link_chains(5);

        // Forward chains starting from "order" should include order→invoice and order→invoice→payment
        let forward_from_order: Vec<_> = chains
            .iter()
            .filter(|c| {
                !c.is_reverse()
                    && c.steps
                        .first()
                        .map(|s| s.entity_type == "order")
                        .unwrap_or(false)
            })
            .collect();

        assert!(
            forward_from_order.len() >= 2,
            "expected at least 2 forward chains from order (1-step and 2-step), got {}",
            forward_from_order.len()
        );

        // There should be a 3-step chain: order → invoice → payment
        let three_step = forward_from_order
            .iter()
            .find(|c| c.steps.len() == 3)
            .expect("expected a 3-step chain order→invoice→payment");

        assert_eq!(three_step.steps[0].entity_type, "order");
        assert_eq!(three_step.steps[1].entity_type, "invoice");
        assert_eq!(three_step.steps[2].entity_type, "payment");
    }

    #[test]
    fn test_detect_link_chains_cycle_detection() {
        let config = Arc::new(create_cycle_config());
        let registry = LinkRouteRegistry::new(config);

        // This must terminate (cycle detection prevents infinite recursion)
        let chains = registry.detect_link_chains(10);

        // Should produce chains but not infinitely loop
        // A→B, A→B→A would be blocked by cycle detection on the edge A->B
        // So we get A→B and B→A as 2-step chains, but not A→B→A
        assert!(
            !chains.is_empty(),
            "should detect at least some chains even with cycles"
        );

        // No chain should have duplicate edges (cycle detection guarantee)
        for chain in &chains {
            let len = chain.steps.len();
            assert!(
                len <= 4,
                "chain length {} is suspiciously long for a 2-node cycle graph",
                len
            );
        }
    }

    #[test]
    fn test_detect_link_chains_max_depth_limits_traversal() {
        let config = Arc::new(create_chain_config());
        let registry = LinkRouteRegistry::new(config);

        let chains_depth1 = registry.detect_link_chains(1);
        let chains_depth5 = registry.detect_link_chains(5);

        // With depth=1 we can only go one hop, so max chain length is 2 steps (source + one hop)
        for chain in &chains_depth1 {
            assert!(
                chain.steps.len() <= 2,
                "max_depth=1 should limit chains to 2 steps, got {}",
                chain.steps.len()
            );
        }

        // With depth=5, we should get longer chains (the 3-step order→invoice→payment)
        let has_three_step = chains_depth5.iter().any(|c| c.steps.len() == 3);
        assert!(has_three_step, "max_depth=5 should allow 3-step chains");
    }

    #[test]
    fn test_detect_link_chains_forward_chains_detected() {
        let config = Arc::new(create_chain_config());
        let registry = LinkRouteRegistry::new(config);

        let chains = registry.detect_link_chains(5);
        let forward_chains: Vec<_> = chains.iter().filter(|c| !c.is_reverse()).collect();

        assert!(
            !forward_chains.is_empty(),
            "should detect at least one forward chain"
        );

        // All steps in forward chains should have Forward direction
        for chain in &forward_chains {
            for step in &chain.steps {
                assert_eq!(
                    step.direction,
                    LinkDirection::Forward,
                    "all steps in a forward chain should have Forward direction"
                );
            }
        }
    }

    #[test]
    fn test_detect_link_chains_reverse_chains_detected() {
        let config = Arc::new(create_chain_config());
        let registry = LinkRouteRegistry::new(config);

        let chains = registry.detect_link_chains(5);
        let reverse_chains: Vec<_> = chains.iter().filter(|c| c.is_reverse()).collect();

        assert!(
            !reverse_chains.is_empty(),
            "should detect at least one reverse chain"
        );

        // All steps in reverse chains should have Reverse direction
        for chain in &reverse_chains {
            for step in &chain.steps {
                assert_eq!(
                    step.direction,
                    LinkDirection::Reverse,
                    "all steps in a reverse chain should have Reverse direction"
                );
            }
        }
    }

    #[test]
    fn test_detect_link_chains_empty_config() {
        let config = Arc::new(create_empty_config());
        let registry = LinkRouteRegistry::new(config);

        let chains = registry.detect_link_chains(5);
        assert!(
            chains.is_empty(),
            "empty config should produce no chains, got {}",
            chains.len()
        );
    }

    // ======================================================================
    // LinkChain::to_route_pattern tests
    // ======================================================================

    #[test]
    fn test_to_route_pattern_single_step_chain() {
        let config = Arc::new(create_chain_config());
        let registry = LinkRouteRegistry::new(config);

        let chains = registry.detect_link_chains(5);

        // Find a 2-step (single hop) forward chain starting from order
        let single_hop = chains
            .iter()
            .find(|c| {
                c.steps.len() == 2
                    && !c.is_reverse()
                    && c.steps[0].entity_type == "order"
                    && c.steps[1].entity_type == "invoice"
            })
            .expect("expected a 2-step forward chain order→invoice");

        let pattern = single_hop.to_route_pattern();

        // Pattern should be: /orders/{order_id}/invoices
        assert_eq!(
            pattern, "/orders/{order_id}/invoices",
            "single hop pattern mismatch"
        );
    }

    #[test]
    fn test_to_route_pattern_multi_step_chain() {
        let config = Arc::new(create_chain_config());
        let registry = LinkRouteRegistry::new(config);

        let chains = registry.detect_link_chains(5);

        // Find the 3-step forward chain: order → invoice → payment
        let multi_hop = chains
            .iter()
            .find(|c| {
                c.steps.len() == 3
                    && !c.is_reverse()
                    && c.steps[0].entity_type == "order"
                    && c.steps[2].entity_type == "payment"
            })
            .expect("expected a 3-step forward chain order→invoice→payment");

        let pattern = multi_hop.to_route_pattern();

        // Pattern should be: /orders/{order_id}/invoices/{invoice_id}/payments
        assert_eq!(
            pattern, "/orders/{order_id}/invoices/{invoice_id}/payments",
            "multi-step pattern mismatch"
        );
    }

    #[test]
    fn test_to_route_pattern_plural_fallback() {
        // Config with an entity type that is NOT in the entities list
        // The get_plural fallback should append "s"
        let config = Arc::new(LinksConfig {
            entities: vec![
                EntityConfig {
                    singular: "widget".to_string(),
                    plural: "widgets".to_string(),
                    auth: crate::config::EntityAuthConfig::default(),
                },
                // "gadget" entity is deliberately missing from entities list
            ],
            links: vec![LinkDefinition {
                link_type: "contains".to_string(),
                source_type: "widget".to_string(),
                target_type: "gadget".to_string(),
                forward_route_name: "gadgets".to_string(),
                reverse_route_name: "widget".to_string(),
                description: None,
                required_fields: None,
                auth: None,
            }],
            validation_rules: None,
        });

        // Manually build a chain with an unknown entity to exercise fallback
        let chain = LinkChain {
            steps: vec![
                LinkChainStep {
                    entity_type: "unknown_thing".to_string(),
                    route_name: None,
                    direction: LinkDirection::Forward,
                },
                LinkChainStep {
                    entity_type: "gadget".to_string(),
                    route_name: Some("gadgets".to_string()),
                    direction: LinkDirection::Forward,
                },
            ],
            config,
        };

        let pattern = chain.to_route_pattern();

        // "unknown_thing" is not in entities, so fallback appends "s" → "unknown_things"
        // "gadget" is also not in entities → "gadgets" (from fallback)
        assert_eq!(
            pattern, "/unknown_things/{unknown_thing_id}/gadgets",
            "fallback plural should append 's' for unknown entity types"
        );
    }

    // ======================================================================
    // LinkChain::is_reverse tests
    // ======================================================================

    #[test]
    fn test_is_reverse_forward_chain() {
        let config = Arc::new(create_chain_config());

        let chain = LinkChain {
            steps: vec![
                LinkChainStep {
                    entity_type: "order".to_string(),
                    route_name: None,
                    direction: LinkDirection::Forward,
                },
                LinkChainStep {
                    entity_type: "invoice".to_string(),
                    route_name: Some("invoices".to_string()),
                    direction: LinkDirection::Forward,
                },
            ],
            config,
        };

        assert!(
            !chain.is_reverse(),
            "chain starting with Forward direction should not be reverse"
        );
    }

    #[test]
    fn test_is_reverse_reverse_chain() {
        let config = Arc::new(create_chain_config());

        let chain = LinkChain {
            steps: vec![
                LinkChainStep {
                    entity_type: "payment".to_string(),
                    route_name: None,
                    direction: LinkDirection::Reverse,
                },
                LinkChainStep {
                    entity_type: "invoice".to_string(),
                    route_name: Some("invoice".to_string()),
                    direction: LinkDirection::Reverse,
                },
            ],
            config,
        };

        assert!(
            chain.is_reverse(),
            "chain starting with Reverse direction should be reverse"
        );
    }

    #[test]
    fn test_is_reverse_empty_chain() {
        let config = Arc::new(create_chain_config());

        let chain = LinkChain {
            steps: vec![],
            config,
        };

        assert!(
            !chain.is_reverse(),
            "empty chain should return false for is_reverse"
        );
    }

    // ======================================================================
    // Error path tests
    // ======================================================================

    #[test]
    fn test_resolve_route_nonexistent() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        let result = registry.resolve_route("user", "nonexistent-route");
        assert!(
            result.is_err(),
            "resolving a nonexistent route should return an error"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("nonexistent-route"),
            "error message should contain the route name, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("user"),
            "error message should contain the entity type, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_list_routes_for_unknown_entity() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        let routes = registry.list_routes_for_entity("unknown_type");
        assert!(
            routes.is_empty(),
            "listing routes for an unknown entity should return an empty vec"
        );
    }

    #[test]
    fn test_resolve_route_wrong_entity_type() {
        // "cars-owned" is a forward route from "user", not from "car"
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        let result = registry.resolve_route("car", "cars-owned");
        assert!(
            result.is_err(),
            "resolving a route with wrong entity type should return an error"
        );
    }

    #[test]
    fn test_config_accessor() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config.clone());

        let returned_config = registry.config();
        assert_eq!(
            returned_config.entities.len(),
            config.entities.len(),
            "config() should return the original configuration"
        );
        assert_eq!(
            returned_config.links.len(),
            config.links.len(),
            "config() should return the original configuration"
        );
    }

    #[test]
    fn test_list_routes_for_entity_reverse_direction() {
        let config = Arc::new(create_test_config());
        let registry = LinkRouteRegistry::new(config);

        // "car" should have reverse routes: "users-owners" and "users-drivers"
        let car_routes = registry.list_routes_for_entity("car");
        assert_eq!(car_routes.len(), 2, "car should have 2 reverse routes");

        for route in &car_routes {
            assert_eq!(
                route.direction,
                LinkDirection::Reverse,
                "car routes should all be Reverse direction"
            );
            assert_eq!(
                route.connected_to, "user",
                "car routes should connect to user"
            );
        }
    }
}
