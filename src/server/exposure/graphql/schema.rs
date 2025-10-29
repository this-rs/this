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
