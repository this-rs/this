//! Link system for managing relationships between entities

use crate::core::pluralize::Pluralizer;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A polymorphic link between two entities
///
/// Links follow the Entity model with base fields (id, type, timestamps, status)
/// plus relationship-specific fields (source_id, target_id, link_type).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkEntity {
    /// Unique identifier for this link
    pub id: Uuid,

    /// Entity type (always "link" for base links)
    #[serde(rename = "type")]
    pub entity_type: String,

    /// When this link was created
    pub created_at: DateTime<Utc>,

    /// When this link was last updated
    pub updated_at: DateTime<Utc>,

    /// When this link was soft-deleted (if applicable)
    pub deleted_at: Option<DateTime<Utc>>,

    /// Status of the link
    pub status: String,

    /// Optional tenant ID for multi-tenant isolation
    ///
    /// When set, this link belongs to a specific tenant.
    /// When None, the link is treated as system-wide or single-tenant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<Uuid>,

    /// The type of relationship (e.g., "owner", "driver", "worker")
    pub link_type: String,

    /// The ID of the source entity
    pub source_id: Uuid,

    /// The ID of the target entity
    pub target_id: Uuid,

    /// Optional metadata for the relationship
    pub metadata: Option<serde_json::Value>,
}

impl LinkEntity {
    /// Create a new link without tenant context
    ///
    /// For multi-tenant applications, use `new_with_tenant()` instead.
    pub fn new(
        link_type: impl Into<String>,
        source_id: Uuid,
        target_id: Uuid,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            entity_type: "link".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".to_string(),
            tenant_id: None,
            link_type: link_type.into(),
            source_id,
            target_id,
            metadata,
        }
    }

    /// Create a new link with tenant context for multi-tenant applications
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use uuid::Uuid;
    /// use this::core::link::LinkEntity;
    ///
    /// let tenant_id = Uuid::new_v4();
    /// let link = LinkEntity::new_with_tenant(
    ///     tenant_id,
    ///     "has_invoice",
    ///     order_id,
    ///     invoice_id,
    ///     None,
    /// );
    /// assert_eq!(link.tenant_id, Some(tenant_id));
    /// ```
    pub fn new_with_tenant(
        tenant_id: Uuid,
        link_type: impl Into<String>,
        source_id: Uuid,
        target_id: Uuid,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            entity_type: "link".to_string(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".to_string(),
            tenant_id: Some(tenant_id),
            link_type: link_type.into(),
            source_id,
            target_id,
            metadata,
        }
    }

    /// Soft delete this link
    pub fn soft_delete(&mut self) {
        self.deleted_at = Some(Utc::now());
        self.updated_at = Utc::now();
    }

    /// Restore a soft-deleted link
    pub fn restore(&mut self) {
        self.deleted_at = None;
        self.updated_at = Utc::now();
    }

    /// Update the updated_at timestamp
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    /// Check if the link is deleted
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Check if the link is active
    pub fn is_active(&self) -> bool {
        self.status == "active" && !self.is_deleted()
    }
}

/// Authorization configuration for link operations
///
/// This allows fine-grained control over who can perform operations
/// on specific link types, independent of entity-level permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkAuthConfig {
    /// Policy for listing links (GET /{source}/{id}/{route_name})
    #[serde(default = "default_link_auth_policy")]
    pub list: String,

    /// Policy for getting a specific link by ID
    #[serde(default = "default_link_auth_policy")]
    pub get: String,

    /// Policy for creating a link
    #[serde(default = "default_link_auth_policy")]
    pub create: String,

    /// Policy for updating a link
    #[serde(default = "default_link_auth_policy")]
    pub update: String,

    /// Policy for deleting a link
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
            get: default_link_auth_policy(),
            create: default_link_auth_policy(),
            update: default_link_auth_policy(),
            delete: default_link_auth_policy(),
        }
    }
}

/// Configuration for a specific type of link between two entity types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkDefinition {
    /// The type of link (e.g., "owner", "driver")
    pub link_type: String,

    /// The source entity type (e.g., "user")
    pub source_type: String,

    /// The target entity type (e.g., "car")
    pub target_type: String,

    /// Route name when navigating from source to target
    pub forward_route_name: String,

    /// Route name when navigating from target to source
    pub reverse_route_name: String,

    /// Optional description of this link type
    pub description: Option<String>,

    /// Optional list of required metadata fields
    pub required_fields: Option<Vec<String>>,

    /// Authorization configuration specific to this link type
    #[serde(default)]
    pub auth: Option<LinkAuthConfig>,
}

impl LinkDefinition {
    /// Generate the default forward route name
    pub fn default_forward_route_name(target_type: &str, link_type: &str) -> String {
        format!(
            "{}-{}",
            Pluralizer::pluralize(target_type),
            Pluralizer::pluralize(link_type)
        )
    }

    /// Generate the default reverse route name
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
    fn test_link_creation() {
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = LinkEntity::new("owner", user_id, car_id, None);

        assert_eq!(link.link_type, "owner");
        assert_eq!(link.source_id, user_id);
        assert_eq!(link.target_id, car_id);
        assert!(link.metadata.is_none());
        assert!(link.tenant_id.is_none());
        assert_eq!(link.status, "active");
        assert!(!link.is_deleted());
        assert!(link.is_active());
    }

    #[test]
    fn test_link_creation_without_tenant() {
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = LinkEntity::new("owner", user_id, car_id, None);

        // Default behavior: no tenant
        assert!(link.tenant_id.is_none());
    }

    #[test]
    fn test_link_creation_with_tenant() {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = LinkEntity::new_with_tenant(tenant_id, "owner", user_id, car_id, None);

        assert_eq!(link.link_type, "owner");
        assert_eq!(link.source_id, user_id);
        assert_eq!(link.target_id, car_id);
        assert_eq!(link.tenant_id, Some(tenant_id));
        assert_eq!(link.status, "active");
    }

    #[test]
    fn test_link_with_tenant_and_metadata() {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let company_id = Uuid::new_v4();

        let metadata = serde_json::json!({
            "role": "Senior Developer",
            "start_date": "2024-01-01"
        });

        let link = LinkEntity::new_with_tenant(
            tenant_id,
            "worker",
            user_id,
            company_id,
            Some(metadata.clone()),
        );

        assert_eq!(link.tenant_id, Some(tenant_id));
        assert_eq!(link.metadata, Some(metadata));
    }

    #[test]
    fn test_link_serialization_without_tenant() {
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);
        let json = serde_json::to_value(&link).unwrap();

        // tenant_id should not appear in JSON when None (skip_serializing_if)
        assert!(json.get("tenant_id").is_none());
    }

    #[test]
    fn test_link_serialization_with_tenant() {
        let tenant_id = Uuid::new_v4();
        let link =
            LinkEntity::new_with_tenant(tenant_id, "owner", Uuid::new_v4(), Uuid::new_v4(), None);
        let json = serde_json::to_value(&link).unwrap();

        // tenant_id should appear in JSON when Some
        assert_eq!(
            json.get("tenant_id").and_then(|v| v.as_str()),
            Some(tenant_id.to_string().as_str())
        );
    }

    #[test]
    fn test_link_with_metadata() {
        let user_id = Uuid::new_v4();
        let company_id = Uuid::new_v4();

        let metadata = serde_json::json!({
            "role": "Senior Developer",
            "start_date": "2024-01-01"
        });

        let link = LinkEntity::new("worker", user_id, company_id, Some(metadata.clone()));

        assert_eq!(link.metadata, Some(metadata));
    }

    #[test]
    fn test_link_soft_delete() {
        let mut link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        assert!(!link.is_deleted());
        assert!(link.is_active());

        link.soft_delete();
        assert!(link.is_deleted());
        assert!(!link.is_active());
    }

    #[test]
    fn test_link_restore() {
        let mut link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        link.soft_delete();
        assert!(link.is_deleted());

        link.restore();
        assert!(!link.is_deleted());
        assert!(link.is_active());
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
        assert_eq!(auth.get, "authenticated");
        assert_eq!(auth.create, "authenticated");
        assert_eq!(auth.update, "authenticated");
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
                get: owner
                create: service_only
                update: owner
                delete: admin_only
        "#;

        let def: LinkDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.link_type, "has_invoice");
        assert_eq!(def.source_type, "order");
        assert_eq!(def.target_type, "invoice");

        let auth = def.auth.unwrap();
        assert_eq!(auth.list, "authenticated");
        assert_eq!(auth.get, "owner");
        assert_eq!(auth.create, "service_only");
        assert_eq!(auth.update, "owner");
        assert_eq!(auth.delete, "admin_only");
    }
}
