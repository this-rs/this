//! Link-specific mutations for GraphQL

use anyhow::{Result, bail};
use graphql_parser::query::Field;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

use super::field_resolver;
use super::utils;
use crate::core::link::LinkEntity;
use crate::server::host::ServerHost;

/// Create a link between two existing entities
pub async fn create_link_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    // Get arguments
    let source_id = utils::get_string_arg(field, "sourceId")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'sourceId'"))?;
    let target_id = utils::get_string_arg(field, "targetId")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'targetId'"))?;
    let link_type = utils::get_string_arg(field, "linkType")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'linkType'"))?;

    let source_uuid = Uuid::parse_str(&source_id)?;
    let target_uuid = Uuid::parse_str(&target_id)?;

    // Get optional metadata
    let metadata = utils::get_json_arg(field, "metadata");

    // Create the link
    let link_entity = LinkEntity::new(link_type, source_uuid, target_uuid, metadata);
    let created_link = host.link_service.create(link_entity).await?;

    // Return the created link as JSON
    Ok(json!({
        "id": created_link.id.to_string(),
        "sourceId": created_link.source_id.to_string(),
        "targetId": created_link.target_id.to_string(),
        "linkType": created_link.link_type,
        "metadata": created_link.metadata,
        "createdAt": created_link.created_at.to_rfc3339(),
    }))
}

/// Delete a link by ID
pub async fn delete_link_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let link_id = utils::get_string_arg(field, "id")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
    let uuid = Uuid::parse_str(&link_id)?;

    host.link_service.delete(&uuid).await?;
    Ok(Value::Bool(true))
}

/// Create an entity and link it to another entity (e.g., createInvoiceForOrder)
pub async fn create_and_link_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();

    // Parse field name: createInvoiceForOrder -> (invoice, order)
    let parts: Vec<&str> = field_name
        .strip_prefix("create")
        .unwrap_or("")
        .split("For")
        .collect();

    if parts.len() != 2 {
        bail!("Invalid createAndLink mutation format: {}", field_name);
    }

    let entity_type = utils::pascal_to_snake(parts[0]);
    let parent_type = utils::pascal_to_snake(parts[1]);

    // Get arguments
    let parent_id = utils::get_string_arg(field, "parentId")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'parentId'"))?;
    let data = utils::get_json_arg(field, "data")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'data'"))?;
    let link_type = utils::get_string_arg(field, "linkType");

    let parent_uuid = Uuid::parse_str(&parent_id)?;

    // Create the entity
    if let Some(creator) = host.entity_creators.get(&entity_type) {
        let created = creator.create_from_json(data).await?;

        // Extract the new entity's ID
        let entity_id = created
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Created entity missing id field"))?;
        let entity_uuid = Uuid::parse_str(entity_id)?;

        // Find the appropriate link type from config
        let actual_link_type = if let Some(lt) = link_type {
            lt
        } else {
            // Try to find link type from config
            utils::find_link_type(&host.config.links, &parent_type, &entity_type)?
        };

        // Create the link
        let link_entity = LinkEntity::new(actual_link_type, parent_uuid, entity_uuid, None);
        host.link_service.create(link_entity).await?;

        // Resolve sub-fields for the created entity
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

/// Link two existing entities (e.g., linkInvoiceToOrder)
pub async fn link_entities_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();

    // Parse field name: linkInvoiceToOrder -> (invoice, order)
    let parts: Vec<&str> = field_name
        .strip_prefix("link")
        .unwrap_or("")
        .split("To")
        .collect();

    if parts.len() != 2 {
        bail!("Invalid link mutation format: {}", field_name);
    }

    let source_type = utils::pascal_to_snake(parts[0]);
    let target_type = utils::pascal_to_snake(parts[1]);

    // Get arguments
    let source_id = utils::get_string_arg(field, "sourceId")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'sourceId'"))?;
    let target_id = utils::get_string_arg(field, "targetId")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'targetId'"))?;
    let link_type = utils::get_string_arg(field, "linkType");

    let source_uuid = Uuid::parse_str(&source_id)?;
    let target_uuid = Uuid::parse_str(&target_id)?;

    // Find the appropriate link type from config
    let actual_link_type = if let Some(lt) = link_type {
        lt
    } else {
        utils::find_link_type(&host.config.links, &source_type, &target_type)?
    };

    // Get optional metadata
    let metadata = utils::get_json_arg(field, "metadata");

    // Create the link
    let link_entity = LinkEntity::new(actual_link_type, source_uuid, target_uuid, metadata);
    let created_link = host.link_service.create(link_entity).await?;

    // Return the created link
    Ok(json!({
        "id": created_link.id.to_string(),
        "sourceId": created_link.source_id.to_string(),
        "targetId": created_link.target_id.to_string(),
        "linkType": created_link.link_type,
        "metadata": created_link.metadata,
        "createdAt": created_link.created_at.to_rfc3339(),
    }))
}

/// Unlink two entities (e.g., unlinkInvoiceFromOrder)
pub async fn unlink_entities_mutation(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    let field_name = field.name.as_str();

    // Parse field name: unlinkInvoiceFromOrder -> (invoice, order)
    let parts: Vec<&str> = field_name
        .strip_prefix("unlink")
        .unwrap_or("")
        .split("From")
        .collect();

    if parts.len() != 2 {
        bail!("Invalid unlink mutation format: {}", field_name);
    }

    let source_type = utils::pascal_to_snake(parts[0]);
    let target_type = utils::pascal_to_snake(parts[1]);

    // Get arguments
    let source_id = utils::get_string_arg(field, "sourceId")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'sourceId'"))?;
    let target_id = utils::get_string_arg(field, "targetId")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument 'targetId'"))?;
    let link_type = utils::get_string_arg(field, "linkType");

    let source_uuid = Uuid::parse_str(&source_id)?;
    let target_uuid = Uuid::parse_str(&target_id)?;

    // Find the appropriate link type from config
    let actual_link_type = if let Some(lt) = link_type {
        Some(lt)
    } else {
        utils::find_link_type(&host.config.links, &source_type, &target_type).ok()
    };

    // Find and delete the link
    let links = host
        .link_service
        .find_by_source(
            &source_uuid,
            actual_link_type.as_deref(),
            Some(&target_type),
        )
        .await?;

    for link in links {
        if link.target_id == target_uuid {
            host.link_service.delete(&link.id).await?;
            return Ok(Value::Bool(true));
        }
    }

    Ok(Value::Bool(false))
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::super::core::GraphQLExecutor;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::core::link::{LinkDefinition, LinkEntity};
    use crate::core::service::LinkService;
    use crate::core::{EntityCreator, EntityFetcher};
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
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

    fn build_test_host_with_link_service(
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

    fn default_host() -> (Arc<ServerHost>, Arc<InMemoryLinkService>) {
        let link_service = Arc::new(InMemoryLinkService::new());
        let host = build_test_host_with_link_service(link_service.clone());
        (host, link_service)
    }

    // -----------------------------------------------------------------------
    // create_link_mutation tests (via executor)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_link_mutation_success() {
        let (host, _) = default_host();
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
            .expect("should create link");

        let link_result = result
            .get("data")
            .and_then(|d| d.get("createLink"))
            .expect("should have createLink");
        assert!(link_result.get("id").is_some(), "should have id");
    }

    #[tokio::test]
    async fn test_create_link_mutation_missing_source_id() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ createLink(targetId: "{}", linkType: "has_invoice") {{ id }} }}"#,
            target_id
        );
        let result = executor.execute(&query, None).await;
        assert!(result.is_err(), "missing sourceId should error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("sourceId"),
            "should mention sourceId: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_create_link_mutation_missing_link_type() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ createLink(sourceId: "{}", targetId: "{}") {{ id }} }}"#,
            source_id, target_id
        );
        let result = executor.execute(&query, None).await;
        assert!(result.is_err(), "missing linkType should error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("linkType"),
            "should mention linkType: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // delete_link_mutation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_delete_link_mutation_direct_call() {
        // Note: deleteLink via executor gets caught by the "delete*" prefix check
        // and routes to delete_entity_mutation instead of delete_link_mutation.
        // So we test delete_link_mutation directly using a manually constructed Field.
        use super::delete_link_mutation;
        use graphql_parser::Pos;
        use graphql_parser::query::{SelectionSet, Value as GqlValue};

        let (host, link_service) = default_host();

        // Create a link first
        let link = LinkEntity::new("has_invoice", Uuid::new_v4(), Uuid::new_v4(), None);
        let created = link_service.create(link).await.expect("should create link");

        let pos = Pos { line: 1, column: 1 };
        let field = graphql_parser::query::Field {
            position: pos,
            alias: None,
            name: "deleteLink".to_string(),
            arguments: vec![("id".to_string(), GqlValue::String(created.id.to_string()))],
            directives: vec![],
            selection_set: SelectionSet {
                span: (pos, pos),
                items: vec![],
            },
        };

        let result = delete_link_mutation(&host, &field)
            .await
            .expect("should delete link");
        assert_eq!(result, Value::Bool(true));
    }

    #[tokio::test]
    async fn test_delete_link_mutation_missing_id_direct_call() {
        use super::delete_link_mutation;
        use graphql_parser::Pos;
        use graphql_parser::query::SelectionSet;

        let (host, _) = default_host();

        let pos = Pos { line: 1, column: 1 };
        let field = graphql_parser::query::Field {
            position: pos,
            alias: None,
            name: "deleteLink".to_string(),
            arguments: vec![],
            directives: vec![],
            selection_set: SelectionSet {
                span: (pos, pos),
                items: vec![],
            },
        };

        let result = delete_link_mutation(&host, &field).await;
        assert!(result.is_err(), "missing id should error");
    }

    // -----------------------------------------------------------------------
    // create_and_link_mutation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_and_link_mutation_success() {
        let (host, link_service) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let parent_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ createInvoiceForOrder(parentId: "{}", data: {{amount: 100}}) {{ id }} }}"#,
            parent_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should create and link");

        let created = result
            .get("data")
            .and_then(|d| d.get("createInvoiceForOrder"))
            .expect("should have createInvoiceForOrder");
        assert!(created.get("id").is_some(), "created entity should have id");

        // Verify the link was created
        let links = link_service
            .find_by_source(&parent_id, Some("has_invoice"), None)
            .await
            .expect("should find links");
        assert_eq!(links.len(), 1, "should have one link from parent");
    }

    #[tokio::test]
    async fn test_create_and_link_mutation_missing_parent_id() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;

        let result = executor
            .execute(
                r#"mutation { createInvoiceForOrder(data: {amount: 100}) { id } }"#,
                None,
            )
            .await;
        assert!(result.is_err(), "missing parentId should error");
        let err_msg = result.expect_err("error").to_string();
        assert!(
            err_msg.contains("parentId"),
            "should mention parentId: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_create_and_link_mutation_missing_data() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let parent_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ createInvoiceForOrder(parentId: "{}") {{ id }} }}"#,
            parent_id
        );
        let result = executor.execute(&query, None).await;
        assert!(result.is_err(), "missing data should error");
    }

    #[tokio::test]
    async fn test_create_and_link_mutation_unknown_entity_type() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let parent_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ createWidgetForGadget(parentId: "{}", data: {{name: "w"}}) {{ id }} }}"#,
            parent_id
        );
        let result = executor.execute(&query, None).await;
        assert!(result.is_err(), "unknown entity type should error");
    }

    #[tokio::test]
    async fn test_create_and_link_mutation_with_explicit_link_type() {
        let (host, link_service) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let parent_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ createInvoiceForOrder(parentId: "{}", data: {{amount: 200}}, linkType: "has_invoice") {{ id }} }}"#,
            parent_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should succeed with explicit linkType");

        let created = result
            .get("data")
            .and_then(|d| d.get("createInvoiceForOrder"))
            .expect("should have result");
        assert!(created.get("id").is_some());

        let links = link_service
            .find_by_source(&parent_id, Some("has_invoice"), None)
            .await
            .expect("should find links");
        assert_eq!(links.len(), 1);
    }

    // -----------------------------------------------------------------------
    // link_entities_mutation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_link_entities_mutation_success() {
        let (host, link_service) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        // linkOrderToInvoice -> source_type="order", target_type="invoice" -> matches config
        let query = format!(
            r#"mutation {{ linkOrderToInvoice(sourceId: "{}", targetId: "{}") {{ id }} }}"#,
            source_id, target_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should link entities");

        let link_result = result
            .get("data")
            .and_then(|d| d.get("linkOrderToInvoice"))
            .expect("should have result");
        assert!(link_result.get("id").is_some(), "link should have id");

        // Verify link in storage
        let links = link_service
            .find_by_source(&source_id, Some("has_invoice"), None)
            .await
            .expect("should find links");
        assert_eq!(links.len(), 1);
    }

    #[tokio::test]
    async fn test_link_entities_mutation_missing_source_id() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ linkOrderToInvoice(targetId: "{}") {{ id }} }}"#,
            target_id
        );
        let result = executor.execute(&query, None).await;
        assert!(result.is_err(), "missing sourceId should error");
    }

    #[tokio::test]
    async fn test_link_entities_mutation_with_explicit_link_type() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ linkOrderToInvoice(sourceId: "{}", targetId: "{}", linkType: "has_invoice") {{ id }} }}"#,
            source_id, target_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should succeed with explicit linkType");

        let link_result = result
            .get("data")
            .and_then(|d| d.get("linkOrderToInvoice"))
            .expect("should have result");
        assert!(link_result.get("id").is_some());
    }

    // -----------------------------------------------------------------------
    // unlink_entities_mutation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_unlink_entities_mutation_found_and_deleted() {
        let (host, link_service) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        // Create a link first
        let link = LinkEntity::new("has_invoice", source_id, target_id, None);
        link_service.create(link).await.expect("should create link");

        // unlinkOrderFromInvoice -> source_type="order", target_type="invoice"
        // find_link_type("order", "invoice") -> "has_invoice"
        // find_by_source(source_id, "has_invoice", "invoice")
        let query = format!(
            r#"mutation {{ unlinkOrderFromInvoice(sourceId: "{}", targetId: "{}") }}"#,
            source_id, target_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should succeed");

        let unlink_result = result
            .get("data")
            .and_then(|d| d.get("unlinkOrderFromInvoice"))
            .expect("should have result");
        assert_eq!(
            *unlink_result,
            Value::Bool(true),
            "should return true when link found and deleted"
        );
    }

    #[tokio::test]
    async fn test_unlink_entities_mutation_no_link_found() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ unlinkOrderFromInvoice(sourceId: "{}", targetId: "{}") }}"#,
            source_id, target_id
        );
        let result = executor
            .execute(&query, None)
            .await
            .expect("should succeed even without link");

        let unlink_result = result
            .get("data")
            .and_then(|d| d.get("unlinkOrderFromInvoice"))
            .expect("should have result");
        assert_eq!(
            *unlink_result,
            Value::Bool(false),
            "should return false when no link found"
        );
    }

    #[tokio::test]
    async fn test_unlink_entities_mutation_missing_source_id() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let target_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ unlinkOrderFromInvoice(targetId: "{}") }}"#,
            target_id
        );
        let result = executor.execute(&query, None).await;
        assert!(result.is_err(), "missing sourceId should error");
    }

    #[tokio::test]
    async fn test_unlink_entities_mutation_missing_target_id() {
        let (host, _) = default_host();
        let executor = GraphQLExecutor::new(host).await;
        let source_id = Uuid::new_v4();

        let query = format!(
            r#"mutation {{ unlinkOrderFromInvoice(sourceId: "{}") }}"#,
            source_id
        );
        let result = executor.execute(&query, None).await;
        assert!(result.is_err(), "missing targetId should error");
    }
}
