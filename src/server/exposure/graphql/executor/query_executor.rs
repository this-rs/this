//! Query execution for GraphQL

use anyhow::{Result, bail};
use graphql_parser::query::Field;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use super::field_resolver;
use super::utils;
use crate::server::host::ServerHost;

/// Resolve a query field (e.g., "orders", "order", "invoice", etc.)
pub async fn resolve_query_field(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();

    // Check if this is a plural query (e.g., "orders", "invoices")
    if let Some(entity_type) = get_entity_type_from_plural(host, field_name) {
        // Get pagination arguments
        let limit = utils::get_int_arg(field, "limit");
        let offset = utils::get_int_arg(field, "offset");

        // Fetch entities
        if let Some(fetcher) = host.entity_fetchers.get(entity_type) {
            let entities = fetcher.list_as_json(limit, offset).await?;

            // Resolve sub-fields for each entity
            let resolved_entities = field_resolver::resolve_entity_list(
                host,
                entities,
                &field.selection_set.items,
                entity_type,
            )
            .await?;

            return Ok(Value::Array(resolved_entities));
        } else {
            bail!("Unknown entity type: {}", entity_type);
        }
    }

    // Check if this is a singular query (e.g., "order", "invoice")
    if let Some(entity_type) = get_entity_type_from_singular(host, field_name) {
        // Get the ID argument
        let id = utils::get_string_arg(field, "id")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
        let uuid = Uuid::parse_str(&id)?;

        // Fetch the entity
        if let Some(fetcher) = host.entity_fetchers.get(entity_type) {
            let entity = fetcher.fetch_as_json(&uuid).await?;

            // Resolve sub-fields
            let resolved = field_resolver::resolve_entity_fields(
                host,
                entity,
                &field.selection_set.items,
                entity_type,
            )
            .await?;

            return Ok(resolved);
        } else {
            bail!("Unknown entity type: {}", entity_type);
        }
    }

    bail!("Unknown query field: {}", field_name);
}

/// Get entity type from plural field name (e.g., "orders" -> "order")
fn get_entity_type_from_plural<'a>(host: &'a Arc<ServerHost>, field_name: &str) -> Option<&'a str> {
    for entity_type in host.entity_types() {
        let plural = utils::pluralize(entity_type);
        if plural == field_name {
            return Some(entity_type);
        }
    }
    None
}

/// Get entity type from singular field name (e.g., "order" -> "order")
fn get_entity_type_from_singular<'a>(
    host: &'a Arc<ServerHost>,
    field_name: &str,
) -> Option<&'a str> {
    host.entity_types()
        .into_iter()
        .find(|&entity_type| entity_type == field_name)
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::link::LinkDefinition;
    use crate::core::EntityFetcher;
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use super::super::core::GraphQLExecutor;
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

        let host = ServerHost::from_builder_components(
            link_service,
            config,
            registry,
            fetchers,
            HashMap::new(),
        )
        .expect("should build test host");

        Arc::new(host)
    }

    // -----------------------------------------------------------------------
    // Tests via GraphQLExecutor::execute (integration-style)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_plural_query_returns_entity_list() {
        let order_id = Uuid::new_v4();
        let order = json!({"id": order_id.to_string(), "name": "Order 1"});

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(
            "order".to_string(),
            Arc::new(MockFetcher::new().with_entity(order_id, order)),
        );
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));

        let host = build_test_host(fetchers);
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute("query { orders { id name } }", None)
            .await
            .expect("should resolve orders");

        let orders = result
            .get("data")
            .and_then(|d| d.get("orders"))
            .expect("should have orders");
        assert!(orders.is_array(), "plural query should return array");
        let arr = orders.as_array().expect("should be array");
        assert_eq!(arr.len(), 1, "should have one order");
    }

    #[tokio::test]
    async fn test_plural_query_empty_collection() {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));

        let host = build_test_host(fetchers);
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute("query { orders { id } }", None)
            .await
            .expect("should resolve orders");

        let orders = result
            .get("data")
            .and_then(|d| d.get("orders"))
            .expect("should have orders");
        assert!(orders.is_array(), "should be an array");
        assert_eq!(
            orders.as_array().expect("array").len(),
            0,
            "should be empty"
        );
    }

    #[tokio::test]
    async fn test_singular_query_returns_single_entity() {
        let order_id = Uuid::new_v4();
        let order = json!({"id": order_id.to_string(), "total": 99.9});

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(
            "order".to_string(),
            Arc::new(MockFetcher::new().with_entity(order_id, order)),
        );
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));

        let host = build_test_host(fetchers);
        let executor = GraphQLExecutor::new(host).await;

        let query_str = format!(r#"{{ order(id: "{}") {{ id total }} }}"#, order_id);
        let result = executor
            .execute(&query_str, None)
            .await
            .expect("should resolve single order");

        let order_result = result
            .get("data")
            .and_then(|d| d.get("order"))
            .expect("should have order");
        assert!(order_result.is_object(), "singular query should return object");
        assert_eq!(
            order_result.get("id").and_then(|v| v.as_str()),
            Some(order_id.to_string()).as_deref()
        );
    }

    #[tokio::test]
    async fn test_singular_query_missing_id_arg_returns_err() {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));

        let host = build_test_host(fetchers);
        let executor = GraphQLExecutor::new(host).await;

        let result = executor.execute("{ order { id } }", None).await;
        assert!(result.is_err(), "missing id should return error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("id"),
            "error should mention 'id': {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_unknown_field_returns_err() {
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));

        let host = build_test_host(fetchers);
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute("{ unknownEntity { id } }", None)
            .await;
        assert!(result.is_err(), "unknown field should return error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("Unknown query field"),
            "should mention unknown field: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // Helper function unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_entity_type_from_plural_known() {
        let host = {
            let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
            fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
            fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
            build_test_host(fetchers)
        };

        assert_eq!(get_entity_type_from_plural(&host, "orders"), Some("order"));
        assert_eq!(
            get_entity_type_from_plural(&host, "invoices"),
            Some("invoice")
        );
    }

    #[test]
    fn test_get_entity_type_from_plural_unknown() {
        let host = {
            let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
            fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
            build_test_host(fetchers)
        };

        assert_eq!(get_entity_type_from_plural(&host, "widgets"), None);
    }

    #[test]
    fn test_get_entity_type_from_singular_known() {
        let host = {
            let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
            fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
            fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
            build_test_host(fetchers)
        };

        assert_eq!(
            get_entity_type_from_singular(&host, "order"),
            Some("order")
        );
        assert_eq!(
            get_entity_type_from_singular(&host, "invoice"),
            Some("invoice")
        );
    }

    #[test]
    fn test_get_entity_type_from_singular_unknown() {
        let host = {
            let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
            fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
            build_test_host(fetchers)
        };

        assert_eq!(get_entity_type_from_singular(&host, "widget"), None);
    }
}
