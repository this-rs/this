//! Core GraphQL executor orchestration

use anyhow::{Result, bail};
use graphql_parser::query::{Document, OperationDefinition, Selection, parse_query};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

use super::mutation_executor;
use super::query_executor;
use crate::server::exposure::graphql::schema_generator::SchemaGenerator;
use crate::server::host::ServerHost;

/// GraphQL executor that executes queries against the dynamic schema
pub struct GraphQLExecutor {
    host: Arc<ServerHost>,
    #[allow(dead_code)]
    schema_sdl: String,
}

impl GraphQLExecutor {
    /// Create a new executor with the given host
    pub async fn new(host: Arc<ServerHost>) -> Self {
        let generator = SchemaGenerator::new(host.clone());
        let schema_sdl = generator.generate_sdl().await;

        Self { host, schema_sdl }
    }

    /// Execute a GraphQL query and return the result as JSON
    pub async fn execute(
        &self,
        query: &str,
        variables: Option<HashMap<String, Value>>,
    ) -> Result<Value> {
        // Parse the query
        let doc = parse_query::<String>(query)
            .map_err(|e| anyhow::anyhow!("Failed to parse query: {:?}", e))?;

        // Execute the query
        let result = self
            .execute_document(&doc, variables.unwrap_or_default())
            .await?;

        Ok(json!({
            "data": result
        }))
    }

    /// Execute a parsed GraphQL document
    async fn execute_document(
        &self,
        doc: &Document<'_, String>,
        variables: HashMap<String, Value>,
    ) -> Result<Value> {
        // Find the operation to execute (default to first query)
        let operation = doc
            .definitions
            .iter()
            .find_map(|def| {
                if let graphql_parser::query::Definition::Operation(op) = def {
                    Some(op)
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow::anyhow!("No operation found in query"))?;

        match operation {
            OperationDefinition::Query(query) => {
                self.execute_query(&query.selection_set.items, &variables)
                    .await
            }
            OperationDefinition::Mutation(mutation) => {
                self.execute_mutation(&mutation.selection_set.items, &variables)
                    .await
            }
            OperationDefinition::SelectionSet(selection_set) => {
                self.execute_query(&selection_set.items, &variables).await
            }
            _ => bail!("Subscriptions are not supported"),
        }
    }

    /// Execute a query operation
    async fn execute_query(
        &self,
        selections: &[Selection<'_, String>],
        _variables: &HashMap<String, Value>,
    ) -> Result<Value> {
        let mut result = serde_json::Map::new();

        for selection in selections {
            if let Selection::Field(field) = selection {
                let field_name = field.name.as_str();
                let field_value = query_executor::resolve_query_field(&self.host, field).await?;
                result.insert(field_name.to_string(), field_value);
            }
        }

        Ok(Value::Object(result))
    }

    /// Execute a mutation operation
    async fn execute_mutation(
        &self,
        selections: &[Selection<'_, String>],
        _variables: &HashMap<String, Value>,
    ) -> Result<Value> {
        let mut result = serde_json::Map::new();

        for selection in selections {
            if let Selection::Field(field) = selection {
                let field_name = field.name.as_str();
                let field_value =
                    mutation_executor::resolve_mutation_field(&self.host, field).await?;
                result.insert(field_name.to_string(), field_value);
            }
        }

        Ok(Value::Object(result))
    }
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::link::LinkDefinition;
    use crate::core::{EntityCreator, EntityFetcher};
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use crate::storage::in_memory::InMemoryLinkService;
    use async_trait::async_trait;
    use axum::Router;
    use serde_json::json;
    use uuid::Uuid;

    // -----------------------------------------------------------------------
    // Shared mock infrastructure
    // -----------------------------------------------------------------------

    /// Mock entity fetcher that stores entities in memory
    struct MockFetcher {
        entities: std::sync::Mutex<HashMap<Uuid, Value>>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                entities: std::sync::Mutex::new(HashMap::new()),
            }
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

    /// Mock entity creator that returns the data with a generated id
    struct MockCreator;

    #[async_trait]
    impl EntityCreator for MockCreator {
        async fn create_from_json(&self, mut entity_data: Value) -> anyhow::Result<Value> {
            let id = Uuid::new_v4();
            if let Some(obj) = entity_data.as_object_mut() {
                obj.insert("id".to_string(), json!(id.to_string()));
            }
            Ok(entity_data)
        }

        async fn update_from_json(
            &self,
            entity_id: &Uuid,
            mut entity_data: Value,
        ) -> anyhow::Result<Value> {
            if let Some(obj) = entity_data.as_object_mut() {
                obj.insert("id".to_string(), json!(entity_id.to_string()));
            }
            Ok(entity_data)
        }

        async fn delete(&self, _entity_id: &Uuid) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Minimal entity descriptor for the registry
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

    /// Build a ServerHost with "order" and "invoice" entity types,
    /// an InMemoryLinkService, and an order->invoice link definition.
    fn build_test_host(
        fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
        creators: HashMap<String, Arc<dyn EntityCreator>>,
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

        let host =
            ServerHost::from_builder_components(link_service, config, registry, fetchers, creators)
                .expect("should build test host");

        Arc::new(host)
    }

    fn default_host() -> Arc<ServerHost> {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));

        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), Arc::new(MockCreator));
        creators.insert("invoice".to_string(), Arc::new(MockCreator));

        build_test_host(fetchers, creators)
    }

    // -----------------------------------------------------------------------
    // core.rs tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_execute_valid_query_returns_data_wrapper() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute("query { orders { id } }", None)
            .await
            .expect("execute should succeed");

        assert!(
            result.get("data").is_some(),
            "result should have a 'data' key"
        );
    }

    #[tokio::test]
    async fn test_execute_shorthand_query_treated_as_query() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        // Shorthand selection set (no `query` keyword)
        let result = executor
            .execute("{ orders { id } }", None)
            .await
            .expect("shorthand query should succeed");

        let data = result.get("data").expect("should have data key");
        assert!(
            data.get("orders").is_some(),
            "data should contain orders field"
        );
    }

    #[tokio::test]
    async fn test_execute_parse_error_returns_err() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor.execute("not valid graphql {{{{", None).await;

        assert!(result.is_err(), "parse error should return Err");
        let err_msg = result.expect_err("should be error").to_string();
        assert!(
            err_msg.contains("Failed to parse query"),
            "error message should mention parsing: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_execute_empty_document_returns_err() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        // A fragment-only document has no operation
        let result = executor.execute("fragment F on Order { id }", None).await;

        assert!(result.is_err(), "empty doc should return Err");
        let err_msg = result.expect_err("should be error").to_string();
        assert!(
            err_msg.contains("No operation found"),
            "should mention no operation: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_execute_mutation_dispatches_correctly() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        let query = r#"mutation { createOrder(data: {name: "test"}) { id } }"#;
        let result = executor
            .execute(query, None)
            .await
            .expect("mutation should succeed");

        let data = result.get("data").expect("should have data");
        let created = data.get("createOrder").expect("should have createOrder");
        assert!(
            created.get("id").is_some(),
            "created entity should have an id"
        );
    }

    #[tokio::test]
    async fn test_execute_with_variables_does_not_panic() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        let mut vars = HashMap::new();
        vars.insert("limit".to_string(), json!(10));

        let result = executor
            .execute("query { orders { id } }", Some(vars))
            .await
            .expect("should not panic with variables");

        assert!(result.get("data").is_some());
    }

    #[tokio::test]
    async fn test_execute_query_returns_empty_list_for_no_entities() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute("query { orders { id } }", None)
            .await
            .expect("should succeed");

        let orders = result
            .get("data")
            .and_then(|d| d.get("orders"))
            .expect("should have orders");

        assert!(orders.is_array(), "orders should be an array");
        assert_eq!(
            orders.as_array().expect("should be array").len(),
            0,
            "orders should be empty"
        );
    }

    #[tokio::test]
    async fn test_execute_subscription_returns_error() {
        let host = default_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute("subscription { orderCreated { id } }", None)
            .await;

        assert!(result.is_err(), "subscriptions should not be supported");
        let err_msg = result.expect_err("should be error").to_string();
        assert!(
            err_msg.contains("Subscriptions are not supported"),
            "should mention subscriptions: {}",
            err_msg
        );
    }
}
