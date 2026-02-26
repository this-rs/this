//! Dynamic GraphQL schema generator
//!
//! This module generates GraphQL schemas automatically from registered entities
//! without any hardcoded entity types.

#[cfg(feature = "graphql")]
use crate::server::host::ServerHost;
#[cfg(feature = "graphql")]
use serde_json::Value;
#[cfg(feature = "graphql")]
use std::sync::Arc;

#[cfg(feature = "graphql")]
/// Information about a GraphQL field
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub graphql_type: String,
    pub nullable: bool,
    #[allow(dead_code)]
    pub description: Option<String>,
}

#[cfg(feature = "graphql")]
/// Information about a relation between entities
#[derive(Debug, Clone)]
pub struct RelationInfo {
    pub name: String,
    pub target_type: String,
    pub is_list: bool,
    #[allow(dead_code)]
    pub link_type: String,
}

#[cfg(feature = "graphql")]
/// Schema generator that creates GraphQL SDL from ServerHost
pub struct SchemaGenerator {
    host: Arc<ServerHost>,
}

#[cfg(feature = "graphql")]
impl SchemaGenerator {
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self { host }
    }

    /// Generate the complete SDL schema
    pub async fn generate_sdl(&self) -> String {
        let mut sdl = String::new();

        // Generate entity types
        for entity_type in self.host.entity_types() {
            if let Ok(type_def) = self.generate_entity_type(entity_type).await {
                sdl.push_str(&type_def);
                sdl.push_str("\n\n");
            }
        }

        // Generate Query root
        sdl.push_str(&self.generate_query_root());
        sdl.push_str("\n\n");

        // Generate Mutation root
        sdl.push_str(&self.generate_mutation_root());
        sdl.push_str("\n\n");

        // Add schema definition
        sdl.push_str("schema {\n");
        sdl.push_str("  query: Query\n");
        sdl.push_str("  mutation: Mutation\n");
        sdl.push_str("}\n");

        sdl
    }

    /// Generate a GraphQL type for an entity
    async fn generate_entity_type(&self, entity_type: &str) -> anyhow::Result<String> {
        let type_name = Self::to_pascal_case(entity_type);
        let mut type_def = format!("type {} {{\n", type_name);

        // Get fields from a sample entity or from listing entities
        if let Some(fetcher) = self.host.entity_fetchers.get(entity_type) {
            // Try to get a sample entity first
            let mut sample = fetcher.get_sample_entity().await?;

            // If sample is empty, try to get the first entity from the list
            if sample.as_object().is_none_or(|obj| obj.is_empty())
                && let Ok(entities) = fetcher.list_as_json(Some(1), None).await
                && let Some(first_entity) = entities.first()
            {
                sample = first_entity.clone();
            }

            let fields = Self::extract_fields_from_json(&sample);

            for field in fields {
                let nullable = if field.nullable { "" } else { "!" };
                type_def.push_str(&format!(
                    "  {}: {}{}\n",
                    field.name, field.graphql_type, nullable
                ));
            }
        }

        // Add relations from links config
        let relations = self.get_relations_for(entity_type);
        for relation in relations {
            let target_type = Self::to_pascal_case(&relation.target_type);
            if relation.is_list {
                type_def.push_str(&format!("  {}: [{}!]!\n", relation.name, target_type));
            } else {
                type_def.push_str(&format!("  {}: {}\n", relation.name, target_type));
            }
        }

        type_def.push('}');
        Ok(type_def)
    }

    /// Generate the Query root type
    fn generate_query_root(&self) -> String {
        let mut query = String::from("type Query {\n");

        for entity_type in self.host.entity_types() {
            let type_name = Self::to_pascal_case(entity_type);
            let singular = entity_type;
            let plural = self.get_plural(entity_type);

            // Single query: order(id: ID!): Order
            query.push_str(&format!("  {}(id: ID!): {}\n", singular, type_name));

            // List query: orders(limit: Int, offset: Int): [Order!]!
            query.push_str(&format!(
                "  {}(limit: Int, offset: Int): [{}!]!\n",
                plural, type_name
            ));
        }

        query.push('}');
        query
    }

    /// Generate the Mutation root type
    fn generate_mutation_root(&self) -> String {
        let mut mutation = String::from("type Mutation {\n");

        for entity_type in self.host.entity_types() {
            let type_name = Self::to_pascal_case(entity_type);

            // CREATE: createOrder(data: JSON!): Order!
            mutation.push_str(&format!(
                "  create{}(data: JSON!): {}!\n",
                type_name, type_name
            ));

            // UPDATE: updateOrder(id: ID!, data: JSON!): Order!
            mutation.push_str(&format!(
                "  update{}(id: ID!, data: JSON!): {}!\n",
                type_name, type_name
            ));

            // DELETE: deleteOrder(id: ID!): Boolean!
            mutation.push_str(&format!("  delete{}(id: ID!): Boolean!\n", type_name));
        }

        // Add generic link mutations
        mutation.push_str("\n  # Generic link mutations\n");
        mutation.push_str("  createLink(sourceId: ID!, targetId: ID!, linkType: String!, metadata: JSON): Link!\n");
        mutation.push_str("  deleteLink(id: ID!): Boolean!\n");

        // Add typed link mutations for each entity combination
        mutation.push_str("\n  # Typed link mutations\n");
        for link_config in &self.host.config.links {
            let source_type = Self::to_pascal_case(&link_config.source_type);
            let target_type = Self::to_pascal_case(&link_config.target_type);

            // createInvoiceForOrder(parentId: ID!, data: JSON!): Invoice!
            mutation.push_str(&format!(
                "  create{}For{}(parentId: ID!, data: JSON!, linkType: String): {}!\n",
                target_type, source_type, target_type
            ));

            // linkInvoiceToOrder(sourceId: ID!, targetId: ID!, metadata: JSON): Link!
            mutation.push_str(&format!(
                "  link{}To{}(sourceId: ID!, targetId: ID!, linkType: String, metadata: JSON): Link!\n",
                target_type, source_type
            ));

            // unlinkInvoiceFromOrder(sourceId: ID!, targetId: ID!): Boolean!
            mutation.push_str(&format!(
                "  unlink{}From{}(sourceId: ID!, targetId: ID!, linkType: String): Boolean!\n",
                target_type, source_type
            ));
        }

        mutation.push('}');
        mutation
    }

    /// Extract fields from a JSON sample
    fn extract_fields_from_json(json: &Value) -> Vec<FieldInfo> {
        let mut fields = Vec::new();

        if let Value::Object(map) = json {
            for (key, value) in map {
                // Skip the 'host' field if present
                if key == "host" {
                    continue;
                }

                fields.push(FieldInfo {
                    name: key.clone(),
                    graphql_type: Self::json_type_to_graphql(value),
                    nullable: value.is_null(),
                    description: None,
                });
            }
        }

        fields
    }

    /// Convert JSON type to GraphQL type
    fn json_type_to_graphql(value: &Value) -> String {
        match value {
            Value::String(_) => "String",
            Value::Number(n) if n.is_f64() => "Float",
            Value::Number(_) => "Int",
            Value::Bool(_) => "Boolean",
            Value::Array(_) => "[String]",
            Value::Object(_) => "JSON",
            Value::Null => "String",
        }
        .to_string()
    }

    /// Get relations for an entity from links config
    fn get_relations_for(&self, entity_type: &str) -> Vec<RelationInfo> {
        let mut relations = Vec::new();

        for link_def in &self.host.config.links {
            // Forward relation: order -> invoices
            if link_def.source_type == entity_type {
                relations.push(RelationInfo {
                    name: link_def.forward_route_name.clone(),
                    target_type: link_def.target_type.clone(),
                    is_list: true,
                    link_type: link_def.link_type.clone(),
                });
            }

            // Reverse relation: invoice -> order
            if link_def.target_type == entity_type {
                relations.push(RelationInfo {
                    name: link_def.reverse_route_name.clone(),
                    target_type: link_def.source_type.clone(),
                    is_list: false,
                    link_type: link_def.link_type.clone(),
                });
            }
        }

        relations
    }

    /// Get plural form of entity type
    fn get_plural(&self, entity_type: &str) -> String {
        self.host
            .config
            .entities
            .iter()
            .find(|e| e.singular == entity_type)
            .map(|e| e.plural.clone())
            .unwrap_or_else(|| format!("{}s", entity_type))
    }

    /// Convert snake_case to PascalCase
    fn to_pascal_case(s: &str) -> String {
        s.split('_')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "graphql")]
    use super::*;

    // ---- to_pascal_case tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_to_pascal_case_snake_case() {
        assert_eq!(SchemaGenerator::to_pascal_case("order_item"), "OrderItem");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_to_pascal_case_single_word() {
        assert_eq!(SchemaGenerator::to_pascal_case("order"), "Order");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_to_pascal_case_empty() {
        assert_eq!(SchemaGenerator::to_pascal_case(""), "");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_to_pascal_case_multiple_underscores() {
        assert_eq!(
            SchemaGenerator::to_pascal_case("user_order_item"),
            "UserOrderItem"
        );
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_to_pascal_case_trailing_underscore() {
        // trailing underscore produces an empty segment
        assert_eq!(SchemaGenerator::to_pascal_case("order_"), "Order");
    }

    // ---- json_type_to_graphql tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_json_type_to_graphql_string() {
        let val = Value::String("hello".to_string());
        assert_eq!(SchemaGenerator::json_type_to_graphql(&val), "String");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_json_type_to_graphql_int() {
        let val = serde_json::json!(42);
        assert_eq!(SchemaGenerator::json_type_to_graphql(&val), "Int");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_json_type_to_graphql_float() {
        let val = serde_json::json!(3.15);
        assert_eq!(SchemaGenerator::json_type_to_graphql(&val), "Float");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_json_type_to_graphql_bool() {
        let val = serde_json::json!(true);
        assert_eq!(SchemaGenerator::json_type_to_graphql(&val), "Boolean");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_json_type_to_graphql_array() {
        let val = serde_json::json!([1, 2, 3]);
        assert_eq!(SchemaGenerator::json_type_to_graphql(&val), "[String]");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_json_type_to_graphql_object() {
        let val = serde_json::json!({"key": "value"});
        assert_eq!(SchemaGenerator::json_type_to_graphql(&val), "JSON");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_json_type_to_graphql_null() {
        let val = Value::Null;
        assert_eq!(SchemaGenerator::json_type_to_graphql(&val), "String");
    }

    // ---- extract_fields_from_json tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_extract_fields_from_json_flat_object() {
        let json = serde_json::json!({
            "id": "abc-123",
            "name": "Alice",
            "active": true
        });
        let fields = SchemaGenerator::extract_fields_from_json(&json);
        assert_eq!(fields.len(), 3);

        let id_field = fields
            .iter()
            .find(|f| f.name == "id")
            .expect("should have 'id' field");
        assert_eq!(id_field.graphql_type, "String");
        assert!(!id_field.nullable);

        let active_field = fields
            .iter()
            .find(|f| f.name == "active")
            .expect("should have 'active' field");
        assert_eq!(active_field.graphql_type, "Boolean");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_extract_fields_from_json_nested_object() {
        let json = serde_json::json!({
            "id": "abc",
            "metadata": {"key": "value"}
        });
        let fields = SchemaGenerator::extract_fields_from_json(&json);
        let meta_field = fields
            .iter()
            .find(|f| f.name == "metadata")
            .expect("should have 'metadata' field");
        assert_eq!(meta_field.graphql_type, "JSON");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_extract_fields_from_json_skips_host() {
        let json = serde_json::json!({
            "id": "abc",
            "host": "should-be-skipped",
            "name": "test"
        });
        let fields = SchemaGenerator::extract_fields_from_json(&json);
        assert_eq!(fields.len(), 2);
        assert!(
            fields.iter().all(|f| f.name != "host"),
            "host field should be skipped"
        );
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_extract_fields_from_json_null_field_is_nullable() {
        let json = serde_json::json!({
            "id": "abc",
            "deleted_at": null
        });
        let fields = SchemaGenerator::extract_fields_from_json(&json);
        let deleted_field = fields
            .iter()
            .find(|f| f.name == "deleted_at")
            .expect("should have 'deleted_at' field");
        assert!(deleted_field.nullable);
        assert_eq!(deleted_field.graphql_type, "String");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_extract_fields_from_json_non_object() {
        // When input is not an object, should return empty vec
        let json = serde_json::json!("just a string");
        let fields = SchemaGenerator::extract_fields_from_json(&json);
        assert!(fields.is_empty());
    }

    // -----------------------------------------------------------------------
    // Integration helpers: mock host infrastructure
    // -----------------------------------------------------------------------

    #[cfg(feature = "graphql")]
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    #[cfg(feature = "graphql")]
    use crate::core::EntityFetcher;
    #[cfg(feature = "graphql")]
    use crate::core::link::LinkDefinition;
    #[cfg(feature = "graphql")]
    use crate::server::entity_registry::{EntityDescriptor, EntityRegistry};
    #[cfg(feature = "graphql")]
    use crate::server::host::ServerHost;
    #[cfg(feature = "graphql")]
    use crate::storage::in_memory::InMemoryLinkService;
    #[cfg(feature = "graphql")]
    use async_trait::async_trait;
    #[cfg(feature = "graphql")]
    use axum::Router;
    #[cfg(feature = "graphql")]
    use std::collections::HashMap;
    #[cfg(feature = "graphql")]
    use uuid::Uuid;

    #[cfg(feature = "graphql")]
    struct MockFetcher {
        sample: Value,
    }

    #[cfg(feature = "graphql")]
    impl MockFetcher {
        fn with_sample(sample: Value) -> Self {
            Self { sample }
        }
    }

    #[cfg(feature = "graphql")]
    #[async_trait]
    impl EntityFetcher for MockFetcher {
        async fn fetch_as_json(&self, _entity_id: &Uuid) -> anyhow::Result<Value> {
            Ok(self.sample.clone())
        }

        async fn get_sample_entity(&self) -> anyhow::Result<Value> {
            Ok(self.sample.clone())
        }
    }

    #[cfg(feature = "graphql")]
    struct StubDescriptor {
        entity_type: String,
        plural: String,
    }

    #[cfg(feature = "graphql")]
    impl StubDescriptor {
        fn new(singular: &str, plural: &str) -> Self {
            Self {
                entity_type: singular.to_string(),
                plural: plural.to_string(),
            }
        }
    }

    #[cfg(feature = "graphql")]
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

    type EntityEntry<'a> = (&'a str, &'a str, Option<Arc<dyn EntityFetcher>>);

    #[cfg(feature = "graphql")]
    fn build_host_with_links(
        entities: Vec<EntityEntry<'_>>,
        links: Vec<LinkDefinition>,
    ) -> Arc<ServerHost> {
        let link_service = Arc::new(InMemoryLinkService::new());
        let entity_configs: Vec<EntityConfig> = entities
            .iter()
            .map(|(singular, plural, _)| EntityConfig {
                singular: singular.to_string(),
                plural: plural.to_string(),
                auth: EntityAuthConfig::default(),
            })
            .collect();

        let mut registry = EntityRegistry::new();
        let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();

        for (singular, plural, fetcher) in &entities {
            registry.register(Box::new(StubDescriptor::new(singular, plural)));
            if let Some(f) = fetcher {
                fetchers.insert(singular.to_string(), f.clone());
            }
        }

        let config = LinksConfig {
            entities: entity_configs,
            links,
            validation_rules: None,
        };

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

    // -----------------------------------------------------------------------
    // get_relations_for tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_relations_for_forward() {
        let link = LinkDefinition {
            link_type: "has_invoice".to_string(),
            source_type: "order".to_string(),
            target_type: "invoice".to_string(),
            forward_route_name: "invoices".to_string(),
            reverse_route_name: "order".to_string(),
            description: None,
            required_fields: None,
            auth: None,
        };

        let host = build_host_with_links(
            vec![("order", "orders", None), ("invoice", "invoices", None)],
            vec![link],
        );
        let generator = SchemaGenerator::new(host);

        let rels = generator.get_relations_for("order");
        assert_eq!(rels.len(), 1, "order should have one forward relation");
        assert_eq!(rels[0].name, "invoices");
        assert_eq!(rels[0].target_type, "invoice");
        assert!(rels[0].is_list, "forward relation should be a list");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_relations_for_reverse() {
        let link = LinkDefinition {
            link_type: "has_invoice".to_string(),
            source_type: "order".to_string(),
            target_type: "invoice".to_string(),
            forward_route_name: "invoices".to_string(),
            reverse_route_name: "order".to_string(),
            description: None,
            required_fields: None,
            auth: None,
        };

        let host = build_host_with_links(
            vec![("order", "orders", None), ("invoice", "invoices", None)],
            vec![link],
        );
        let generator = SchemaGenerator::new(host);

        let rels = generator.get_relations_for("invoice");
        assert_eq!(rels.len(), 1, "invoice should have one reverse relation");
        assert_eq!(rels[0].name, "order");
        assert_eq!(rels[0].target_type, "order");
        assert!(!rels[0].is_list, "reverse relation should not be a list");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_relations_for_both_directions() {
        // Entity that is both source and target
        let link1 = LinkDefinition {
            link_type: "has_invoice".to_string(),
            source_type: "order".to_string(),
            target_type: "invoice".to_string(),
            forward_route_name: "invoices".to_string(),
            reverse_route_name: "parent_order".to_string(),
            description: None,
            required_fields: None,
            auth: None,
        };
        let link2 = LinkDefinition {
            link_type: "has_payment".to_string(),
            source_type: "invoice".to_string(),
            target_type: "payment".to_string(),
            forward_route_name: "payments".to_string(),
            reverse_route_name: "parent_invoice".to_string(),
            description: None,
            required_fields: None,
            auth: None,
        };

        let host = build_host_with_links(
            vec![
                ("order", "orders", None),
                ("invoice", "invoices", None),
                ("payment", "payments", None),
            ],
            vec![link1, link2],
        );
        let generator = SchemaGenerator::new(host);

        let rels = generator.get_relations_for("invoice");
        assert_eq!(
            rels.len(),
            2,
            "invoice should have both forward and reverse relations"
        );
        let names: Vec<&str> = rels.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"parent_order"),
            "should have reverse to order"
        );
        assert!(
            names.contains(&"payments"),
            "should have forward to payment"
        );
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_relations_for_none() {
        let host = build_host_with_links(vec![("order", "orders", None)], vec![]);
        let generator = SchemaGenerator::new(host);

        let rels = generator.get_relations_for("order");
        assert!(rels.is_empty(), "no links means no relations");
    }

    // -----------------------------------------------------------------------
    // get_plural tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_plural_known_entity() {
        let host = build_host_with_links(vec![("order", "orders", None)], vec![]);
        let generator = SchemaGenerator::new(host);
        assert_eq!(generator.get_plural("order"), "orders");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_plural_unknown_entity_uses_fallback() {
        let host = build_host_with_links(vec![("order", "orders", None)], vec![]);
        let generator = SchemaGenerator::new(host);
        assert_eq!(generator.get_plural("widget"), "widgets");
    }

    // -----------------------------------------------------------------------
    // generate_query_root tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "graphql")]
    #[test]
    fn test_generate_query_root_contains_singular_and_plural() {
        let host = build_host_with_links(
            vec![("order", "orders", None), ("invoice", "invoices", None)],
            vec![],
        );
        let generator = SchemaGenerator::new(host);
        let query_root = generator.generate_query_root();

        assert!(
            query_root.contains("type Query {"),
            "should start with type Query"
        );
        assert!(
            query_root.contains("order(id: ID!): Order"),
            "should have singular query"
        );
        assert!(
            query_root.contains("orders(limit: Int, offset: Int): [Order!]!"),
            "should have plural query"
        );
        assert!(
            query_root.contains("invoice(id: ID!): Invoice"),
            "should have invoice singular query"
        );
    }

    // -----------------------------------------------------------------------
    // generate_mutation_root tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "graphql")]
    #[test]
    fn test_generate_mutation_root_contains_crud() {
        let link = LinkDefinition {
            link_type: "has_invoice".to_string(),
            source_type: "order".to_string(),
            target_type: "invoice".to_string(),
            forward_route_name: "invoices".to_string(),
            reverse_route_name: "order".to_string(),
            description: None,
            required_fields: None,
            auth: None,
        };

        let host = build_host_with_links(
            vec![("order", "orders", None), ("invoice", "invoices", None)],
            vec![link],
        );
        let generator = SchemaGenerator::new(host);
        let mutation_root = generator.generate_mutation_root();

        assert!(
            mutation_root.contains("type Mutation {"),
            "should start with type Mutation"
        );
        assert!(
            mutation_root.contains("createOrder(data: JSON!): Order!"),
            "should have createOrder"
        );
        assert!(
            mutation_root.contains("updateOrder(id: ID!, data: JSON!): Order!"),
            "should have updateOrder"
        );
        assert!(
            mutation_root.contains("deleteOrder(id: ID!): Boolean!"),
            "should have deleteOrder"
        );
        assert!(
            mutation_root.contains(
                "createLink(sourceId: ID!, targetId: ID!, linkType: String!, metadata: JSON): Link!"
            ),
            "should have generic createLink"
        );
        assert!(
            mutation_root.contains("deleteLink(id: ID!): Boolean!"),
            "should have generic deleteLink"
        );
        assert!(
            mutation_root.contains("createInvoiceForOrder"),
            "should have typed link creation"
        );
        assert!(
            mutation_root.contains("linkInvoiceToOrder"),
            "should have typed link mutation"
        );
        assert!(
            mutation_root.contains("unlinkInvoiceFromOrder"),
            "should have typed unlink mutation"
        );
    }

    // -----------------------------------------------------------------------
    // generate_sdl end-to-end test
    // -----------------------------------------------------------------------

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn test_generate_sdl_end_to_end() {
        let order_sample = serde_json::json!({
            "id": "uuid-1",
            "name": "Sample Order",
            "total": 42,
            "active": true,
            "deleted_at": null
        });

        let link = LinkDefinition {
            link_type: "has_invoice".to_string(),
            source_type: "order".to_string(),
            target_type: "invoice".to_string(),
            forward_route_name: "invoices".to_string(),
            reverse_route_name: "parent_order".to_string(),
            description: None,
            required_fields: None,
            auth: None,
        };

        let host = build_host_with_links(
            vec![
                (
                    "order",
                    "orders",
                    Some(Arc::new(MockFetcher::with_sample(order_sample))),
                ),
                (
                    "invoice",
                    "invoices",
                    Some(Arc::new(MockFetcher::with_sample(serde_json::json!({})))),
                ),
            ],
            vec![link],
        );

        let generator = SchemaGenerator::new(host);
        let sdl = generator.generate_sdl().await;

        // Verify schema structure
        assert!(sdl.contains("schema {"), "should have schema definition");
        assert!(
            sdl.contains("query: Query"),
            "schema should reference Query"
        );
        assert!(
            sdl.contains("mutation: Mutation"),
            "schema should reference Mutation"
        );

        // Verify Order type was generated with fields from sample
        assert!(
            sdl.contains("type Order {"),
            "should have Order type: {}",
            sdl
        );
        assert!(
            sdl.contains("id: String!"),
            "Order should have non-nullable id field: {}",
            sdl
        );
        assert!(
            sdl.contains("name: String!"),
            "Order should have name field: {}",
            sdl
        );
        assert!(
            sdl.contains("total: Int!"),
            "Order should have total field: {}",
            sdl
        );
        assert!(
            sdl.contains("active: Boolean!"),
            "Order should have active field: {}",
            sdl
        );
        // deleted_at is null, so it should be nullable (no !)
        assert!(
            sdl.contains("deleted_at: String\n"),
            "Order should have nullable deleted_at: {}",
            sdl
        );

        // Verify relation from link config
        assert!(
            sdl.contains("invoices: [Invoice!]!"),
            "Order should have invoices relation: {}",
            sdl
        );

        // Verify Query and Mutation roots
        assert!(
            sdl.contains("type Query {"),
            "should have Query root: {}",
            sdl
        );
        assert!(
            sdl.contains("type Mutation {"),
            "should have Mutation root: {}",
            sdl
        );
    }

    // -----------------------------------------------------------------------
    // generate_sdl with no fetchers (empty entity types)
    // -----------------------------------------------------------------------

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn test_generate_sdl_empty_host() {
        let host = build_host_with_links(vec![], vec![]);
        let generator = SchemaGenerator::new(host);
        let sdl = generator.generate_sdl().await;

        assert!(sdl.contains("type Query {"), "should have Query root");
        assert!(sdl.contains("type Mutation {"), "should have Mutation root");
        assert!(sdl.contains("schema {"), "should have schema definition");
    }
}
