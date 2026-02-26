//! Field and relation resolution for GraphQL entities

use anyhow::Result;
use futures::future::{BoxFuture, FutureExt};
use graphql_parser::query::{Field, Selection};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use super::utils;
use crate::server::host::ServerHost;

/// Resolve fields for a list of entities
pub async fn resolve_entity_list(
    host: &Arc<ServerHost>,
    entities: Vec<Value>,
    selections: &[Selection<'_, String>],
    entity_type: &str,
) -> Result<Vec<Value>> {
    let mut resolved = Vec::new();

    for entity in entities {
        let resolved_entity = resolve_entity_fields(host, entity, selections, entity_type).await?;
        resolved.push(resolved_entity);
    }

    Ok(resolved)
}

/// Resolve fields for a single entity
pub fn resolve_entity_fields<'a>(
    host: &'a Arc<ServerHost>,
    entity: Value,
    selections: &'a [Selection<'_, String>],
    entity_type: &'a str,
) -> BoxFuture<'a, Result<Value>> {
    async move { resolve_entity_fields_impl(host, entity, selections, entity_type).await }.boxed()
}

/// Implementation of resolve_entity_fields
async fn resolve_entity_fields_impl(
    host: &Arc<ServerHost>,
    entity: Value,
    selections: &[Selection<'_, String>],
    entity_type: &str,
) -> Result<Value> {
    let mut result = serde_json::Map::new();

    let entity_obj = entity
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Entity is not an object"))?;

    for selection in selections {
        if let Selection::Field(field) = selection {
            let field_name = field.name.as_str();

            // Check if this is a regular field (exists in the entity data)
            if let Some(value) = entity_obj.get(field_name) {
                result.insert(field_name.to_string(), value.clone());
                continue;
            }

            // Check if this is a snake_case vs camelCase mismatch
            let snake_case_name = utils::camel_to_snake(field_name);
            if let Some(value) = entity_obj.get(&snake_case_name) {
                result.insert(field_name.to_string(), value.clone());
                continue;
            }

            // Check if this is a relation field
            if let Some(relation_value) =
                resolve_relation_field_impl(host, entity_obj, field, entity_type).await?
            {
                result.insert(field_name.to_string(), relation_value);
                continue;
            }

            // Field not found - return null
            result.insert(field_name.to_string(), Value::Null);
        }
    }

    Ok(Value::Object(result))
}

/// Resolve a relation field (e.g., "invoices" for an order)
fn resolve_relation_field_impl<'a>(
    host: &'a Arc<ServerHost>,
    entity: &'a serde_json::Map<String, Value>,
    field: &'a Field<'_, String>,
    entity_type: &'a str,
) -> BoxFuture<'a, Result<Option<Value>>> {
    async move { resolve_relation_field_inner(host, entity, field, entity_type).await }.boxed()
}

/// Inner implementation of resolve_relation_field
async fn resolve_relation_field_inner(
    host: &Arc<ServerHost>,
    entity: &serde_json::Map<String, Value>,
    field: &Field<'_, String>,
    entity_type: &str,
) -> Result<Option<Value>> {
    let field_name = field.name.as_str();
    let entity_id = entity
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Entity missing id field"))?;
    let source_uuid = Uuid::parse_str(entity_id)?;

    // Get links configuration for this entity type
    let links_config = &host.config;

    // Find the link configuration for this relation
    for link_config in &links_config.links {
        if link_config.source_type == entity_type && link_config.forward_route_name == field_name {
            // This is a forward relation (e.g., order -> invoices)
            let links = host
                .link_service
                .find_by_source(
                    &source_uuid,
                    Some(&link_config.link_type),
                    Some(&link_config.target_type),
                )
                .await?;

            // Fetch the target entities
            if let Some(fetcher) = host.entity_fetchers.get(&link_config.target_type) {
                let mut targets = Vec::new();

                for link in links {
                    if let Ok(target_entity) = fetcher.fetch_as_json(&link.target_id).await {
                        let resolved = resolve_entity_fields_impl(
                            host,
                            target_entity,
                            &field.selection_set.items,
                            &link_config.target_type,
                        )
                        .await?;
                        targets.push(resolved);
                    }
                }

                return Ok(Some(Value::Array(targets)));
            }
        } else if link_config.target_type == entity_type
            && link_config.reverse_route_name == field_name
        {
            // This is a reverse relation (e.g., invoice -> order)
            let links = host
                .link_service
                .find_by_target(
                    &source_uuid,
                    Some(&link_config.link_type),
                    Some(&link_config.source_type),
                )
                .await?;

            // Fetch the source entity (should be only one for singular relations)
            if let Some(link) = links.first()
                && let Some(fetcher) = host.entity_fetchers.get(&link_config.source_type)
                && let Ok(source_entity) = fetcher.fetch_as_json(&link.source_id).await
            {
                let resolved = resolve_entity_fields_impl(
                    host,
                    source_entity,
                    &field.selection_set.items,
                    &link_config.source_type,
                )
                .await?;
                return Ok(Some(resolved));
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::super::core::GraphQLExecutor;
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::EntityFetcher;
    use crate::core::link::{LinkDefinition, LinkEntity};
    use crate::core::service::LinkService;
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use crate::server::host::ServerHost;
    use crate::storage::in_memory::InMemoryLinkService;
    use async_trait::async_trait;
    use axum::Router;
    use graphql_parser::Pos;
    use graphql_parser::query::SelectionSet;
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
            _limit: Option<i32>,
            _offset: Option<i32>,
        ) -> anyhow::Result<Vec<Value>> {
            let entities = self.entities.lock().expect("lock poisoned");
            Ok(entities.values().cloned().collect())
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

    fn build_test_host_with_link_service(
        fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
        link_service: Arc<InMemoryLinkService>,
    ) -> Arc<ServerHost> {
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

    /// Helper to create a Field with given name and sub-field names
    fn make_field_with_selections(name: &str, sub_fields: &[&str]) -> Field<'static, String> {
        let pos = Pos { line: 1, column: 1 };
        let sub_items: Vec<Selection<'static, String>> = sub_fields
            .iter()
            .map(|f| {
                Selection::Field(Field {
                    position: pos,
                    alias: None,
                    name: f.to_string(),
                    arguments: vec![],
                    directives: vec![],
                    selection_set: SelectionSet {
                        span: (pos, pos),
                        items: vec![],
                    },
                })
            })
            .collect();

        Field {
            position: pos,
            alias: None,
            name: name.to_string(),
            arguments: vec![],
            directives: vec![],
            selection_set: SelectionSet {
                span: (pos, pos),
                items: sub_items,
            },
        }
    }

    // -----------------------------------------------------------------------
    // resolve_entity_fields tests (using manually constructed fields)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_direct_fields() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
        let host = build_test_host_with_link_service(fetchers, link_service);

        let entity = json!({"id": "abc-123", "name": "Order 1", "total": 99.9});
        let field = make_field_with_selections("order", &["id", "name", "total"]);

        let result = resolve_entity_fields(&host, entity, &field.selection_set.items, "order")
            .await
            .expect("should resolve fields");

        assert_eq!(result.get("id").and_then(|v| v.as_str()), Some("abc-123"));
        assert_eq!(result.get("name").and_then(|v| v.as_str()), Some("Order 1"));
        assert_eq!(result.get("total").and_then(|v| v.as_f64()), Some(99.9));
    }

    #[tokio::test]
    async fn test_resolve_camel_to_snake_case_field() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
        let host = build_test_host_with_link_service(fetchers, link_service);

        // Entity has snake_case field, query asks for camelCase
        let entity = json!({"id": "abc", "created_at": "2024-01-01T00:00:00Z"});
        let field = make_field_with_selections("order", &["id", "createdAt"]);

        let result = resolve_entity_fields(&host, entity, &field.selection_set.items, "order")
            .await
            .expect("should resolve camelCase -> snake_case");

        assert_eq!(
            result.get("createdAt").and_then(|v| v.as_str()),
            Some("2024-01-01T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn test_resolve_unknown_field_returns_null() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
        let host = build_test_host_with_link_service(fetchers, link_service);

        let order_id = Uuid::new_v4();
        let entity = json!({"id": order_id.to_string()});
        let field = make_field_with_selections("order", &["id", "nonExistentField"]);

        let result = resolve_entity_fields(&host, entity, &field.selection_set.items, "order")
            .await
            .expect("should resolve with null for unknown");

        assert_eq!(result.get("nonExistentField"), Some(&Value::Null));
    }

    #[tokio::test]
    async fn test_resolve_entity_not_object_returns_err() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
        let host = build_test_host_with_link_service(fetchers, link_service);

        let entity = json!("not an object");
        let field = make_field_with_selections("order", &["id"]);

        let result =
            resolve_entity_fields(&host, entity, &field.selection_set.items, "order").await;
        assert!(result.is_err(), "non-object entity should error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("not an object"),
            "should mention not an object: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // resolve_entity_list tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_entity_list_multiple_entities() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
        let host = build_test_host_with_link_service(fetchers, link_service);

        let entities = vec![
            json!({"id": "1", "name": "Order 1"}),
            json!({"id": "2", "name": "Order 2"}),
        ];
        let field = make_field_with_selections("orders", &["id", "name"]);

        let result = resolve_entity_list(&host, entities, &field.selection_set.items, "order")
            .await
            .expect("should resolve list");

        assert_eq!(result.len(), 2, "should have two resolved entities");
        assert_eq!(
            result[0].get("name").and_then(|v| v.as_str()),
            Some("Order 1")
        );
        assert_eq!(
            result[1].get("name").and_then(|v| v.as_str()),
            Some("Order 2")
        );
    }

    #[tokio::test]
    async fn test_resolve_entity_list_empty() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher::new()));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));
        let host = build_test_host_with_link_service(fetchers, link_service);

        let entities: Vec<Value> = vec![];
        let field = make_field_with_selections("orders", &["id"]);

        let result = resolve_entity_list(&host, entities, &field.selection_set.items, "order")
            .await
            .expect("should resolve empty list");

        assert!(result.is_empty(), "empty input should produce empty output");
    }

    // -----------------------------------------------------------------------
    // Relation tests via GraphQLExecutor (end-to-end)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_forward_relation_field() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        // Create a link: order -> invoice
        let link = LinkEntity::new("has_invoice", order_id, invoice_id, None);
        link_service.create(link).await.expect("should create link");

        let order = json!({"id": order_id.to_string(), "name": "Order 1"});
        let invoice = json!({"id": invoice_id.to_string(), "amount": 100});

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(
            "order".to_string(),
            Arc::new(MockFetcher::new().with_entity(order_id, order)),
        );
        fetchers.insert(
            "invoice".to_string(),
            Arc::new(MockFetcher::new().with_entity(invoice_id, invoice)),
        );

        let host = build_test_host_with_link_service(fetchers, link_service);
        let executor = GraphQLExecutor::new(host).await;

        let query = format!(
            r#"{{ order(id: "{}") {{ id name invoices {{ id amount }} }} }}"#,
            order_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should resolve forward relation");

        let order_result = result
            .get("data")
            .and_then(|d| d.get("order"))
            .expect("should have order");
        let invoices = order_result
            .get("invoices")
            .expect("should have invoices field");
        assert!(invoices.is_array(), "invoices should be array");
        let arr = invoices.as_array().expect("array");
        assert_eq!(arr.len(), 1, "should have one invoice");
        assert_eq!(arr[0].get("amount").and_then(|v| v.as_i64()), Some(100));
    }

    #[tokio::test]
    async fn test_resolve_reverse_relation_field() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let order_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        // Create a link: order -> invoice
        let link = LinkEntity::new("has_invoice", order_id, invoice_id, None);
        link_service.create(link).await.expect("should create link");

        let order = json!({"id": order_id.to_string(), "name": "Order 1"});
        let invoice = json!({"id": invoice_id.to_string(), "amount": 50});

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(
            "order".to_string(),
            Arc::new(MockFetcher::new().with_entity(order_id, order)),
        );
        fetchers.insert(
            "invoice".to_string(),
            Arc::new(MockFetcher::new().with_entity(invoice_id, invoice)),
        );

        let host = build_test_host_with_link_service(fetchers, link_service);
        let executor = GraphQLExecutor::new(host).await;

        // Query invoice with reverse relation "order"
        let query = format!(
            r#"{{ invoice(id: "{}") {{ id order {{ id name }} }} }}"#,
            invoice_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should resolve reverse relation");

        let invoice_result = result
            .get("data")
            .and_then(|d| d.get("invoice"))
            .expect("should have invoice");
        let order_val = invoice_result
            .get("order")
            .expect("should have order field");
        assert!(order_val.is_object(), "order should be an object");
        assert_eq!(
            order_val.get("name").and_then(|v| v.as_str()),
            Some("Order 1")
        );
    }

    #[tokio::test]
    async fn test_resolve_forward_relation_no_links_returns_empty_array() {
        let link_service = Arc::new(InMemoryLinkService::new());
        let order_id = Uuid::new_v4();

        let order = json!({"id": order_id.to_string(), "name": "Order 1"});

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert(
            "order".to_string(),
            Arc::new(MockFetcher::new().with_entity(order_id, order)),
        );
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher::new()));

        let host = build_test_host_with_link_service(fetchers, link_service);
        let executor = GraphQLExecutor::new(host).await;

        let query = format!(
            r#"{{ order(id: "{}") {{ id invoices {{ id }} }} }}"#,
            order_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should resolve even with no links");

        let order_result = result
            .get("data")
            .and_then(|d| d.get("order"))
            .expect("should have order");
        let invoices = order_result
            .get("invoices")
            .expect("should have invoices field");
        assert!(invoices.is_array(), "should be array");
        assert_eq!(
            invoices.as_array().expect("array").len(),
            0,
            "should be empty when no links"
        );
    }
}
