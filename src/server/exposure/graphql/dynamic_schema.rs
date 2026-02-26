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

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::link::LinkDefinition;
    use crate::core::{EntityCreator, EntityFetcher};
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use crate::server::host::ServerHost;
    use crate::storage::in_memory::InMemoryLinkService;
    use async_trait::async_trait;
    use axum::Router;
    use serde_json::json;
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Mock infrastructure
    // -----------------------------------------------------------------------

    struct MockFetcher {
        entities: std::sync::Mutex<HashMap<Uuid, Value>>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                entities: std::sync::Mutex::new(HashMap::new()),
            }
        }

        fn with_entity(self, id: Uuid, entity: Value) -> Self {
            self.entities
                .lock()
                .expect("lock poisoned")
                .insert(id, entity);
            self
        }
    }

    #[async_trait]
    impl EntityFetcher for MockFetcher {
        async fn fetch_as_json(&self, entity_id: &Uuid) -> anyhow::Result<Value> {
            let entities = self.entities.lock().expect("lock poisoned");
            entities
                .get(entity_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Entity not found: {}", entity_id))
        }

        async fn list_as_json(
            &self,
            limit: Option<i32>,
            offset: Option<i32>,
        ) -> anyhow::Result<Vec<Value>> {
            let entities = self.entities.lock().expect("lock poisoned");
            let mut all: Vec<Value> = entities.values().cloned().collect();
            let start = offset.unwrap_or(0) as usize;
            if start < all.len() {
                all = all.split_off(start);
            } else {
                all.clear();
            }
            if let Some(lim) = limit {
                all.truncate(lim as usize);
            }
            Ok(all)
        }
    }

    struct MockCreator;

    #[async_trait]
    impl EntityCreator for MockCreator {
        async fn create_from_json(&self, mut data: Value) -> anyhow::Result<Value> {
            let id = Uuid::new_v4();
            if let Some(obj) = data.as_object_mut() {
                obj.insert("id".to_string(), json!(id.to_string()));
            }
            Ok(data)
        }

        async fn update_from_json(
            &self,
            entity_id: &Uuid,
            mut data: Value,
        ) -> anyhow::Result<Value> {
            if let Some(obj) = data.as_object_mut() {
                obj.insert("id".to_string(), json!(entity_id.to_string()));
            }
            Ok(data)
        }

        async fn delete(&self, _entity_id: &Uuid) -> anyhow::Result<()> {
            Ok(())
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
        creators: HashMap<String, Arc<dyn EntityCreator>>,
    ) -> Arc<ServerHost> {
        let link_service = Arc::new(InMemoryLinkService::new());
        let config = LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
        };

        let mut registry = EntityRegistry::new();
        registry.register(Box::new(StubDescriptor::new("order", "orders")));

        Arc::new(
            ServerHost::from_builder_components(link_service, config, registry, fetchers, creators)
                .expect("should build test host"),
        )
    }

    // -----------------------------------------------------------------------
    // JsonValue scalar: parse roundtrip tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_json_value_parse_null() {
        let result = <JsonValue as ScalarType>::parse(async_graphql::Value::Null)
            .expect("should parse null");
        assert_eq!(result.0, Value::Null);
    }

    #[test]
    fn test_json_value_parse_boolean_true() {
        let result = <JsonValue as ScalarType>::parse(async_graphql::Value::Boolean(true))
            .expect("should parse bool");
        assert_eq!(result.0, Value::Bool(true));
    }

    #[test]
    fn test_json_value_parse_boolean_false() {
        let result = <JsonValue as ScalarType>::parse(async_graphql::Value::Boolean(false))
            .expect("should parse bool");
        assert_eq!(result.0, Value::Bool(false));
    }

    #[test]
    fn test_json_value_parse_integer() {
        let gql_num = async_graphql::Value::Number(42.into());
        let result = <JsonValue as ScalarType>::parse(gql_num).expect("should parse int");
        assert_eq!(result.0, json!(42));
    }

    #[test]
    fn test_json_value_parse_float() {
        let gql_num =
            async_graphql::Value::Number(async_graphql::Number::from_f64(3.15).expect("valid f64"));
        let result = <JsonValue as ScalarType>::parse(gql_num).expect("should parse float");
        assert_eq!(result.0, json!(3.15));
    }

    #[test]
    fn test_json_value_parse_string() {
        let gql_str = async_graphql::Value::String("hello".to_string());
        let result = <JsonValue as ScalarType>::parse(gql_str).expect("should parse string");
        assert_eq!(result.0, Value::String("hello".to_string()));
    }

    #[test]
    fn test_json_value_parse_list() {
        let gql_list = async_graphql::Value::List(vec![
            async_graphql::Value::Number(1.into()),
            async_graphql::Value::String("two".to_string()),
            async_graphql::Value::Boolean(true),
        ]);
        let result = <JsonValue as ScalarType>::parse(gql_list).expect("should parse list");
        assert_eq!(result.0, json!([1, "two", true]));
    }

    #[test]
    fn test_json_value_parse_object() {
        let mut map = async_graphql::indexmap::IndexMap::new();
        map.insert(
            async_graphql::Name::new("key"),
            async_graphql::Value::String("value".to_string()),
        );
        let gql_obj = async_graphql::Value::Object(map);
        let result = <JsonValue as ScalarType>::parse(gql_obj).expect("should parse object");
        assert_eq!(result.0, json!({"key": "value"}));
    }

    // -----------------------------------------------------------------------
    // JsonValue scalar: to_value roundtrip tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_json_value_to_value_null() {
        let jv = JsonValue(Value::Null);
        assert_eq!(ScalarType::to_value(&jv), async_graphql::Value::Null);
    }

    #[test]
    fn test_json_value_to_value_bool() {
        let jv = JsonValue(Value::Bool(true));
        assert_eq!(
            ScalarType::to_value(&jv),
            async_graphql::Value::Boolean(true)
        );
    }

    #[test]
    fn test_json_value_to_value_integer() {
        let jv = JsonValue(json!(42));
        let gql_val = ScalarType::to_value(&jv);
        if let async_graphql::Value::Number(n) = gql_val {
            assert_eq!(n.as_i64(), Some(42));
        } else {
            panic!("expected Number variant");
        }
    }

    #[test]
    fn test_json_value_to_value_float() {
        let jv = JsonValue(json!(3.15));
        let gql_val = ScalarType::to_value(&jv);
        if let async_graphql::Value::Number(n) = gql_val {
            let f = n.as_f64().expect("should be f64");
            assert!((f - 3.15).abs() < 1e-10);
        } else {
            panic!("expected Number variant");
        }
    }

    #[test]
    fn test_json_value_to_value_string() {
        let jv = JsonValue(Value::String("hello".to_string()));
        assert_eq!(
            ScalarType::to_value(&jv),
            async_graphql::Value::String("hello".to_string())
        );
    }

    #[test]
    fn test_json_value_to_value_array() {
        let jv = JsonValue(json!([1, "two"]));
        let gql_val = ScalarType::to_value(&jv);
        if let async_graphql::Value::List(items) = gql_val {
            assert_eq!(items.len(), 2);
        } else {
            panic!("expected List variant");
        }
    }

    #[test]
    fn test_json_value_to_value_object() {
        let jv = JsonValue(json!({"a": 1}));
        let gql_val = ScalarType::to_value(&jv);
        if let async_graphql::Value::Object(map) = gql_val {
            assert!(map.contains_key("a"), "should contain key 'a'");
        } else {
            panic!("expected Object variant");
        }
    }

    // -----------------------------------------------------------------------
    // JsonValue scalar: full roundtrip parse -> to_value -> parse
    // -----------------------------------------------------------------------

    #[test]
    fn test_json_value_roundtrip() {
        let original = json!({"name": "test", "count": 5, "active": true, "items": [1, 2]});
        let jv = JsonValue(original.clone());
        let gql_val = ScalarType::to_value(&jv);
        let parsed = <JsonValue as ScalarType>::parse(gql_val).expect("should parse back");
        assert_eq!(parsed.0, original);
    }

    // -----------------------------------------------------------------------
    // build_dynamic_schema smoke test
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_dynamic_schema_does_not_panic() {
        let host = build_test_host(HashMap::new(), HashMap::new());
        let _schema = build_dynamic_schema(host);
    }

    // -----------------------------------------------------------------------
    // DynamicQueryRoot: entity_types
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_query_entity_types() {
        let host = build_test_host(HashMap::new(), HashMap::new());
        let schema = build_dynamic_schema(host);

        let result = schema.execute("{ entityTypes }").await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        let types = data["entityTypes"].as_array().expect("array");
        let strs: Vec<&str> = types.iter().map(|v| v.as_str().expect("str")).collect();
        assert!(strs.contains(&"order"), "should have order");
    }

    // -----------------------------------------------------------------------
    // DynamicQueryRoot: entity found
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_query_entity_found() {
        let order_id = Uuid::new_v4();
        let order_json = json!({"id": order_id.to_string(), "name": "Order #1"});

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(
            "order".to_string(),
            Arc::new(MockFetcher::new().with_entity(order_id, order_json)),
        );

        let host = build_test_host(fetchers, HashMap::new());
        let schema = build_dynamic_schema(host);

        let query = format!(r#"{{ entity(id: "{}", entityType: "order") }}"#, order_id);
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    }

    // -----------------------------------------------------------------------
    // DynamicQueryRoot: entity unknown type returns error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_query_entity_unknown_type_errors() {
        let host = build_test_host(HashMap::new(), HashMap::new());
        let schema = build_dynamic_schema(host);

        let id = Uuid::new_v4();
        let query = format!(r#"{{ entity(id: "{}", entityType: "widget") }}"#, id);
        let result = schema.execute(&query).await;
        assert!(
            !result.errors.is_empty(),
            "unknown type should produce error"
        );
        let err_msg = format!("{:?}", result.errors);
        assert!(
            err_msg.contains("Unknown entity type"),
            "should mention unknown entity type: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // DynamicQueryRoot: entity invalid UUID
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_query_entity_invalid_uuid() {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));

        let host = build_test_host(fetchers, HashMap::new());
        let schema = build_dynamic_schema(host);

        let query = r#"{ entity(id: "not-a-uuid", entityType: "order") }"#;
        let result = schema.execute(query).await;
        assert!(
            !result.errors.is_empty(),
            "invalid UUID should produce error"
        );
    }

    // -----------------------------------------------------------------------
    // DynamicQueryRoot: entities list
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_query_entities_list() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(
            "order".to_string(),
            Arc::new(
                MockFetcher::new()
                    .with_entity(id1, json!({"id": id1.to_string(), "name": "A"}))
                    .with_entity(id2, json!({"id": id2.to_string(), "name": "B"})),
            ),
        );

        let host = build_test_host(fetchers, HashMap::new());
        let schema = build_dynamic_schema(host);

        let query = r#"{ entities(entityType: "order") }"#;
        let result = schema.execute(query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    }

    // -----------------------------------------------------------------------
    // DynamicQueryRoot: entities for unknown type
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_query_entities_unknown_type() {
        let host = build_test_host(HashMap::new(), HashMap::new());
        let schema = build_dynamic_schema(host);

        let query = r#"{ entities(entityType: "widget") }"#;
        let result = schema.execute(query).await;
        assert!(
            !result.errors.is_empty(),
            "unknown type should produce error"
        );
    }

    // -----------------------------------------------------------------------
    // DynamicMutationRoot: createEntity
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_mutation_create_entity() {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));

        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), Arc::new(MockCreator));

        let host = build_test_host(fetchers, creators);
        let schema = build_dynamic_schema(host);

        let query = r#"mutation { createEntity(entityType: "order", data: {name: "test"}) }"#;
        let result = schema.execute(query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    }

    // -----------------------------------------------------------------------
    // DynamicMutationRoot: createEntity unknown type
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_mutation_create_entity_unknown_type() {
        let host = build_test_host(HashMap::new(), HashMap::new());
        let schema = build_dynamic_schema(host);

        let query = r#"mutation { createEntity(entityType: "widget", data: {name: "x"}) }"#;
        let result = schema.execute(query).await;
        assert!(
            !result.errors.is_empty(),
            "unknown type should produce error"
        );
    }

    // -----------------------------------------------------------------------
    // DynamicMutationRoot: deleteEntity
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_mutation_delete_entity() {
        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), Arc::new(MockCreator));

        let host = build_test_host(HashMap::new(), creators);
        let schema = build_dynamic_schema(host);

        let id = Uuid::new_v4();
        let query = format!(
            r#"mutation {{ deleteEntity(id: "{}", entityType: "order") }}"#,
            id
        );
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        assert_eq!(data["deleteEntity"], true);
    }

    // -----------------------------------------------------------------------
    // DynamicMutationRoot: createLink and entityLinks query
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_mutation_create_link_and_query_entity_links() {
        let host = build_test_host(HashMap::new(), HashMap::new());
        let schema = build_dynamic_schema(host);

        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        // Create a link
        let create_query = format!(
            r#"mutation {{ createLink(sourceId: "{}", targetId: "{}", linkType: "has_invoice") }}"#,
            source_id, target_id
        );
        let result = schema.execute(&create_query).await;
        assert!(
            result.errors.is_empty(),
            "create errors: {:?}",
            result.errors
        );

        // Query entity_links
        let links_query = format!(r#"{{ entityLinks(entityId: "{}") }}"#, source_id);
        let result = schema.execute(&links_query).await;
        assert!(
            result.errors.is_empty(),
            "query errors: {:?}",
            result.errors
        );
    }

    // -----------------------------------------------------------------------
    // DynamicMutationRoot: deleteLink
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dynamic_mutation_delete_link() {
        let host = build_test_host(HashMap::new(), HashMap::new());

        // Create a link in the store first
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let link = LinkEntity::new("has_invoice", source_id, target_id, None);
        let created = host
            .link_service
            .create(link)
            .await
            .expect("should create link");

        let schema = build_dynamic_schema(host);

        let query = format!(r#"mutation {{ deleteLink(linkId: "{}") }}"#, created.id);
        let result = schema.execute(&query).await;
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let data = result.data.into_json().expect("json");
        assert_eq!(data["deleteLink"], true);
    }
}
