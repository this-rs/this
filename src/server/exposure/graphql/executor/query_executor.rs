//! Query execution for GraphQL

use anyhow::{Result, bail};
use graphql_parser::query::Field;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use super::field_resolver;
use super::utils;
use crate::descriptor::Operation;
use crate::server::exposure::guard::SurfaceGuard;
use crate::server::host::ServerHost;

/// Resolve a query field (e.g., "orders", "order", "invoice", etc.)
pub async fn resolve_query_field(
    host: &Arc<ServerHost>,
    guard: &SurfaceGuard,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();

    // Check if this is a plural query (e.g., "orders", "invoices")
    if let Some(entity_type) = get_entity_type_from_plural(host, field_name) {
        let entity_type = entity_type.as_str();
        // Get pagination arguments
        let limit = utils::get_int_arg(field, "limit");
        let offset = utils::get_int_arg(field, "offset");

        let fetcher = guard
            .fetcher(host, entity_type, Operation::List, None)
            .map_err(anyhow::Error::new)?;

        // Typed filters (validation stream): `filter` (object or JSON string, REST
        // grammar) and `sort` (REST grammar). Present → checked against the descriptor
        // (strict without one) and applied BEFORE pagination; malformed → a
        // `ContractError` (`BAD_USER_INPUT`). Absent → historical path, unchanged.
        let filter = utils::get_json_arg(field, "filter");
        // `sort` present but not a string (list, number, object...) is a typed error:
        // reading it as "no sort" would silently ignore what the client asked for.
        let sort = match utils::get_json_arg(field, "sort") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s),
            Some(other) => {
                return Err(crate::descriptor::ContractError::framework(
                    crate::descriptor::framework::codes::QUERY_MALFORMED_SORT,
                )
                .with_param(
                    "reason",
                    format!("`sort` must be a string (REST grammar), got {other}"),
                )
                .into());
            }
        };
        if filter.is_some() || sort.is_some() {
            let descriptors = host.descriptors();
            let desc = descriptors.entity(entity_type).map(|(_, d)| d);
            let q = crate::core::query::query_from_args(
                desc,
                filter,
                sort.as_deref(),
                limit.and_then(|l| u32::try_from(l).ok()),
                offset.and_then(|o| u32::try_from(o).ok()),
            )?;
            let (entities, _total) = fetcher.list_query(&q).await?;
            let resolved_entities = field_resolver::resolve_entity_list(
                host,
                guard,
                entities,
                &field.selection_set.items,
                entity_type,
            )
            .await?;
            return Ok(Value::Array(resolved_entities));
        }

        // Fetch entities
        let entities = fetcher.list_as_json(limit, offset).await?;

        // Resolve sub-fields for each entity
        let resolved_entities = field_resolver::resolve_entity_list(
            host,
            guard,
            entities,
            &field.selection_set.items,
            entity_type,
        )
        .await?;

        return Ok(Value::Array(resolved_entities));
    }

    // Check if this is a singular query (e.g., "order", "invoice")
    if let Some(entity_type) = get_entity_type_from_singular(host, field_name) {
        let entity_type = entity_type.as_str();
        // Get the ID argument
        let id = utils::get_string_arg(field, "id")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
        let uuid = Uuid::parse_str(&id)?;

        // Fetch the entity
        let fetcher = guard
            .fetcher(host, entity_type, Operation::Get, Some(&uuid))
            .map_err(anyhow::Error::new)?;
        let entity = fetcher.fetch_as_json(&uuid).await?;

        // Resolve sub-fields
        let resolved = field_resolver::resolve_entity_fields(
            host,
            guard,
            entity,
            &field.selection_set.items,
            entity_type,
        )
        .await?;

        return Ok(resolved);
    }

    // ── Notification queries (when NotificationStore is configured) ──
    // Notification queries take the user from an ARGUMENT: not covered by the
    // policies, refused when they are enforced (fail-closed).
    if host.notification_store().is_some()
        && matches!(field_name, "notifications" | "unreadNotificationCount")
    {
        guard
            .deny_uncovered(field_name)
            .map_err(anyhow::Error::new)?;
    }
    if let Some(store) = host.notification_store() {
        if field_name == "notifications" {
            let user_id = utils::get_string_arg(field, "userId")
                .ok_or_else(|| anyhow::anyhow!("Missing required argument 'userId'"))?;
            let limit = utils::get_int_arg(field, "limit").unwrap_or(20).min(100) as usize;
            let offset = utils::get_int_arg(field, "offset").unwrap_or(0) as usize;

            let notifications = store.list_by_user(&user_id, limit, offset).await;
            let total = store.total_count(&user_id).await;
            let unread = store.unread_count(&user_id).await;

            let items: Vec<Value> = notifications
                .into_iter()
                .map(|n| serde_json::to_value(n).unwrap_or(Value::Null))
                .collect();

            return Ok(serde_json::json!({
                "notifications": items,
                "total": total,
                "unread": unread,
                "limit": limit,
                "offset": offset,
            }));
        }

        if field_name == "unreadNotificationCount" {
            let user_id = utils::get_string_arg(field, "userId")
                .ok_or_else(|| anyhow::anyhow!("Missing required argument 'userId'"))?;
            let count = store.unread_count(&user_id).await;
            return Ok(serde_json::json!(count));
        }
    }

    bail!("Unknown query field: {}", field_name);
}

/// Get entity type from plural field name (e.g., "orders" -> "order")
fn get_entity_type_from_plural(host: &Arc<ServerHost>, field_name: &str) -> Option<String> {
    let catalog = host.catalog();
    catalog
        .entity_types()
        .into_iter()
        .find(|entity_type| utils::pluralize(entity_type) == field_name)
        .map(String::from)
}

/// Get entity type from singular field name (e.g., "order" -> "order")
fn get_entity_type_from_singular(host: &Arc<ServerHost>, field_name: &str) -> Option<String> {
    let catalog = host.catalog();
    catalog
        .entity_types()
        .into_iter()
        .find(|&entity_type| entity_type == field_name)
        .map(String::from)
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::super::core::GraphQLExecutor;
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::EntityFetcher;
    use crate::core::link::LinkDefinition;
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

    fn build_test_host(fetchers: HashMap<String, Arc<dyn EntityFetcher>>) -> Arc<ServerHost> {
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
            events: None,
            sinks: None,
            auth: None,
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
        assert!(
            order_result.is_object(),
            "singular query should return object"
        );
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

        let result = executor.execute("{ unknownEntity { id } }", None).await;
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

        assert_eq!(
            get_entity_type_from_plural(&host, "orders"),
            Some("order".to_string())
        );
        assert_eq!(
            get_entity_type_from_plural(&host, "invoices"),
            Some("invoice".to_string())
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
            Some("order".to_string())
        );
        assert_eq!(
            get_entity_type_from_singular(&host, "invoice"),
            Some("invoice".to_string())
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
