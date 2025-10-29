//! Dynamic GraphQL schema using async-graphql with runtime type generation
//!
//! This module creates a GraphQL schema that dynamically exposes entity types
//! based on the registered entities in the ServerHost, using their actual field
//! definitions from the entity structs.

#[cfg(feature = "graphql")]
use async_graphql::*;
#[cfg(feature = "graphql")]
use serde_json::Value;
#[cfg(feature = "graphql")]
use std::sync::Arc;
#[cfg(feature = "graphql")]
use uuid::Uuid;

#[cfg(feature = "graphql")]
use crate::core::link::LinkEntity;
#[cfg(feature = "graphql")]
use crate::server::host::ServerHost;

/// Wrapper type for JSON values to satisfy orphan rules
#[cfg(feature = "graphql")]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct JsonValue(pub Value);

/// JSON scalar type for dynamic fields
#[cfg(feature = "graphql")]
#[Scalar]
impl ScalarType for JsonValue {
    fn parse(value: async_graphql::Value) -> InputValueResult<Self> {
        fn parse_value(value: async_graphql::Value) -> Result<Value, String> {
            match value {
                async_graphql::Value::Null => Ok(Value::Null),
                async_graphql::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Ok(Value::Number(serde_json::Number::from(i)))
                    } else if let Some(f) = n.as_f64() {
                        Ok(serde_json::Number::from_f64(f)
                            .map(Value::Number)
                            .unwrap_or(Value::Null))
                    } else {
                        Ok(Value::Null)
                    }
                }
                async_graphql::Value::String(s) => Ok(Value::String(s)),
                async_graphql::Value::Boolean(b) => Ok(Value::Bool(b)),
                async_graphql::Value::List(list) => {
                    let mut values = Vec::new();
                    for item in list {
                        values.push(parse_value(item)?);
                    }
                    Ok(Value::Array(values))
                }
                async_graphql::Value::Object(obj) => {
                    let mut map = serde_json::Map::new();
                    for (k, v) in obj {
                        map.insert(k.to_string(), parse_value(v)?);
                    }
                    Ok(Value::Object(map))
                }
                _ => Err("Invalid JSON value".to_string()),
            }
        }
        parse_value(value)
            .map(JsonValue)
            .map_err(InputValueError::custom)
    }

    fn to_value(&self) -> async_graphql::Value {
        fn to_gql_value(value: &Value) -> async_graphql::Value {
            match value {
                Value::Null => async_graphql::Value::Null,
                Value::Bool(b) => async_graphql::Value::Boolean(*b),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        async_graphql::Value::Number(i.into())
                    } else if let Some(f) = n.as_f64() {
                        async_graphql::Value::Number(
                            async_graphql::Number::from_f64(f).unwrap_or(0.into()),
                        )
                    } else {
                        async_graphql::Value::Null
                    }
                }
                Value::String(s) => async_graphql::Value::String(s.clone()),
                Value::Array(arr) => {
                    async_graphql::Value::List(arr.iter().map(to_gql_value).collect())
                }
                Value::Object(obj) => {
                    let mut map = async_graphql::indexmap::IndexMap::new();
                    for (k, v) in obj {
                        map.insert(async_graphql::Name::new(k), to_gql_value(v));
                    }
                    async_graphql::Value::Object(map)
                }
            }
        }
        to_gql_value(&self.0)
    }
}

/// Dynamic Query Root that creates resolvers for all registered entities
#[cfg(feature = "graphql")]
#[allow(dead_code)]
pub struct DynamicQueryRoot {
    pub host: Arc<ServerHost>,
}

#[cfg(feature = "graphql")]
#[Object]
impl DynamicQueryRoot {
    /// Get a list of all registered entity types
    async fn entity_types(&self) -> Vec<String> {
        self.host
            .entity_types()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Get an entity by ID and type - returns the full JSON object
    async fn entity(
        &self,
        id: ID,
        entity_type: String,
    ) -> async_graphql::Result<Option<JsonValue>> {
        let uuid = Uuid::parse_str(&id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        if let Some(fetcher) = self.host.entity_fetchers.get(&entity_type) {
            match fetcher.fetch_as_json(&uuid).await {
                Ok(json) => Ok(Some(JsonValue(json))),
                Err(_) => Ok(None),
            }
        } else {
            Err(Error::new(format!("Unknown entity type: {}", entity_type)))
        }
    }

    /// List entities of a specific type - returns full JSON objects
    async fn entities(
        &self,
        entity_type: String,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> async_graphql::Result<Vec<JsonValue>> {
        if let Some(fetcher) = self.host.entity_fetchers.get(&entity_type) {
            fetcher
                .list_as_json(limit, offset)
                .await
                .map(|list| list.into_iter().map(JsonValue).collect())
                .map_err(|e| Error::new(format!("Failed to list entities: {}", e)))
        } else {
            Err(Error::new(format!("Unknown entity type: {}", entity_type)))
        }
    }

    /// Get links for an entity (as source)
    async fn entity_links(&self, entity_id: ID) -> async_graphql::Result<Vec<JsonValue>> {
        let uuid =
            Uuid::parse_str(&entity_id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        let links = self
            .host
            .link_service
            .find_by_source(&uuid, None, None)
            .await
            .map_err(|e| Error::new(format!("Failed to get links: {}", e)))?;

        // Convert links to JSON
        let json_links: Vec<JsonValue> = links
            .into_iter()
            .map(|link| {
                JsonValue(serde_json::json!({
                    "id": link.id.to_string(),
                    "sourceId": link.source_id.to_string(),
                    "targetId": link.target_id.to_string(),
                    "linkType": link.link_type,
                    "metadata": link.metadata,
                    "createdAt": link.created_at.to_rfc3339(),
                }))
            })
            .collect();

        Ok(json_links)
    }
}

/// Dynamic Mutation Root for CRUD operations
#[cfg(feature = "graphql")]
#[allow(dead_code)]
pub struct DynamicMutationRoot {
    pub host: Arc<ServerHost>,
}

#[cfg(feature = "graphql")]
#[Object]
impl DynamicMutationRoot {
    /// Create a new entity of the specified type
    async fn create_entity(
        &self,
        entity_type: String,
        data: JsonValue,
    ) -> async_graphql::Result<JsonValue> {
        if let Some(creator) = self.host.entity_creators.get(&entity_type) {
            creator
                .create_from_json(data.0)
                .await
                .map(JsonValue)
                .map_err(|e| Error::new(format!("Failed to create entity: {}", e)))
        } else {
            Err(Error::new(format!("Unknown entity type: {}", entity_type)))
        }
    }

    /// Update an existing entity
    async fn update_entity(
        &self,
        id: ID,
        entity_type: String,
        data: JsonValue,
    ) -> async_graphql::Result<JsonValue> {
        let uuid = Uuid::parse_str(&id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        if let Some(creator) = self.host.entity_creators.get(&entity_type) {
            creator
                .update_from_json(&uuid, data.0)
                .await
                .map(JsonValue)
                .map_err(|e| Error::new(format!("Failed to update entity: {}", e)))
        } else {
            Err(Error::new(format!("Unknown entity type: {}", entity_type)))
        }
    }

    /// Delete an entity
    async fn delete_entity(&self, id: ID, entity_type: String) -> async_graphql::Result<bool> {
        let uuid = Uuid::parse_str(&id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        if let Some(creator) = self.host.entity_creators.get(&entity_type) {
            creator
                .delete(&uuid)
                .await
                .map(|_| true)
                .map_err(|e| Error::new(format!("Failed to delete entity: {}", e)))
        } else {
            Err(Error::new(format!("Unknown entity type: {}", entity_type)))
        }
    }

    /// Create a link between two entities
    async fn create_link(
        &self,
        source_id: ID,
        target_id: ID,
        link_type: String,
        metadata: Option<JsonValue>,
    ) -> async_graphql::Result<JsonValue> {
        let source_uuid = Uuid::parse_str(&source_id)
            .map_err(|e| Error::new(format!("Invalid source UUID: {}", e)))?;
        let target_uuid = Uuid::parse_str(&target_id)
            .map_err(|e| Error::new(format!("Invalid target UUID: {}", e)))?;

        let metadata_value = metadata.map(|j| j.0);

        let link_entity = LinkEntity::new(link_type, source_uuid, target_uuid, metadata_value);

        let link = self
            .host
            .link_service
            .create(link_entity)
            .await
            .map_err(|e| Error::new(format!("Failed to create link: {}", e)))?;

        Ok(JsonValue(serde_json::json!({
            "id": link.id.to_string(),
            "sourceId": link.source_id.to_string(),
            "targetId": link.target_id.to_string(),
            "linkType": link.link_type,
            "metadata": link.metadata,
            "createdAt": link.created_at.to_rfc3339(),
        })))
    }

    /// Delete a link
    async fn delete_link(&self, link_id: ID) -> async_graphql::Result<bool> {
        let uuid =
            Uuid::parse_str(&link_id).map_err(|e| Error::new(format!("Invalid UUID: {}", e)))?;

        self.host
            .link_service
            .delete(&uuid)
            .await
            .map(|_| true)
            .map_err(|e| Error::new(format!("Failed to delete link: {}", e)))
    }
}

/// Build the dynamic GraphQL schema
#[cfg(feature = "graphql")]
#[allow(dead_code)]
pub fn build_dynamic_schema(
    host: Arc<ServerHost>,
) -> Schema<DynamicQueryRoot, DynamicMutationRoot, EmptySubscription> {
    Schema::build(
        DynamicQueryRoot { host: host.clone() },
        DynamicMutationRoot { host },
        EmptySubscription,
    )
    .finish()
}
