//! Mutation execution for GraphQL

use anyhow::{Result, bail};
use graphql_parser::query::Field;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use super::field_resolver;
use super::link_mutations;
use super::utils;
use crate::server::host::ServerHost;

/// Resolve a mutation field (e.g., "createOrder", "updateInvoice", etc.)
pub async fn resolve_mutation_field(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();

    // Check for createLink mutation (must be before create check)
    if field_name == "createLink" {
        return link_mutations::create_link_mutation(host, field).await;
    }

    // Check for createAndLink mutation (e.g., "createInvoiceForOrder") - must be before create check
    if field_name.starts_with("create") && field_name.contains("For") {
        return link_mutations::create_and_link_mutation(host, field).await;
    }

    // Check for link mutation (e.g., "linkInvoiceToOrder")
    if field_name.starts_with("link") && field_name.contains("To") {
        return link_mutations::link_entities_mutation(host, field).await;
    }

    // Check for unlink mutation (e.g., "unlinkInvoiceFromOrder")
    if field_name.starts_with("unlink") && field_name.contains("From") {
        return link_mutations::unlink_entities_mutation(host, field).await;
    }

    // Check for create mutation (e.g., "createOrder")
    if field_name.starts_with("create") {
        return create_entity_mutation(host, field).await;
    }

    // Check for update mutation (e.g., "updateOrder")
    if field_name.starts_with("update") {
        return update_entity_mutation(host, field).await;
    }

    // Check for delete mutation (e.g., "deleteOrder")
    if field_name.starts_with("delete") {
        return delete_entity_mutation(host, field).await;
    }

    // Check for deleteLink mutation
    if field_name == "deleteLink" {
        return link_mutations::delete_link_mutation(host, field).await;
    }

    bail!("Unknown mutation field: {}", field_name);
}

/// Create an entity
async fn create_entity_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();
    let entity_type = utils::mutation_name_to_entity_type(field_name, "create");

    // Get data argument
    let data = utils::get_json_arg(field, "data")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'data'"))?;

    // Create the entity
    if let Some(creator) = host.entity_creators.get(&entity_type) {
        let created = creator.create_from_json(data).await?;

        // Resolve sub-fields
        let resolved = field_resolver::resolve_entity_fields(
            host,
            created,
            &field.selection_set.items,
            &entity_type,
        )
        .await?;

        Ok(resolved)
    } else {
        bail!("Unknown entity type: {}", entity_type);
    }
}

/// Update an entity
async fn update_entity_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();
    let entity_type = utils::mutation_name_to_entity_type(field_name, "update");

    // Get arguments
    let id = utils::get_string_arg(field, "id")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
    let uuid = Uuid::parse_str(&id)?;
    let data = utils::get_json_arg(field, "data")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'data'"))?;

    // Update the entity
    if let Some(creator) = host.entity_creators.get(&entity_type) {
        let updated = creator.update_from_json(&uuid, data).await?;

        // Resolve sub-fields
        let resolved = field_resolver::resolve_entity_fields(
            host,
            updated,
            &field.selection_set.items,
            &entity_type,
        )
        .await?;

        Ok(resolved)
    } else {
        bail!("Unknown entity type: {}", entity_type);
    }
}

/// Delete an entity
async fn delete_entity_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();
    let entity_type = utils::mutation_name_to_entity_type(field_name, "delete");

    // Get ID argument
    let id = utils::get_string_arg(field, "id")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
    let uuid = Uuid::parse_str(&id)?;

    // Delete the entity
    if let Some(creator) = host.entity_creators.get(&entity_type) {
        creator.delete(&uuid).await?;
        Ok(Value::Bool(true))
    } else {
        bail!("Unknown entity type: {}", entity_type);
    }
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::link::LinkDefinition;
    use crate::core::{EntityCreator, EntityFetcher};
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
    use super::super::core::GraphQLExecutor;
    use crate::server::host::ServerHost;
    use crate::storage::in_memory::InMemoryLinkService;
    use async_trait::async_trait;
    use axum::Router;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::sync::Arc;
    use uuid::Uuid;

    // -----------------------------------------------------------------------
    // Mock infrastructure
    // -----------------------------------------------------------------------

    struct MockFetcher;

    #[async_trait]
    impl EntityFetcher for MockFetcher {
        async fn fetch_as_json(&self, _entity_id: &Uuid) -> anyhow::Result<Value> {
            Ok(json!({}))
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

    fn build_test_host() -> Arc<ServerHost> {
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

        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
        fetchers.insert("order".to_string(), Arc::new(MockFetcher));
        fetchers.insert("invoice".to_string(), Arc::new(MockFetcher));

        let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
        creators.insert("order".to_string(), Arc::new(MockCreator));
        creators.insert("invoice".to_string(), Arc::new(MockCreator));

        Arc::new(
            ServerHost::from_builder_components(link_service, config, registry, fetchers, creators)
                .expect("should build test host"),
        )
    }

    // -----------------------------------------------------------------------
    // Tests via GraphQLExecutor::execute
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_entity_mutation() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute(
                r#"mutation { createOrder(data: {name: "test"}) { id name } }"#,
                None,
            )
            .await
            .expect("createOrder should succeed");

        let created = result
            .get("data")
            .and_then(|d| d.get("createOrder"))
            .expect("should have createOrder");
        assert!(created.get("id").is_some(), "created entity should have id");
    }

    #[tokio::test]
    async fn test_create_entity_missing_data_returns_err() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute(r#"mutation { createOrder { id } }"#, None)
            .await;
        assert!(result.is_err(), "missing data should error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("data"),
            "error should mention 'data': {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_update_entity_mutation() {
        let order_id = Uuid::new_v4();
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let query = format!(
            r#"mutation {{ updateOrder(id: "{}", data: {{name: "updated"}}) {{ id name }} }}"#,
            order_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("updateOrder should succeed");

        let updated = result
            .get("data")
            .and_then(|d| d.get("updateOrder"))
            .expect("should have updateOrder");
        assert_eq!(
            updated.get("id").and_then(|v| v.as_str()),
            Some(order_id.to_string()).as_deref()
        );
    }

    #[tokio::test]
    async fn test_update_entity_missing_id_returns_err() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute(
                r#"mutation { updateOrder(data: {name: "updated"}) { id } }"#,
                None,
            )
            .await;
        assert!(result.is_err(), "missing id should error");
    }

    #[tokio::test]
    async fn test_delete_entity_mutation() {
        let order_id = Uuid::new_v4();
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let query = format!(r#"mutation {{ deleteOrder(id: "{}") }}"#, order_id);
        let result = executor
            .execute(&query, None)
            .await
            .expect("deleteOrder should succeed");

        let deleted = result
            .get("data")
            .and_then(|d| d.get("deleteOrder"))
            .expect("should have deleteOrder");
        assert_eq!(*deleted, Value::Bool(true));
    }

    #[tokio::test]
    async fn test_delete_entity_missing_id_returns_err() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute(r#"mutation { deleteOrder }"#, None)
            .await;
        assert!(result.is_err(), "missing id should error");
    }

    #[tokio::test]
    async fn test_unknown_mutation_returns_err() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute(r#"mutation { doSomethingWeird { id } }"#, None)
            .await;
        assert!(result.is_err(), "unknown mutation should error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("Unknown mutation field"),
            "should mention unknown mutation: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_create_unknown_entity_type_returns_err() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute(
                r#"mutation { createWidget(data: {name: "w"}) { id } }"#,
                None,
            )
            .await;
        assert!(result.is_err(), "unknown entity type should error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("Unknown entity type"),
            "should mention unknown entity: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_create_link_mutation_dispatches() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ createLink(sourceId: "{}", targetId: "{}", linkType: "has_invoice") {{ id }} }}"#,
            source_id, target_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("createLink should succeed");

        let link_result = result
            .get("data")
            .and_then(|d| d.get("createLink"))
            .expect("should have createLink");
        assert!(link_result.get("id").is_some(), "link should have id");
    }

    #[tokio::test]
    async fn test_unlink_entities_mutation_dispatches() {
        let host = build_test_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ unlinkInvoiceFromOrder(sourceId: "{}", targetId: "{}") }}"#,
            source_id, target_id
        );
        // unlinkInvoiceFromOrder -> source_type = "invoice", target_type = "order"
        // No matching link in store -> should return false
        let result = executor
            .execute(&query, None)
            .await
            .expect("unlink should succeed");

        let unlink_result = result
            .get("data")
            .and_then(|d| d.get("unlinkInvoiceFromOrder"))
            .expect("should have unlink result");
        assert_eq!(*unlink_result, Value::Bool(false), "should return false when no link found");
    }
}
