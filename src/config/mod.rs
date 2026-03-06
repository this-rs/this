//! Configuration loading and management

pub mod events;
pub mod sinks;

use crate::core::LinkDefinition;
use anyhow::Result;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use events::*;
pub use sinks::*;

/// Authorization configuration for an entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAuthConfig {
    /// Policy for listing entities (GET /{entities})
    #[serde(default = "default_auth_policy")]
    pub list: String,

    /// Policy for getting a single entity (GET /{entities}/{id})
    #[serde(default = "default_auth_policy")]
    pub get: String,

    /// Policy for creating an entity (POST /{entities})
    #[serde(default = "default_auth_policy")]
    pub create: String,

    /// Policy for updating an entity (PUT /{entities}/{id})
    #[serde(default = "default_auth_policy")]
    pub update: String,

    /// Policy for deleting an entity (DELETE /{entities}/{id})
    #[serde(default = "default_auth_policy")]
    pub delete: String,

    /// Policy for listing links (GET /{entities}/{id}/{link_route})
    #[serde(default = "default_auth_policy")]
    pub list_links: String,

    /// Policy for creating links (POST /{entities}/{id}/{link_type}/{target_type}/{target_id})
    #[serde(default = "default_auth_policy")]
    pub create_link: String,

    /// Policy for deleting links (DELETE /{entities}/{id}/{link_type}/{target_type}/{target_id})
    #[serde(default = "default_auth_policy")]
    pub delete_link: String,
}

fn default_auth_policy() -> String {
    "authenticated".to_string()
}

impl Default for EntityAuthConfig {
    fn default() -> Self {
        Self {
            list: default_auth_policy(),
            get: default_auth_policy(),
            create: default_auth_policy(),
            update: default_auth_policy(),
            delete: default_auth_policy(),
            list_links: default_auth_policy(),
            create_link: default_auth_policy(),
            delete_link: default_auth_policy(),
        }
    }
}

/// Configuration for an entity type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityConfig {
    /// Singular form (e.g., "user", "company")
    pub singular: String,

    /// Plural form (e.g., "users", "companies")
    pub plural: String,

    /// Authorization configuration
    #[serde(default)]
    pub auth: EntityAuthConfig,
}

/// Validation rule for a link type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    /// Source entity type
    pub source: String,

    /// Allowed target entity types
    pub targets: Vec<String>,
}

/// Complete configuration for the links system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinksConfig {
    /// List of entity configurations
    pub entities: Vec<EntityConfig>,

    /// List of link definitions
    pub links: Vec<LinkDefinition>,

    /// Optional validation rules (link_type -> rules)
    #[serde(default)]
    pub validation_rules: Option<HashMap<String, Vec<ValidationRule>>>,

    /// Optional event flow configuration (backend, flows, consumers)
    #[serde(default)]
    pub events: Option<EventsConfig>,

    /// Optional sink configurations (notification destinations)
    #[serde(default)]
    pub sinks: Option<Vec<SinkConfig>>,
}

impl LinksConfig {
    /// Load configuration from a YAML file
    pub fn from_yaml_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Load configuration from a YAML string
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(yaml)?;
        Ok(config)
    }

    /// Merge multiple configurations into one
    ///
    /// Rules:
    /// - Entities: Combined from all configs, duplicates (by singular name) use last definition
    /// - Links: Combined from all configs, duplicates (by link_type+source+target) use last definition
    /// - Validation rules: Merged by link_type, rules combined for each link type
    pub fn merge(configs: Vec<LinksConfig>) -> Self {
        if configs.is_empty() {
            return Self {
                entities: vec![],
                links: vec![],
                validation_rules: None,
                events: None,
                sinks: None,
            };
        }

        if configs.len() == 1 {
            return configs.into_iter().next().unwrap();
        }

        let mut entities_map: IndexMap<String, EntityConfig> = IndexMap::new();
        let mut links_map: IndexMap<(String, String, String), LinkDefinition> = IndexMap::new();
        let mut validation_rules_map: HashMap<String, Vec<ValidationRule>> = HashMap::new();

        // Merge entities (last one wins for duplicates)
        for config in &configs {
            for entity in &config.entities {
                entities_map.insert(entity.singular.clone(), entity.clone());
            }
        }

        // Merge links (last one wins for duplicates)
        for config in &configs {
            for link in &config.links {
                let key = (
                    link.link_type.clone(),
                    link.source_type.clone(),
                    link.target_type.clone(),
                );
                links_map.insert(key, link.clone());
            }
        }

        // Merge validation rules (combine rules for same link_type)
        for config in &configs {
            if let Some(rules) = &config.validation_rules {
                for (link_type, link_rules) in rules {
                    validation_rules_map
                        .entry(link_type.clone())
                        .or_default()
                        .extend(link_rules.clone());
                }
            }
        }

        // Merge events: backend last-wins, flows are concatenated (with duplicate warning)
        let mut merged_events: Option<EventsConfig> = None;
        for config in &configs {
            if let Some(events) = &config.events {
                if let Some(ref mut existing) = merged_events {
                    // Backend: last-wins (consistent with entities/links merge behavior)
                    existing.backend = events.backend.clone();
                    existing.flows.extend(events.flows.clone());
                    existing.consumers.extend(events.consumers.clone());
                } else {
                    merged_events = Some(events.clone());
                }
            }
        }

        // Detect duplicate flow names and warn
        if let Some(ref events) = merged_events {
            let mut seen_names = std::collections::HashSet::new();
            for flow in &events.flows {
                if !seen_names.insert(&flow.name) {
                    tracing::warn!(
                        flow_name = %flow.name,
                        "config merge: duplicate flow name detected — \
                         later definition will shadow earlier one at runtime"
                    );
                }
            }
        }

        // Merge sinks: deduplicate by name (last wins), preserving insertion order
        let mut sinks_map: IndexMap<String, SinkConfig> = IndexMap::new();
        for config in &configs {
            if let Some(sinks) = &config.sinks {
                for sink in sinks {
                    sinks_map.insert(sink.name.clone(), sink.clone());
                }
            }
        }
        let merged_sinks = if sinks_map.is_empty() {
            None
        } else {
            Some(sinks_map.into_values().collect())
        };

        // Convert back to vectors
        let entities: Vec<EntityConfig> = entities_map.into_values().collect();
        let links: Vec<LinkDefinition> = links_map.into_values().collect();
        let validation_rules = if validation_rules_map.is_empty() {
            None
        } else {
            Some(validation_rules_map)
        };

        Self {
            entities,
            links,
            validation_rules,
            events: merged_events,
            sinks: merged_sinks,
        }
    }

    /// Validate if a link combination is allowed
    ///
    /// If no validation rules are defined, all combinations are allowed (permissive mode)
    pub fn is_valid_link(&self, link_type: &str, source_type: &str, target_type: &str) -> bool {
        // If no validation rules, accept everything
        let Some(rules) = &self.validation_rules else {
            return true;
        };

        // Check if there are rules for this link type
        let Some(link_rules) = rules.get(link_type) else {
            return true; // No rules for this link type, accept
        };

        // Check if the combination is in the rules
        link_rules.iter().any(|rule| {
            rule.source == source_type && rule.targets.contains(&target_type.to_string())
        })
    }

    /// Find a link definition
    pub fn find_link_definition(
        &self,
        link_type: &str,
        source_type: &str,
        target_type: &str,
    ) -> Option<&LinkDefinition> {
        self.links.iter().find(|def| {
            def.link_type == link_type
                && def.source_type == source_type
                && def.target_type == target_type
        })
    }

    /// Create a default configuration for testing
    pub fn default_config() -> Self {
        Self {
            entities: vec![
                EntityConfig {
                    singular: "user".to_string(),
                    plural: "users".to_string(),
                    auth: EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "company".to_string(),
                    plural: "companies".to_string(),
                    auth: EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "car".to_string(),
                    plural: "cars".to_string(),
                    auth: EntityAuthConfig::default(),
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
                LinkDefinition {
                    link_type: "worker".to_string(),
                    source_type: "user".to_string(),
                    target_type: "company".to_string(),
                    forward_route_name: "companies-work".to_string(),
                    reverse_route_name: "users-workers".to_string(),
                    description: Some("User works at a company".to_string()),
                    required_fields: Some(vec!["role".to_string()]),
                    auth: None,
                },
            ],
            validation_rules: None,
            events: None,
            sinks: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LinksConfig::default_config();

        assert_eq!(config.entities.len(), 3);
        assert_eq!(config.links.len(), 3);
    }

    #[test]
    fn test_yaml_serialization() {
        let config = LinksConfig::default_config();
        let yaml = serde_yaml::to_string(&config).unwrap();

        // Should be able to parse it back
        let parsed = LinksConfig::from_yaml_str(&yaml).unwrap();
        assert_eq!(parsed.entities.len(), config.entities.len());
        assert_eq!(parsed.links.len(), config.links.len());
    }

    #[test]
    fn test_link_auth_config_parsing() {
        let yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    auth:
      list: authenticated
      create: service_only
      delete: admin_only
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.links.len(), 1);

        let link_def = &config.links[0];
        assert!(link_def.auth.is_some());

        let auth = link_def.auth.as_ref().unwrap();
        assert_eq!(auth.list, "authenticated");
        assert_eq!(auth.create, "service_only");
        assert_eq!(auth.delete, "admin_only");
    }

    #[test]
    fn test_link_without_auth_config() {
        let yaml = r#"
entities:
  - singular: invoice
    plural: invoices
  - singular: payment
    plural: payments

links:
  - link_type: payment
    source_type: invoice
    target_type: payment
    forward_route_name: payments
    reverse_route_name: invoice
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.links.len(), 1);

        let link_def = &config.links[0];
        assert!(link_def.auth.is_none());
    }

    #[test]
    fn test_mixed_link_auth_configs() {
        let yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices
  - singular: payment
    plural: payments

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    auth:
      list: authenticated
      create: service_only
      delete: admin_only
  
  - link_type: payment
    source_type: invoice
    target_type: payment
    forward_route_name: payments
    reverse_route_name: invoice
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.links.len(), 2);

        // First link has auth
        assert!(config.links[0].auth.is_some());
        let auth1 = config.links[0].auth.as_ref().unwrap();
        assert_eq!(auth1.create, "service_only");

        // Second link has no auth
        assert!(config.links[1].auth.is_none());
    }

    #[test]
    fn test_merge_empty() {
        let merged = LinksConfig::merge(vec![]);
        assert_eq!(merged.entities.len(), 0);
        assert_eq!(merged.links.len(), 0);
    }

    #[test]
    fn test_merge_single() {
        let config = LinksConfig::default_config();
        let merged = LinksConfig::merge(vec![config.clone()]);
        assert_eq!(merged.entities.len(), config.entities.len());
        assert_eq!(merged.links.len(), config.links.len());
    }

    #[test]
    fn test_merge_disjoint_configs() {
        let config1 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
        };

        let config2 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "invoice".to_string(),
                plural: "invoices".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
        };

        let merged = LinksConfig::merge(vec![config1, config2]);
        assert_eq!(merged.entities.len(), 2);
    }

    #[test]
    fn test_merge_overlapping_entities() {
        let auth1 = EntityAuthConfig {
            list: "public".to_string(),
            ..Default::default()
        };

        let config1 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "user".to_string(),
                plural: "users".to_string(),
                auth: auth1,
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
        };

        let auth2 = EntityAuthConfig {
            list: "authenticated".to_string(),
            ..Default::default()
        };

        let config2 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "user".to_string(),
                plural: "users".to_string(),
                auth: auth2,
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
        };

        let merged = LinksConfig::merge(vec![config1, config2]);

        // Should have only 1 entity (last wins)
        assert_eq!(merged.entities.len(), 1);
        assert_eq!(merged.entities[0].auth.list, "authenticated");
    }

    #[test]
    fn test_merge_validation_rules() {
        let mut rules1 = HashMap::new();
        rules1.insert(
            "works_at".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec!["company".to_string()],
            }],
        );

        let config1 = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules1),
            events: None,
            sinks: None,
        };

        let mut rules2 = HashMap::new();
        rules2.insert(
            "works_at".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec!["project".to_string()],
            }],
        );

        let config2 = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules2),
            events: None,
            sinks: None,
        };

        let merged = LinksConfig::merge(vec![config1, config2]);

        // Validation rules should be combined
        assert!(merged.validation_rules.is_some());
        let rules = merged.validation_rules.unwrap();
        assert_eq!(rules["works_at"].len(), 2);
    }

    #[test]
    fn test_find_link_definition_found() {
        let config = LinksConfig::default_config();

        let def = config.find_link_definition("owner", "user", "car");
        assert!(def.is_some(), "should find owner link from user to car");
        let def = def.expect("link definition should exist");
        assert_eq!(def.link_type, "owner");
        assert_eq!(def.source_type, "user");
        assert_eq!(def.target_type, "car");
    }

    #[test]
    fn test_find_link_definition_not_found() {
        let config = LinksConfig::default_config();

        let def = config.find_link_definition("nonexistent", "user", "car");
        assert!(def.is_none(), "should not find a nonexistent link type");

        // Wrong source type
        let def = config.find_link_definition("owner", "company", "car");
        assert!(def.is_none(), "should not find link with wrong source type");
    }

    #[test]
    fn test_is_valid_link_source_type_mismatch() {
        let mut rules = HashMap::new();
        rules.insert(
            "owner".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec!["car".to_string()],
            }],
        );

        let config = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules),
            events: None,
            sinks: None,
        };

        // Correct combination
        assert!(config.is_valid_link("owner", "user", "car"));

        // Source type mismatch
        assert!(
            !config.is_valid_link("owner", "company", "car"),
            "should reject mismatched source type"
        );

        // Target type mismatch
        assert!(
            !config.is_valid_link("owner", "user", "truck"),
            "should reject mismatched target type"
        );
    }

    #[test]
    fn test_is_valid_link_empty_targets() {
        let mut rules = HashMap::new();
        rules.insert(
            "membership".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec![], // empty targets list
            }],
        );

        let config = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules),
            events: None,
            sinks: None,
        };

        // With empty targets, no target type can match
        assert!(
            !config.is_valid_link("membership", "user", "group"),
            "should reject when targets list is empty"
        );
    }

    #[test]
    fn test_yaml_backward_compatible_without_events() {
        // Old-style YAML without events/sinks should still parse
        let yaml = r#"
entities:
  - singular: user
    plural: users
links:
  - link_type: follows
    source_type: user
    target_type: user
    forward_route_name: following
    reverse_route_name: followers
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.entities.len(), 1);
        assert_eq!(config.links.len(), 1);
        assert!(config.events.is_none());
        assert!(config.sinks.is_none());
    }

    #[test]
    fn test_yaml_with_events_and_sinks() {
        let yaml = r#"
entities:
  - singular: user
    plural: users
  - singular: capture
    plural: captures

links:
  - link_type: follows
    source_type: user
    target_type: user
    forward_route_name: following
    reverse_route_name: followers
  - link_type: likes
    source_type: user
    target_type: capture
    forward_route_name: liked-captures
    reverse_route_name: likers
  - link_type: owns
    source_type: user
    target_type: capture
    forward_route_name: captures
    reverse_route_name: owner

events:
  backend:
    type: memory
  flows:
    - name: notify-new-follower
      trigger:
        kind: link.created
        link_type: follows
      pipeline:
        - resolve:
            from: source_id
            as: follower
        - map:
            template:
              type: follow
              message: "{{ follower.name }} started following you"
        - deliver:
            sinks: [push-notification, in-app-notification]
    - name: notify-like
      trigger:
        kind: link.created
        link_type: likes
      pipeline:
        - resolve:
            from: target_id
            via: owns
            direction: reverse
            as: owner
        - filter:
            condition: "source_id != owner.id"
        - batch:
            key: target_id
            window: 5m
        - deliver:
            sink: push-notification
  consumers:
    - name: mobile-feed
      seek: last_acknowledged

sinks:
  - name: push-notification
    type: push
    config:
      provider: expo
  - name: in-app-notification
    type: in_app
    config:
      ttl: 30d
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();

        // Entities and links
        assert_eq!(config.entities.len(), 2);
        assert_eq!(config.links.len(), 3);

        // Events
        assert!(config.events.is_some());
        let events = config.events.as_ref().unwrap();
        assert_eq!(events.backend.backend_type, "memory");
        assert_eq!(events.flows.len(), 2);
        assert_eq!(events.flows[0].name, "notify-new-follower");
        assert_eq!(events.flows[1].name, "notify-like");
        assert_eq!(events.consumers.len(), 1);
        assert_eq!(events.consumers[0].name, "mobile-feed");

        // Sinks
        assert!(config.sinks.is_some());
        let sinks = config.sinks.as_ref().unwrap();
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].name, "push-notification");
        assert_eq!(sinks[0].sink_type, SinkType::Push);
        assert_eq!(sinks[1].name, "in-app-notification");
        assert_eq!(sinks[1].sink_type, SinkType::InApp);
    }

    #[test]
    fn test_merge_configs_with_events() {
        let config1 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "user".to_string(),
                plural: "users".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
            events: Some(EventsConfig {
                backend: BackendConfig::default(),
                flows: vec![FlowConfig {
                    name: "flow-a".to_string(),
                    description: None,
                    trigger: TriggerConfig {
                        kind: "link.created".to_string(),
                        link_type: Some("follows".to_string()),
                        entity_type: None,
                    },
                    pipeline: vec![],
                }],
                consumers: vec![],
            }),
            sinks: Some(vec![SinkConfig {
                name: "push".to_string(),
                sink_type: SinkType::Push,
                config: HashMap::new(),
            }]),
        };

        let config2 = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: None,
            events: Some(EventsConfig {
                backend: BackendConfig::default(),
                flows: vec![FlowConfig {
                    name: "flow-b".to_string(),
                    description: None,
                    trigger: TriggerConfig {
                        kind: "entity.created".to_string(),
                        link_type: None,
                        entity_type: Some("user".to_string()),
                    },
                    pipeline: vec![],
                }],
                consumers: vec![ConsumerConfig {
                    name: "mobile".to_string(),
                    seek: SeekMode::LastAcknowledged,
                }],
            }),
            sinks: Some(vec![SinkConfig {
                name: "in-app".to_string(),
                sink_type: SinkType::InApp,
                config: HashMap::new(),
            }]),
        };

        let merged = LinksConfig::merge(vec![config1, config2]);

        // Events should be merged
        let events = merged.events.unwrap();
        assert_eq!(events.flows.len(), 2);
        assert_eq!(events.flows[0].name, "flow-a");
        assert_eq!(events.flows[1].name, "flow-b");
        assert_eq!(events.consumers.len(), 1);

        // Sinks should be merged (deduplicated by name)
        let sinks = merged.sinks.unwrap();
        assert_eq!(sinks.len(), 2);
    }
}
