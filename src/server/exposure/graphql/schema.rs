//! GraphQL schema generation
//!
//! This module generates the GraphQL schema from the ServerHost configuration.
//! It automatically exposes all entities defined by the client in their modules.
//!
//! The schema uses a dynamic approach where entity types are exposed with their
//! actual names (order, invoice, payment) rather than a generic "entity" type.

#[cfg(feature = "graphql")]
use async_graphql::*;
#[cfg(feature = "graphql")]
use serde_json::Value;
#[cfg(feature = "graphql")]
use std::sync::Arc;
#[cfg(feature = "graphql")]
use uuid::Uuid;

#[cfg(feature = "graphql")]
/// Generic entity type for GraphQL (dynamically exposed)
///
/// This type represents any entity in the system and includes
/// methods to access related entities through links.
#[allow(dead_code)]
pub struct Entity {
    /// Entity ID
    pub id: String,

    /// Entity type
    pub entity_type: String,

    /// Entity name
    pub name: String,

    /// Created timestamp
    pub created_at: String,

    /// Updated timestamp
    pub updated_at: String,

    /// Deleted timestamp (if soft deleted)
    pub deleted_at: Option<String>,

    /// Entity status
    pub status: String,

    /// All entity data as JSON (includes custom fields)
    pub data: Value,

    /// Host reference for resolving relations (not exposed in GraphQL)
    pub host: Option<Arc<crate::server::host::ServerHost>>,
}

#[cfg(feature = "graphql")]
#[Object]
impl Entity {
    /// Entity ID
    async fn id(&self) -> &str {
        &self.id
    }

    /// Entity type
    #[graphql(name = "type")]
    async fn entity_type(&self) -> &str {
        &self.entity_type
    }

    /// Entity name
    async fn name(&self) -> &str {
        &self.name
    }

    /// Created timestamp
    #[graphql(name = "createdAt")]
    async fn created_at(&self) -> &str {
        &self.created_at
    }

    /// Updated timestamp
    #[graphql(name = "updatedAt")]
    async fn updated_at(&self) -> &str {
        &self.updated_at
    }

    /// Deleted timestamp (if soft deleted)
    #[graphql(name = "deletedAt")]
    async fn deleted_at(&self) -> Option<&str> {
        self.deleted_at.as_deref()
    }

    /// Entity status
    async fn status(&self) -> &str {
        &self.status
    }

    /// All entity data as JSON (includes custom fields)
    async fn data(&self) -> &Value {
        &self.data
    }

    /// Get linked invoices (for orders)
    async fn invoices(&self) -> Result<Vec<Entity>> {
        self.get_linked_entities("invoices", "invoice").await
    }

    /// Get linked payments (for invoices)
    async fn payments(&self) -> Result<Vec<Entity>> {
        self.get_linked_entities("payments", "payment").await
    }

    /// Get linked order (for invoices)
    async fn order(&self) -> Result<Option<Entity>> {
        let links = self.get_linked_entities("order", "order").await?;
        Ok(links.into_iter().next())
    }

    /// Get linked invoice (for payments)
    async fn invoice(&self) -> Result<Option<Entity>> {
        let links = self.get_linked_entities("invoice", "invoice").await?;
        Ok(links.into_iter().next())
    }

    /// Generic method to get all links of a specific type
    async fn links(&self, link_type: Option<String>) -> Result<Vec<Link>> {
        if let Some(host) = &self.host {
            let uuid = Uuid::parse_str(&self.id)
                .map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

            let link_type_str = link_type.as_deref();

            match host
                .link_service
                .find_by_source(&uuid, link_type_str, None)
                .await
            {
                Ok(links) => Ok(links
                    .into_iter()
                    .map(|link_entity| Link {
                        id: link_entity.id.to_string(),
                        source_id: link_entity.source_id.to_string(),
                        target_id: link_entity.target_id.to_string(),
                        link_type: link_entity.link_type.clone(),
                        metadata: link_entity
                            .metadata
                            .clone()
                            .unwrap_or(serde_json::json!({})),
                        created_at: link_entity.created_at.to_rfc3339(),
                        target: None,
                        source: None,
                    })
                    .collect()),
                Err(_) => Ok(vec![]),
            }
        } else {
            Ok(vec![])
        }
    }
}

#[cfg(feature = "graphql")]
impl Entity {
    /// Helper method to get linked entities
    #[allow(dead_code)]
    async fn get_linked_entities(&self, link_type: &str, target_type: &str) -> Result<Vec<Entity>> {
        if let Some(host) = &self.host {
            let uuid = Uuid::parse_str(&self.id)
                .map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

            match host
                .link_service
                .find_by_source(&uuid, Some(link_type), Some(target_type))
                .await
            {
                Ok(links) => {
                    let mut entities = Vec::new();

                    if let Some(fetcher) = host.entity_fetchers.get(target_type) {
                        for link in links {
                            if let Ok(value) = fetcher.fetch_as_json(&link.target_id).await {
                                entities.push(Entity {
                                    id: value["id"].as_str().unwrap_or("").to_string(),
                                    entity_type: value["type"].as_str().unwrap_or("").to_string(),
                                    name: value["name"].as_str().unwrap_or("").to_string(),
                                    created_at: value["created_at"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string(),
                                    updated_at: value["updated_at"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string(),
                                    deleted_at: value["deleted_at"].as_str().map(String::from),
                                    status: value["status"].as_str().unwrap_or("").to_string(),
                                    data: value,
                                    host: Some(host.clone()),
                                });
                            }
                        }
                    }

                    Ok(entities)
                }
                Err(_) => Ok(vec![]),
            }
        } else {
            Ok(vec![])
        }
    }
}

#[cfg(feature = "graphql")]
/// Link between entities
#[derive(SimpleObject)]
#[allow(dead_code)]
pub struct Link {
    /// Link ID
    pub id: String,

    /// Source entity ID
    #[graphql(name = "sourceId")]
    pub source_id: String,

    /// Target entity ID
    #[graphql(name = "targetId")]
    pub target_id: String,

    /// Link type
    #[graphql(name = "linkType")]
    pub link_type: String,

    /// Link metadata
    pub metadata: Value,

    /// Created timestamp
    #[graphql(name = "createdAt")]
    pub created_at: String,

    /// Target entity (enriched)
    pub target: Option<Entity>,

    /// Source entity (enriched)
    pub source: Option<Entity>,
}

#[cfg(feature = "graphql")]
#[allow(dead_code)]
pub struct QueryRoot {
    pub(super) host: Arc<crate::server::host::ServerHost>,
}

#[cfg(feature = "graphql")]
#[Object]
impl QueryRoot {
    /// Get entity by ID and type
    ///
    /// Returns the entity with all its custom fields in the `data` field
    async fn entity(&self, id: String, entity_type: String) -> Result<Option<Entity>> {
        if let Some(fetcher) = self.host.entity_fetchers.get(&entity_type) {
            let uuid =
                Uuid::parse_str(&id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

            match fetcher.fetch_as_json(&uuid).await {
                Ok(value) => Ok(Some(Entity {
                    id: value["id"].as_str().unwrap_or("").to_string(),
                    entity_type: value["type"].as_str().unwrap_or("").to_string(),
                    name: value["name"].as_str().unwrap_or("").to_string(),
                    created_at: value["created_at"].as_str().unwrap_or("").to_string(),
                    updated_at: value["updated_at"].as_str().unwrap_or("").to_string(),
                    deleted_at: value["deleted_at"].as_str().map(String::from),
                    status: value["status"].as_str().unwrap_or("").to_string(),
                    data: value,
                    host: Some(self.host.clone()),
                })),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// List all entity types registered in the system
    async fn entity_types(&self) -> Vec<String> {
        self.host
            .entity_types()
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Get an order by ID
    #[graphql(name = "order")]
    async fn get_order(&self, id: String) -> Result<Option<Entity>> {
        self.get_entity_by_type(id, "order".to_string()).await
    }

    /// Get an invoice by ID
    #[graphql(name = "invoice")]
    async fn get_invoice(&self, id: String) -> Result<Option<Entity>> {
        self.get_entity_by_type(id, "invoice".to_string()).await
    }

    /// Get a payment by ID
    #[graphql(name = "payment")]
    async fn get_payment(&self, id: String) -> Result<Option<Entity>> {
        self.get_entity_by_type(id, "payment".to_string()).await
    }

    /// List all orders
    #[graphql(name = "orders")]
    async fn list_orders(&self) -> Result<Vec<Entity>> {
        self.list_entities("order").await
    }

    /// List all invoices
    #[graphql(name = "invoices")]
    async fn list_invoices(&self) -> Result<Vec<Entity>> {
        self.list_entities("invoice").await
    }

    /// List all payments
    #[graphql(name = "payments")]
    async fn list_payments(&self) -> Result<Vec<Entity>> {
        self.list_entities("payment").await
    }

    /// Get link by ID
    async fn link(&self, id: String) -> Result<Option<Link>> {
        let uuid = Uuid::parse_str(&id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        match self.host.link_service.get(&uuid).await {
            Ok(Some(link_entity)) => Ok(Some(Link {
                id: link_entity.id.to_string(),
                source_id: link_entity.source_id.to_string(),
                target_id: link_entity.target_id.to_string(),
                link_type: link_entity.link_type.clone(),
                metadata: link_entity
                    .metadata
                    .clone()
                    .unwrap_or(serde_json::json!({})),
                created_at: link_entity.created_at.to_rfc3339(),
                target: None,
                source: None,
            })),
            _ => Ok(None),
        }
    }

    /// Get links for an entity
    async fn entity_links(
        &self,
        entity_id: String,
        link_type: Option<String>,
        target_type: Option<String>,
    ) -> Result<Vec<Link>> {
        let uuid =
            Uuid::parse_str(&entity_id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        let link_type_str = link_type.as_deref();
        let target_type_str = target_type.as_deref();

        match self
            .host
            .link_service
            .find_by_source(&uuid, link_type_str, target_type_str)
            .await
        {
            Ok(links) => Ok(links
                .into_iter()
                .map(|link_entity| Link {
                    id: link_entity.id.to_string(),
                    source_id: link_entity.source_id.to_string(),
                    target_id: link_entity.target_id.to_string(),
                    link_type: link_entity.link_type.clone(),
                    metadata: link_entity
                        .metadata
                        .clone()
                        .unwrap_or(serde_json::json!({})),
                    created_at: link_entity.created_at.to_rfc3339(),
                    target: None,
                    source: None,
                })
                .collect()),
            _ => Ok(vec![]),
        }
    }
}

#[cfg(feature = "graphql")]
impl QueryRoot {
    /// Helper to get entity by type
    #[allow(dead_code)]
    async fn get_entity_by_type(&self, id: String, entity_type: String) -> Result<Option<Entity>> {
        if let Some(fetcher) = self.host.entity_fetchers.get(&entity_type) {
            let uuid =
                Uuid::parse_str(&id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

            match fetcher.fetch_as_json(&uuid).await {
                Ok(value) => Ok(Some(Entity {
                    id: value["id"].as_str().unwrap_or("").to_string(),
                    entity_type: value["type"].as_str().unwrap_or("").to_string(),
                    name: value["name"].as_str().unwrap_or("").to_string(),
                    created_at: value["created_at"].as_str().unwrap_or("").to_string(),
                    updated_at: value["updated_at"].as_str().unwrap_or("").to_string(),
                    deleted_at: value["deleted_at"].as_str().map(String::from),
                    status: value["status"].as_str().unwrap_or("").to_string(),
                    data: value,
                    host: Some(self.host.clone()),
                })),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Helper to list entities of a given type
    async fn list_entities(&self, _entity_type: &str) -> Result<Vec<Entity>> {
        // For now, return empty vec as we need pagination support
        // This will be implemented when we add proper list support to EntityFetcher
        Ok(vec![])
    }
}

#[cfg(feature = "graphql")]
#[allow(dead_code)]
pub struct MutationRoot {
    pub(super) host: Arc<crate::server::host::ServerHost>,
}

#[cfg(feature = "graphql")]
#[Object]
impl MutationRoot {
    /// Create a link between entities
    #[allow(dead_code)]
    async fn create_link(
        &self,
        source_id: String,
        target_id: String,
        link_type: String,
        metadata: Option<Value>,
    ) -> Result<Link> {
        let source_uuid = Uuid::parse_str(&source_id)
            .map_err(|e| Error::new(format!("Invalid source UUID: {}", e)))?;
        let target_uuid = Uuid::parse_str(&target_id)
            .map_err(|e| Error::new(format!("Invalid target UUID: {}", e)))?;

        use crate::core::link::LinkEntity as CoreLinkEntity;

        let link_entity = CoreLinkEntity::new(
            link_type,
            source_uuid,
            target_uuid,
            Some(metadata.unwrap_or(serde_json::json!({}))),
        );

        match self.host.link_service.create(link_entity).await {
            Ok(created) => Ok(Link {
                id: created.id.to_string(),
                source_id: created.source_id.to_string(),
                target_id: created.target_id.to_string(),
                link_type: created.link_type.clone(),
                metadata: created.metadata.clone().unwrap_or(serde_json::json!({})),
                created_at: created.created_at.to_rfc3339(),
                target: None,
                source: None,
            }),
            Err(e) => Err(Error::new(format!("Failed to create link: {}", e))),
        }
    }

    /// Delete a link
    #[allow(dead_code)]
    async fn delete_link(&self, id: String) -> Result<bool> {
        let uuid = Uuid::parse_str(&id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        match self.host.link_service.delete(&uuid).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(feature = "graphql")]
#[allow(dead_code)]
pub type SubscriptionRoot = EmptySubscription;

#[cfg(feature = "graphql")]
/// Build the GraphQL schema from the host
#[allow(dead_code)]
pub fn build_schema(
    host: Arc<crate::server::host::ServerHost>,
) -> Schema<QueryRoot, MutationRoot, EmptySubscription> {
    let query = QueryRoot { host: host.clone() };
    let mutation = MutationRoot { host: host.clone() };

    Schema::build(query, mutation, EmptySubscription).finish()
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::link::LinkDefinition;
    use crate::core::EntityFetcher;
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use crate::server::host::ServerHost;
    use crate::storage::in_memory::InMemoryLinkService;
    use async_trait::async_trait;
    use axum::Router;
    use serde_json::{json, Value as JsonVal};
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Mock infrastructure
    // -----------------------------------------------------------------------

    struct MockFetcher {
        entities: std::sync::Mutex<HashMap<Uuid, JsonVal>>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                entities: std::sync::Mutex::new(HashMap::new()),
            }
        }

        fn with_entity(self, id: Uuid, entity: JsonVal) -> Self {
            self.entities
                .lock()
                .expect("lock poisoned")
                .insert(id, entity);
            self
        }
    }

    #[async_trait]
    impl EntityFetcher for MockFetcher {
        async fn fetch_as_json(&self, entity_id: &Uuid) -> anyhow::Result<JsonVal> {
            let entities = self.entities.lock().expect("lock poisoned");
            entities
                .get(entity_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Entity not found: {}", entity_id))
        }
    }

    struct StubDescriptor {
        entity_type: String,
        plural: String,
    }

    impl StubDescriptor {
        fn new(singular: &str, plural: &str) -> Self {
            Self {
                entity_type: singular.to_string(),
                plural: plural.to_string(),
            }
        }
    }

    impl EntityDescriptor for StubDescriptor {
        fn entity_type(&self) -> &str {
            &self.entity_type
        }
        fn plural(&self) -> &str {
            &self.plural
        }
        fn build_routes(&self) -> Router {
            Router::new()
        }
    }

    fn build_test_host(
        fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    ) -> Arc<ServerHost> {
        let link_service = Arc::new(InMemoryLinkService::new());
        let config = LinksConfig {
            entities: vec![
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
            links: vec![LinkDefinition {
                link_type: "has_invoice".to_string(),
                source_type: "order".to_string(),
                target_type: "invoice".to_string(),
                forward_route_name: "invoices".to_string(),
                reverse_route_name: "order".to_string(),
                description: None,
                required_fields: None,
                auth: None,
            }],
            validation_rules: None,
        };

        let mut registry = EntityRegistry::new();
        registry.register(Box::new(StubDescriptor::new("order", "orders")));
        registry.register(Box::new(StubDescriptor::new("invoice", "invoices")));

        Arc::new(
            ServerHost::from_builder_components(
                link_service,
                config,
                registry,
                fetchers,
                HashMap::new(),
            )
            .expect("should build test host"),
        )
    }

    fn build_host_with_fetcher(
        entity_type: &str,
        fetcher: Arc<dyn EntityFetcher>,
    ) -> Arc<ServerHost> {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(entity_type.to_string(), fetcher);
        build_test_host(fetchers)
    }

    // -----------------------------------------------------------------------
    // build_schema smoke test
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_schema_does_not_panic() {
        let host = build_test_host(HashMap::new());
        let _schema = build_schema(host);
    }

    // -----------------------------------------------------------------------
    // Entity struct fields (direct access, not #[Object] resolvers)
    // -----------------------------------------------------------------------

    #[test]
    fn test_entity_struct_fields() {
        let entity = Entity {
            id: "abc-123".to_string(),
            entity_type: "order".to_string(),
            name: "Test Order".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-02T00:00:00Z".to_string(),
            deleted_at: Some("2024-01-03T00:00:00Z".to_string()),
            status: "active".to_string(),
            data: json!({"custom": "field"}),
            host: None,
        };

        assert_eq!(entity.id, "abc-123");
        assert_eq!(entity.entity_type, "order");
        assert_eq!(entity.name, "Test Order");
        assert_eq!(entity.created_at, "2024-01-01T00:00:00Z");
        assert_eq!(entity.updated_at, "2024-01-02T00:00:00Z");
        assert_eq!(entity.deleted_at.as_deref(), Some("2024-01-03T00:00:00Z"));
        assert_eq!(entity.status, "active");
        assert_eq!(entity.data, json!({"custom": "field"}));
    }

    #[test]
    fn test_entity_deleted_at_none() {
        let entity = Entity {
            id: "x".to_string(),
            entity_type: "order".to_string(),
            name: "".to_string(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
            deleted_at: None,
            status: "".to_string(),
            data: json!(null),
            host: None,
        };

        assert!(entity.deleted_at.is_none());
    }

    // -----------------------------------------------------------------------
    // Entity: get_linked_entities without host returns empty
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_linked_entities_no_host_returns_empty() {
        let entity = Entity {
            id: Uuid::new_v4().to_string(),
            entity_type: "order".to_string(),
            name: "".to_string(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
            deleted_at: None,
            status: "".to_string(),
            data: json!({}),
            host: None,
        };

        let result = entity
            .get_linked_entities("invoices", "invoice")
            .await
            .expect("should not error");
        assert!(result.is_empty(), "no host means no linked entities");
    }

    // -----------------------------------------------------------------------
    // Entity: get_linked_entities with host but no links
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_linked_entities_host_empty_store() {
        let entity_id = Uuid::new_v4();
        let host = build_test_host(HashMap::new());

        let entity = Entity {
            id: entity_id.to_string(),
            entity_type: "order".to_string(),
            name: "".to_string(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
            deleted_at: None,
            status: "".to_string(),
            data: json!({}),
            host: Some(host),
        };

        let result = entity
            .get_linked_entities("invoices", "invoice")
            .await
            .expect("should not error");
        assert!(result.is_empty(), "empty store means no linked entities");
    }

    // -----------------------------------------------------------------------
    // Entity: get_linked_entities with invalid UUID returns error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_linked_entities_invalid_uuid() {
        let host = build_test_host(HashMap::new());

        let entity = Entity {
            id: "not-a-uuid".to_string(),
            entity_type: "order".to_string(),
            name: "".to_string(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
            deleted_at: None,
            status: "".to_string(),
            data: json!({}),
            host: Some(host),
        };

        let result = entity.get_linked_entities("invoices", "invoice").await;
        assert!(result.is_err(), "invalid UUID should produce an error");
    }

    // -----------------------------------------------------------------------
    // Entity: get_linked_entities with fetcher resolves targets
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_linked_entities_resolves_targets() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let invoice_json = json!({
            "id": target_id.to_string(),
            "type": "invoice",
            "name": "Invoice #1",
            "created_at": "2024-01-01",
            "updated_at": "2024-01-02",
            "deleted_at": null,
            "status": "paid"
        });

        let host = build_host_with_fetcher(
            "invoice",
            Arc::new(MockFetcher::new().with_entity(target_id, invoice_json)),
        );

        // Create a link in the store
        let link_entity = crate::core::link::LinkEntity::new(
            "has_invoice",
            source_id,
            target_id,
            None,
        );
        host.link_service
            .create(link_entity)
            .await
            .expect("should create link");

        let entity = Entity {
            id: source_id.to_string(),
            entity_type: "order".to_string(),
            name: "".to_string(),
            created_at: "".to_string(),
            updated_at: "".to_string(),
            deleted_at: None,
            status: "".to_string(),
            data: json!({}),
            host: Some(host),
        };

        let result = entity
            .get_linked_entities("has_invoice", "invoice")
            .await
            .expect("should not error");
        assert_eq!(result.len(), 1, "should resolve one linked entity");
        assert_eq!(result[0].name, "Invoice #1");
        assert_eq!(result[0].status, "paid");
    }

    // -----------------------------------------------------------------------
    // Link struct fields
    // -----------------------------------------------------------------------

    #[test]
    fn test_link_struct_fields() {
        let link = Link {
            id: "link-1".to_string(),
            source_id: "src-1".to_string(),
            target_id: "tgt-1".to_string(),
            link_type: "has_invoice".to_string(),
            metadata: json!({"key": "val"}),
            created_at: "2024-01-01".to_string(),
            target: None,
            source: None,
        };

        assert_eq!(link.id, "link-1");
        assert_eq!(link.source_id, "src-1");
        assert_eq!(link.target_id, "tgt-1");
        assert_eq!(link.link_type, "has_invoice");
        assert!(link.target.is_none());
        assert!(link.source.is_none());
    }

    // -----------------------------------------------------------------------
    // Schema: entity_types query via schema execution
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_entity_types() {
        let host = build_test_host(HashMap::new());
        let schema = build_schema(host);

        let result = schema
            .execute("{ entityTypes }")
            .await;

        assert!(result.errors.is_empty(), "should have no errors: {:?}", result.errors);
        let data = result.data.into_json().expect("should serialize");
        let types = data["entityTypes"]
            .as_array()
            .expect("should be array");
        let type_strs: Vec<&str> = types
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();
        assert!(type_strs.contains(&"order"), "should have order");
        assert!(type_strs.contains(&"invoice"), "should have invoice");
    }

    // -----------------------------------------------------------------------
    // Schema: entity query with fetcher
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_entity_found() {
        let order_id = Uuid::new_v4();
        let order_json = json!({
            "id": order_id.to_string(),
            "type": "order",
            "name": "Order #1",
            "created_at": "2024-01-01",
            "updated_at": "2024-01-02",
            "deleted_at": null,
            "status": "active"
        });

        let host = build_host_with_fetcher(
            "order",
            Arc::new(MockFetcher::new().with_entity(order_id, order_json)),
        );
        let schema = build_schema(host);

        let query = format!(
            r#"{{ entity(id: "{}", entityType: "order") {{ id name status }} }}"#,
            order_id
        );
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        let entity = &data["entity"];
        assert_eq!(entity["name"], "Order #1");
        assert_eq!(entity["status"], "active");
    }

    // -----------------------------------------------------------------------
    // Schema: entity query unknown type returns null
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_entity_unknown_type_returns_null() {
        let host = build_test_host(HashMap::new());
        let schema = build_schema(host);

        let id = Uuid::new_v4();
        let query = format!(
            r#"{{ entity(id: "{}", entityType: "widget") {{ id }} }}"#,
            id
        );
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        assert!(data["entity"].is_null(), "unknown type should return null");
    }

    // -----------------------------------------------------------------------
    // Schema: entity query - entity not found returns null
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_entity_not_found_returns_null() {
        let host = build_host_with_fetcher(
            "order",
            Arc::new(MockFetcher::new()),
        );
        let schema = build_schema(host);

        let id = Uuid::new_v4();
        let query = format!(
            r#"{{ entity(id: "{}", entityType: "order") {{ id }} }}"#,
            id
        );
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        assert!(data["entity"].is_null(), "not found should return null");
    }

    // -----------------------------------------------------------------------
    // Schema: entity query with invalid UUID returns error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_entity_invalid_uuid() {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        let host = build_test_host(fetchers);
        let schema = build_schema(host);

        let query = r#"{ entity(id: "not-a-uuid", entityType: "order") { id } }"#;
        let result = schema.execute(query).await;
        assert!(
            !result.errors.is_empty(),
            "invalid UUID should produce errors"
        );
    }

    // -----------------------------------------------------------------------
    // Schema: entityLinks query
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_entity_links() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let host = build_test_host(HashMap::new());

        // Create a link in the store
        let link_entity = crate::core::link::LinkEntity::new(
            "has_invoice",
            source_id,
            target_id,
            Some(json!({"amount": 100})),
        );
        host.link_service
            .create(link_entity)
            .await
            .expect("should create link");

        let schema = build_schema(host);

        let query = format!(
            r#"{{ entityLinks(entityId: "{}") {{ linkType sourceId targetId }} }}"#,
            source_id
        );
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        let links = data["entityLinks"].as_array().expect("should be array");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0]["linkType"], "has_invoice");
        assert_eq!(links[0]["sourceId"], source_id.to_string());
    }

    // -----------------------------------------------------------------------
    // Schema: createLink mutation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mutation_create_link() {
        let host = build_test_host(HashMap::new());
        let schema = build_schema(host);

        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let query = format!(
            r#"mutation {{ createLink(sourceId: "{}", targetId: "{}", linkType: "has_invoice") {{ id sourceId targetId linkType }} }}"#,
            source_id, target_id
        );

        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        let link = &data["createLink"];
        assert_eq!(link["sourceId"], source_id.to_string());
        assert_eq!(link["targetId"], target_id.to_string());
        assert_eq!(link["linkType"], "has_invoice");
        assert!(link["id"].as_str().is_some(), "link should have an id");
    }

    // -----------------------------------------------------------------------
    // Schema: createLink with invalid UUID
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mutation_create_link_invalid_source_uuid() {
        let host = build_test_host(HashMap::new());
        let schema = build_schema(host);

        let query = format!(
            r#"mutation {{ createLink(sourceId: "not-a-uuid", targetId: "{}", linkType: "x") {{ id }} }}"#,
            Uuid::new_v4()
        );

        let result = schema.execute(&query).await;
        assert!(
            !result.errors.is_empty(),
            "invalid UUID should produce errors"
        );
    }

    // -----------------------------------------------------------------------
    // Schema: deleteLink mutation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mutation_delete_link() {
        let host = build_test_host(HashMap::new());

        // First create a link
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let link_entity = crate::core::link::LinkEntity::new(
            "has_invoice",
            source_id,
            target_id,
            None,
        );
        let created = host
            .link_service
            .create(link_entity)
            .await
            .expect("should create link");

        let schema = build_schema(host);

        let query = format!(
            r#"mutation {{ deleteLink(id: "{}") }}"#,
            created.id
        );
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        assert_eq!(data["deleteLink"], true);
    }

    // -----------------------------------------------------------------------
    // Schema: list_entities returns empty (stub implementation)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_list_orders_returns_empty() {
        let host = build_test_host(HashMap::new());
        let schema = build_schema(host);

        let result = schema.execute("{ orders { id } }").await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        let orders = data["orders"].as_array().expect("should be array");
        assert!(orders.is_empty(), "list_entities returns empty for now");
    }

    // -----------------------------------------------------------------------
    // Schema: link query by ID
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_link_by_id() {
        let host = build_test_host(HashMap::new());

        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let link_entity = crate::core::link::LinkEntity::new(
            "has_invoice",
            source_id,
            target_id,
            None,
        );
        let created = host
            .link_service
            .create(link_entity)
            .await
            .expect("should create link");

        let schema = build_schema(host);

        let query = format!(
            r#"{{ link(id: "{}") {{ id linkType sourceId targetId }} }}"#,
            created.id
        );
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        let link = &data["link"];
        assert_eq!(link["linkType"], "has_invoice");
        assert_eq!(link["sourceId"], source_id.to_string());
    }

    // -----------------------------------------------------------------------
    // Schema: link query not found returns null
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_root_link_not_found() {
        let host = build_test_host(HashMap::new());
        let schema = build_schema(host);

        let id = Uuid::new_v4();
        let query = format!(r#"{{ link(id: "{}") {{ id }} }}"#, id);
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        assert!(data["link"].is_null(), "non-existent link should return null");
    }
}
