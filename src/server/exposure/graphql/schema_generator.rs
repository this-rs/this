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
    pub description: Option<String>,
}

#[cfg(feature = "graphql")]
/// Information about a relation between entities
#[derive(Debug, Clone)]
pub struct RelationInfo {
    pub name: String,
    pub target_type: String,
    pub is_list: bool,
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
            if sample.as_object().map_or(true, |obj| obj.is_empty()) {
                if let Ok(entities) = fetcher.list_as_json(Some(1), None).await {
                    if let Some(first_entity) = entities.first() {
                        sample = first_entity.clone();
                    }
                }
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

        type_def.push_str("}");
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

        query.push_str("}");
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

        mutation.push_str("}");
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
