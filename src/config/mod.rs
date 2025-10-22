//! Configuration loading and management

use crate::core::LinkDefinition;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for an entity type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityConfig {
    /// Singular form (e.g., "user", "company")
    pub singular: String,

    /// Plural form (e.g., "users", "companies")
    pub plural: String,
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
                },
                EntityConfig {
                    singular: "company".to_string(),
                    plural: "companies".to_string(),
                },
                EntityConfig {
                    singular: "car".to_string(),
                    plural: "cars".to_string(),
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
                },
                LinkDefinition {
                    link_type: "driver".to_string(),
                    source_type: "user".to_string(),
                    target_type: "car".to_string(),
                    forward_route_name: "cars-driven".to_string(),
                    reverse_route_name: "users-drivers".to_string(),
                    description: Some("User drives a car".to_string()),
                    required_fields: None,
                },
                LinkDefinition {
                    link_type: "worker".to_string(),
                    source_type: "user".to_string(),
                    target_type: "company".to_string(),
                    forward_route_name: "companies-work".to_string(),
                    reverse_route_name: "users-workers".to_string(),
                    description: Some("User works at a company".to_string()),
                    required_fields: Some(vec!["role".to_string()]),
                },
            ],
            validation_rules: None,
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
}
