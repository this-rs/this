//! Link system for managing relationships between entities

use crate::core::pluralize::Pluralizer;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Reference to an entity instance in a link
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityReference {
    /// The unique ID of the entity
    pub id: Uuid,

    /// The type of entity (e.g., "user", "company", "car")
    ///
    /// CRITICAL: This is a String, not an enum, to maintain complete
    /// decoupling from specific entity types
    pub entity_type: String,
}

impl EntityReference {
    /// Create a new entity reference
    pub fn new(id: Uuid, entity_type: impl Into<String>) -> Self {
        Self {
            id,
            entity_type: entity_type.into(),
        }
    }
}

/// A polymorphic link between two entities
///
/// Links are completely agnostic to the types of entities they connect.
/// This allows the link system to work with any entity types without
/// modification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    /// Unique identifier for this link
    pub id: Uuid,

    /// Tenant ID for multi-tenant isolation
    pub tenant_id: Uuid,

    /// The type of relationship (e.g., "owner", "driver", "worker")
    ///
    /// CRITICAL: This is a String, not an enum, to support any
    /// relationship type without modifying the core framework
    pub link_type: String,

    /// The source entity in this relationship
    pub source: EntityReference,

    /// The target entity in this relationship
    pub target: EntityReference,

    /// Optional metadata for the relationship
    ///
    /// Can store additional context like:
    /// - start_date / end_date for temporal relationships
    /// - role for employment relationships
    /// - permission level for access relationships
    pub metadata: Option<serde_json::Value>,

    /// When this link was created
    pub created_at: DateTime<Utc>,

    /// When this link was last updated
    pub updated_at: DateTime<Utc>,
}

impl Link {
    /// Create a new link
    pub fn new(
        tenant_id: Uuid,
        link_type: impl Into<String>,
        source: EntityReference,
        target: EntityReference,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            tenant_id,
            link_type: link_type.into(),
            source,
            target,
            metadata,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Authorization configuration for link operations
///
/// This allows fine-grained control over who can perform operations
/// on specific link types, independent of entity-level permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkAuthConfig {
    /// Policy for listing links (GET /{source}/{id}/{route_name})
    /// Examples: "authenticated", "owner", "public", "role:admin"
    #[serde(default = "default_link_auth_policy")]
    pub list: String,

    /// Policy for creating a link (POST /{source}/{id}/{link_type}/{target}/{id})
    /// Examples: "owner", "service_only", "role:manager", "source_owner"
    #[serde(default = "default_link_auth_policy")]
    pub create: String,

    /// Policy for deleting a link (DELETE /{source}/{id}/{link_type}/{target}/{id})
    /// Examples: "owner", "admin_only", "source_owner_or_target_owner"
    #[serde(default = "default_link_auth_policy")]
    pub delete: String,
}

fn default_link_auth_policy() -> String {
    "authenticated".to_string()
}

impl Default for LinkAuthConfig {
    fn default() -> Self {
        Self {
            list: default_link_auth_policy(),
            create: default_link_auth_policy(),
            delete: default_link_auth_policy(),
        }
    }
}

/// Configuration for a specific type of link between two entity types
///
/// This defines how entities can be related and how those relationships
/// are exposed through the API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkDefinition {
    /// The type of link (e.g., "owner", "driver")
    pub link_type: String,

    /// The source entity type (e.g., "user")
    pub source_type: String,

    /// The target entity type (e.g., "car")
    pub target_type: String,

    /// Route name when navigating from source to target
    ///
    /// Example: "cars-owned" → /users/{id}/cars-owned
    pub forward_route_name: String,

    /// Route name when navigating from target to source
    ///
    /// Example: "users-owners" → /cars/{id}/users-owners
    pub reverse_route_name: String,

    /// Optional description of this link type
    pub description: Option<String>,

    /// Optional list of required metadata fields
    pub required_fields: Option<Vec<String>>,

    /// Authorization configuration specific to this link type
    ///
    /// When specified, these permissions override entity-level link permissions.
    /// This allows different link types between the same entities to have
    /// different permission requirements.
    ///
    /// Examples:
    /// - order → invoice: create=service_only (auto-created by system)
    /// - order → approval: create=owner (manually created by user)
    #[serde(default)]
    pub auth: Option<LinkAuthConfig>,
}

impl LinkDefinition {
    /// Generate the default forward route name
    ///
    /// Format: {target_plural}-{link_type_plural}
    /// Example: "cars-owned" for (target="car", link_type="owner")
    pub fn default_forward_route_name(target_type: &str, link_type: &str) -> String {
        format!(
            "{}-{}",
            Pluralizer::pluralize(target_type),
            Pluralizer::pluralize(link_type)
        )
    }

    /// Generate the default reverse route name
    ///
    /// Format: {source_plural}-{link_type_plural}
    /// Example: "users-owners" for (source="user", link_type="owner")
    pub fn default_reverse_route_name(source_type: &str, link_type: &str) -> String {
        format!(
            "{}-{}",
            Pluralizer::pluralize(source_type),
            Pluralizer::pluralize(link_type)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_reference_creation() {
        let user_id = Uuid::new_v4();
        let reference = EntityReference::new(user_id, "user");

        assert_eq!(reference.id, user_id);
        assert_eq!(reference.entity_type, "user");
    }

    #[test]
    fn test_link_creation() {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = Link::new(
            tenant_id,
            "owner",
            EntityReference::new(user_id, "user"),
            EntityReference::new(car_id, "car"),
            None,
        );

        assert_eq!(link.tenant_id, tenant_id);
        assert_eq!(link.link_type, "owner");
        assert_eq!(link.source.id, user_id);
        assert_eq!(link.target.id, car_id);
        assert!(link.metadata.is_none());
    }

    #[test]
    fn test_link_with_metadata() {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let company_id = Uuid::new_v4();

        let metadata = serde_json::json!({
            "role": "Senior Developer",
            "start_date": "2024-01-01"
        });

        let link = Link::new(
            tenant_id,
            "worker",
            EntityReference::new(user_id, "user"),
            EntityReference::new(company_id, "company"),
            Some(metadata.clone()),
        );

        assert_eq!(link.metadata, Some(metadata));
    }

    #[test]
    fn test_default_route_names() {
        let forward = LinkDefinition::default_forward_route_name("car", "owner");
        assert_eq!(forward, "cars-owners");

        let reverse = LinkDefinition::default_reverse_route_name("user", "owner");
        assert_eq!(reverse, "users-owners");
    }

    #[test]
    fn test_route_names_with_irregular_plurals() {
        let forward = LinkDefinition::default_forward_route_name("company", "owner");
        assert_eq!(forward, "companies-owners");

        let reverse = LinkDefinition::default_reverse_route_name("company", "worker");
        assert_eq!(reverse, "companies-workers");
    }

    #[test]
    fn test_link_auth_config_default() {
        let auth = LinkAuthConfig::default();
        assert_eq!(auth.list, "authenticated");
        assert_eq!(auth.create, "authenticated");
        assert_eq!(auth.delete, "authenticated");
    }

    #[test]
    fn test_link_definition_with_auth() {
        let yaml = r#"
            link_type: has_invoice
            source_type: order
            target_type: invoice
            forward_route_name: invoices
            reverse_route_name: order
            auth:
                list: authenticated
                create: service_only
                delete: admin_only
        "#;

        let def: LinkDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.link_type, "has_invoice");
        assert_eq!(def.source_type, "order");
        assert_eq!(def.target_type, "invoice");

        let auth = def.auth.unwrap();
        assert_eq!(auth.list, "authenticated");
        assert_eq!(auth.create, "service_only");
        assert_eq!(auth.delete, "admin_only");
    }

    #[test]
    fn test_link_definition_without_auth() {
        let yaml = r#"
            link_type: payment
            source_type: invoice
            target_type: payment
            forward_route_name: payments
            reverse_route_name: invoice
        "#;

        let def: LinkDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.link_type, "payment");
        assert!(def.auth.is_none());
    }
}
