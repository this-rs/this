//! GraphQL query executor for dynamic schema
//!
//! This module implements a custom GraphQL executor that can execute queries
//! against our dynamically generated schema, using entity fetchers and creators.

#[cfg(feature = "graphql")]
use graphql_parser::query::{parse_query, Document, OperationDefinition, Selection, Field, Value as GqlValue};
#[cfg(feature = "graphql")]
use graphql_parser::schema::{parse_schema, Document as SchemaDocument};
#[cfg(feature = "graphql")]
use serde_json::{json, Value};
#[cfg(feature = "graphql")]
use std::collections::HashMap;
#[cfg(feature = "graphql")]
use std::sync::Arc;
#[cfg(feature = "graphql")]
use uuid::Uuid;
#[cfg(feature = "graphql")]
use anyhow::{Result, bail};
#[cfg(feature = "graphql")]
use futures::future::{BoxFuture, FutureExt};

#[cfg(feature = "graphql")]
use crate::server::host::ServerHost;
#[cfg(feature = "graphql")]
use super::schema_generator::SchemaGenerator;

/// GraphQL executor that executes queries against the dynamic schema
#[cfg(feature = "graphql")]
pub struct GraphQLExecutor {
    host: Arc<ServerHost>,
    schema_sdl: String,
}

#[cfg(feature = "graphql")]
impl GraphQLExecutor {
    /// Create a new executor with the given host
    pub async fn new(host: Arc<ServerHost>) -> Self {
        let generator = SchemaGenerator::new(host.clone());
        let schema_sdl = generator.generate_sdl().await;
        
        Self {
            host,
            schema_sdl,
        }
    }

    /// Execute a GraphQL query and return the result as JSON
    pub async fn execute(&self, query: &str, variables: Option<HashMap<String, Value>>) -> Result<Value> {
        // Parse the query
        let doc = parse_query::<String>(query)
            .map_err(|e| anyhow::anyhow!("Failed to parse query: {:?}", e))?;

        // Execute the query
        let result = self.execute_document(&doc, variables.unwrap_or_default()).await?;

        Ok(json!({
            "data": result
        }))
    }

    /// Execute a parsed GraphQL document
    async fn execute_document(&self, doc: &Document<'_, String>, variables: HashMap<String, Value>) -> Result<Value> {
        // Find the operation to execute (default to first query)
        let operation = doc.definitions.iter()
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
                self.execute_query(&query.selection_set.items, &variables).await
            }
            OperationDefinition::Mutation(mutation) => {
                self.execute_mutation(&mutation.selection_set.items, &variables).await
            }
            OperationDefinition::SelectionSet(selection_set) => {
                self.execute_query(&selection_set.items, &variables).await
            }
            _ => bail!("Subscriptions are not supported"),
        }
    }

    /// Execute a query operation
    async fn execute_query(&self, selections: &[Selection<'_, String>], _variables: &HashMap<String, Value>) -> Result<Value> {
        let mut result = serde_json::Map::new();

        for selection in selections {
            if let Selection::Field(field) = selection {
                let field_name = field.name.as_str();
                let field_value = self.resolve_query_field(field).await?;
                result.insert(field_name.to_string(), field_value);
            }
        }

        Ok(Value::Object(result))
    }

    /// Execute a mutation operation
    async fn execute_mutation(&self, selections: &[Selection<'_, String>], _variables: &HashMap<String, Value>) -> Result<Value> {
        let mut result = serde_json::Map::new();

        for selection in selections {
            if let Selection::Field(field) = selection {
                let field_name = field.name.as_str();
                let field_value = self.resolve_mutation_field(field).await?;
                result.insert(field_name.to_string(), field_value);
            }
        }

        Ok(Value::Object(result))
    }

    /// Resolve a query field (e.g., "orders", "order", "invoice", etc.)
    async fn resolve_query_field(&self, field: &Field<'_, String>) -> Result<Value> {
        let field_name = field.name.as_str();

        // Check if this is a plural query (e.g., "orders", "invoices")
        if let Some(entity_type) = self.get_entity_type_from_plural(field_name) {
            // Get pagination arguments
            let limit = self.get_int_arg(field, "limit");
            let offset = self.get_int_arg(field, "offset");

            // Fetch entities
            if let Some(fetcher) = self.host.entity_fetchers.get(entity_type) {
                let entities = fetcher.list_as_json(limit, offset).await?;
                
                // Resolve sub-fields for each entity
                let resolved_entities = self.resolve_entity_list(entities, &field.selection_set.items, entity_type).await?;
                
                return Ok(Value::Array(resolved_entities));
            } else {
                bail!("Unknown entity type: {}", entity_type);
            }
        }

        // Check if this is a singular query (e.g., "order", "invoice")
        if let Some(entity_type) = self.get_entity_type_from_singular(field_name) {
            // Get the ID argument
            let id = self.get_string_arg(field, "id")
                .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
            let uuid = Uuid::parse_str(&id)?;

            // Fetch the entity
            if let Some(fetcher) = self.host.entity_fetchers.get(entity_type) {
                let entity = fetcher.fetch_as_json(&uuid).await?;
                
                // Resolve sub-fields
                let resolved = self.resolve_entity_fields(entity, &field.selection_set.items, entity_type).await?;
                
                return Ok(resolved);
            } else {
                bail!("Unknown entity type: {}", entity_type);
            }
        }

        bail!("Unknown query field: {}", field_name);
    }

    /// Resolve a mutation field (e.g., "createOrder", "updateInvoice", etc.)
    async fn resolve_mutation_field(&self, field: &Field<'_, String>) -> Result<Value> {
        let field_name = field.name.as_str();

        // Check for createLink mutation (must be before create check)
        if field_name == "createLink" {
            return self.create_link_mutation(field).await;
        }

        // Check for createAndLink mutation (e.g., "createInvoiceForOrder") - must be before create check
        if field_name.starts_with("create") && field_name.contains("For") {
            return self.create_and_link_mutation(field).await;
        }

        // Check for link mutation (e.g., "linkInvoiceToOrder")
        if field_name.starts_with("link") && field_name.contains("To") {
            return self.link_entities_mutation(field).await;
        }

        // Check for unlink mutation (e.g., "unlinkInvoiceFromOrder")
        if field_name.starts_with("unlink") && field_name.contains("From") {
            return self.unlink_entities_mutation(field).await;
        }

        // Check for create mutation (e.g., "createOrder")
        if field_name.starts_with("create") {
            let entity_type = self.mutation_name_to_entity_type(field_name, "create");
            
            // Get data argument
            let data = self.get_json_arg(field, "data")
                .ok_or_else(|| anyhow::anyhow!("Missing required argument 'data'"))?;

            // Create the entity
            if let Some(creator) = self.host.entity_creators.get(&entity_type) {
                let created = creator.create_from_json(data).await?;
                
                // Resolve sub-fields
                let resolved = self.resolve_entity_fields(created, &field.selection_set.items, &entity_type).await?;
                
                return Ok(resolved);
            } else {
                bail!("Unknown entity type: {}", entity_type);
            }
        }

        // Check for update mutation (e.g., "updateOrder")
        if field_name.starts_with("update") {
            let entity_type = self.mutation_name_to_entity_type(field_name, "update");
            
            // Get arguments
            let id = self.get_string_arg(field, "id")
                .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
            let uuid = Uuid::parse_str(&id)?;
            let data = self.get_json_arg(field, "data")
                .ok_or_else(|| anyhow::anyhow!("Missing required argument 'data'"))?;

            // Update the entity
            if let Some(creator) = self.host.entity_creators.get(&entity_type) {
                let updated = creator.update_from_json(&uuid, data).await?;
                
                // Resolve sub-fields
                let resolved = self.resolve_entity_fields(updated, &field.selection_set.items, &entity_type).await?;
                
                return Ok(resolved);
            } else {
                bail!("Unknown entity type: {}", entity_type);
            }
        }

        // Check for delete mutation (e.g., "deleteOrder")
        if field_name.starts_with("delete") {
            let entity_type = self.mutation_name_to_entity_type(field_name, "delete");
            
            // Get ID argument
            let id = self.get_string_arg(field, "id")
                .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
            let uuid = Uuid::parse_str(&id)?;

            // Delete the entity
            if let Some(creator) = self.host.entity_creators.get(&entity_type) {
                creator.delete(&uuid).await?;
                return Ok(Value::Bool(true));
            } else {
                bail!("Unknown entity type: {}", entity_type);
            }
        }

        // Check for deleteLink mutation
        if field_name == "deleteLink" {
            return self.delete_link_mutation(field).await;
        }

        bail!("Unknown mutation field: {}", field_name);
    }

    /// Resolve fields for a list of entities
    async fn resolve_entity_list(
        &self,
        entities: Vec<Value>,
        selections: &[Selection<'_, String>],
        entity_type: &str,
    ) -> Result<Vec<Value>> {
        let mut resolved = Vec::new();
        
        for entity in entities {
            let resolved_entity = self.resolve_entity_fields(entity, selections, entity_type).await?;
            resolved.push(resolved_entity);
        }
        
        Ok(resolved)
    }

    /// Resolve fields for a single entity
    fn resolve_entity_fields<'a>(
        &'a self,
        entity: Value,
        selections: &'a [Selection<'_, String>],
        entity_type: &'a str,
    ) -> BoxFuture<'a, Result<Value>> {
        async move {
            self.resolve_entity_fields_impl(entity, selections, entity_type).await
        }.boxed()
    }

    /// Implementation of resolve_entity_fields
    async fn resolve_entity_fields_impl(
        &self,
        entity: Value,
        selections: &[Selection<'_, String>],
        entity_type: &str,
    ) -> Result<Value> {
        let mut result = serde_json::Map::new();
        
        let entity_obj = entity.as_object()
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
                let snake_case_name = Self::camel_to_snake(field_name);
                if let Some(value) = entity_obj.get(&snake_case_name) {
                    result.insert(field_name.to_string(), value.clone());
                    continue;
                }

                // Check if this is a relation field
                if let Some(relation_value) = self.resolve_relation_field_impl(entity_obj, field, entity_type).await? {
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
        &'a self,
        entity: &'a serde_json::Map<String, Value>,
        field: &'a Field<'_, String>,
        entity_type: &'a str,
    ) -> BoxFuture<'a, Result<Option<Value>>> {
        async move {
            self.resolve_relation_field_inner(entity, field, entity_type).await
        }.boxed()
    }

    /// Inner implementation of resolve_relation_field
    async fn resolve_relation_field_inner(
        &self,
        entity: &serde_json::Map<String, Value>,
        field: &Field<'_, String>,
        entity_type: &str,
    ) -> Result<Option<Value>> {
        let field_name = field.name.as_str();
        let entity_id = entity.get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Entity missing id field"))?;
        let source_uuid = Uuid::parse_str(entity_id)?;

        // Get links configuration for this entity type
        let links_config = &self.host.config;
        
        // Find the link configuration for this relation
        for link_config in &links_config.links {
            if link_config.source_type == entity_type && link_config.forward_route_name == field_name {
                // This is a forward relation (e.g., order -> invoices)
                let links = self.host.link_service
                    .find_by_source(&source_uuid, Some(&link_config.link_type), Some(&link_config.target_type))
                    .await?;

                // Fetch the target entities
                if let Some(fetcher) = self.host.entity_fetchers.get(&link_config.target_type) {
                    let mut targets = Vec::new();
                    
                    for link in links {
                        if let Ok(target_entity) = fetcher.fetch_as_json(&link.target_id).await {
                            let resolved = self.resolve_entity_fields_impl(
                                target_entity,
                                &field.selection_set.items,
                                &link_config.target_type
                            ).await?;
                            targets.push(resolved);
                        }
                    }

                    return Ok(Some(Value::Array(targets)));
                }
            } else if link_config.target_type == entity_type && link_config.reverse_route_name == field_name {
                // This is a reverse relation (e.g., invoice -> order)
                let links = self.host.link_service
                    .find_by_target(&source_uuid, Some(&link_config.link_type), Some(&link_config.source_type))
                    .await?;

                // Fetch the source entity (should be only one for singular relations)
                if let Some(link) = links.first() {
                    if let Some(fetcher) = self.host.entity_fetchers.get(&link_config.source_type) {
                        if let Ok(source_entity) = fetcher.fetch_as_json(&link.source_id).await {
                            let resolved = self.resolve_entity_fields_impl(
                                source_entity,
                                &field.selection_set.items,
                                &link_config.source_type
                            ).await?;
                            return Ok(Some(resolved));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Get entity type from plural field name (e.g., "orders" -> "order")
    fn get_entity_type_from_plural(&self, field_name: &str) -> Option<&str> {
        for entity_type in self.host.entity_types() {
            let plural = self.pluralize(entity_type);
            if plural == field_name {
                return Some(entity_type);
            }
        }
        None
    }

    /// Get entity type from singular field name (e.g., "order" -> "order")
    fn get_entity_type_from_singular(&self, field_name: &str) -> Option<&str> {
        for entity_type in self.host.entity_types() {
            if entity_type == field_name {
                return Some(entity_type);
            }
        }
        None
    }

    /// Convert mutation name to entity type (e.g., "createOrder" -> "order")
    fn mutation_name_to_entity_type(&self, mutation_name: &str, prefix: &str) -> String {
        let name_without_prefix = mutation_name.strip_prefix(prefix).unwrap_or(mutation_name);
        Self::pascal_to_snake(name_without_prefix)
    }

    /// Get string argument from field
    fn get_string_arg(&self, field: &Field<String>, arg_name: &str) -> Option<String> {
        field.arguments.iter()
            .find(|(name, _)| name.as_str() == arg_name)
            .and_then(|(_, value)| {
                if let GqlValue::String(s) = value {
                    Some(s.clone())
                } else {
                    None
                }
            })
    }

    /// Get int argument from field
    fn get_int_arg(&self, field: &Field<String>, arg_name: &str) -> Option<i32> {
        field.arguments.iter()
            .find(|(name, _)| name.as_str() == arg_name)
            .and_then(|(_, value)| {
                if let GqlValue::Int(i) = value {
                    Some(i.as_i64().unwrap_or(0) as i32)
                } else {
                    None
                }
            })
    }

    /// Get JSON argument from field
    fn get_json_arg(&self, field: &Field<String>, arg_name: &str) -> Option<Value> {
        field.arguments.iter()
            .find(|(name, _)| name.as_str() == arg_name)
            .and_then(|(_, value)| {
                Some(self.gql_value_to_json(value))
            })
    }

    /// Convert GraphQL value to JSON
    fn gql_value_to_json(&self, value: &GqlValue<String>) -> Value {
        match value {
            GqlValue::Null => Value::Null,
            GqlValue::Int(i) => json!(i.as_i64().unwrap_or(0)),
            GqlValue::Float(f) => json!(f),
            GqlValue::String(s) => json!(s),
            GqlValue::Boolean(b) => json!(b),
            GqlValue::Enum(e) => json!(e),
            GqlValue::List(list) => {
                Value::Array(list.iter().map(|v| self.gql_value_to_json(v)).collect())
            }
            GqlValue::Object(obj) => {
                let mut map = serde_json::Map::new();
                for (k, v) in obj {
                    map.insert(k.clone(), self.gql_value_to_json(v));
                }
                Value::Object(map)
            }
            GqlValue::Variable(_) => Value::Null, // Variables should be resolved before this
        }
    }

    /// Simple pluralization (can be improved)
    fn pluralize(&self, word: &str) -> String {
        if word.ends_with('y') {
            format!("{}ies", &word[..word.len() - 1])
        } else if word.ends_with('s') || word.ends_with("sh") || word.ends_with("ch") {
            format!("{}es", word)
        } else {
            format!("{}s", word)
        }
    }

    /// Convert PascalCase to snake_case
    fn pascal_to_snake(s: &str) -> String {
        let mut result = String::new();
        for (i, ch) in s.chars().enumerate() {
            if ch.is_uppercase() {
                if i > 0 {
                    result.push('_');
                }
                result.push(ch.to_ascii_lowercase());
            } else {
                result.push(ch);
            }
        }
        result
    }

    /// Convert camelCase to snake_case
    fn camel_to_snake(s: &str) -> String {
        let mut result = String::new();
        for (i, ch) in s.chars().enumerate() {
            if ch.is_uppercase() {
                if i > 0 {
                    result.push('_');
                }
                result.push(ch.to_ascii_lowercase());
            } else {
                result.push(ch);
            }
        }
        result
    }

    /// Create a link between two existing entities
    async fn create_link_mutation(&self, field: &Field<'_, String>) -> Result<Value> {
        use crate::core::link::LinkEntity;

        // Get arguments
        let source_id = self.get_string_arg(field, "sourceId")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'sourceId'"))?;
        let target_id = self.get_string_arg(field, "targetId")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'targetId'"))?;
        let link_type = self.get_string_arg(field, "linkType")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'linkType'"))?;
        
        let source_uuid = Uuid::parse_str(&source_id)?;
        let target_uuid = Uuid::parse_str(&target_id)?;
        
        // Get optional metadata
        let metadata = self.get_json_arg(field, "metadata");

        // Create the link
        let link_entity = LinkEntity::new(link_type, source_uuid, target_uuid, metadata);
        let created_link = self.host.link_service.create(link_entity).await?;

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
    async fn delete_link_mutation(&self, field: &Field<'_, String>) -> Result<Value> {
        let link_id = self.get_string_arg(field, "id")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'id'"))?;
        let uuid = Uuid::parse_str(&link_id)?;

        self.host.link_service.delete(&uuid).await?;
        Ok(Value::Bool(true))
    }

    /// Create an entity and link it to another entity (e.g., createInvoiceForOrder)
    async fn create_and_link_mutation(&self, field: &Field<'_, String>) -> Result<Value> {
        use crate::core::link::LinkEntity;

        let field_name = field.name.as_str();
        
        // Parse field name: createInvoiceForOrder -> (invoice, order)
        let parts: Vec<&str> = field_name.strip_prefix("create").unwrap_or("")
            .split("For")
            .collect();
        
        if parts.len() != 2 {
            bail!("Invalid createAndLink mutation format: {}", field_name);
        }

        let entity_type = Self::pascal_to_snake(parts[0]);
        let parent_type = Self::pascal_to_snake(parts[1]);

        // Get arguments
        let parent_id = self.get_string_arg(field, "parentId")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'parentId'"))?;
        let data = self.get_json_arg(field, "data")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'data'"))?;
        let link_type = self.get_string_arg(field, "linkType");
        
        let parent_uuid = Uuid::parse_str(&parent_id)?;

        // Create the entity
        if let Some(creator) = self.host.entity_creators.get(&entity_type) {
            let created = creator.create_from_json(data).await?;
            
            // Extract the new entity's ID
            let entity_id = created.get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Created entity missing id field"))?;
            let entity_uuid = Uuid::parse_str(entity_id)?;

            // Find the appropriate link type from config
            let actual_link_type = if let Some(lt) = link_type {
                lt
            } else {
                // Try to find link type from config
                self.find_link_type(&parent_type, &entity_type)?
            };

            // Create the link
            let link_entity = LinkEntity::new(actual_link_type, parent_uuid, entity_uuid, None);
            self.host.link_service.create(link_entity).await?;

            // Resolve sub-fields for the created entity
            let resolved = self.resolve_entity_fields(created, &field.selection_set.items, &entity_type).await?;
            
            return Ok(resolved);
        } else {
            bail!("Unknown entity type: {}", entity_type);
        }
    }

    /// Link two existing entities (e.g., linkInvoiceToOrder)
    async fn link_entities_mutation(&self, field: &Field<'_, String>) -> Result<Value> {
        use crate::core::link::LinkEntity;

        let field_name = field.name.as_str();
        
        // Parse field name: linkInvoiceToOrder -> (invoice, order)
        let parts: Vec<&str> = field_name.strip_prefix("link").unwrap_or("")
            .split("To")
            .collect();
        
        if parts.len() != 2 {
            bail!("Invalid link mutation format: {}", field_name);
        }

        let source_type = Self::pascal_to_snake(parts[0]);
        let target_type = Self::pascal_to_snake(parts[1]);

        // Get arguments
        let source_id = self.get_string_arg(field, "sourceId")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'sourceId'"))?;
        let target_id = self.get_string_arg(field, "targetId")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'targetId'"))?;
        let link_type = self.get_string_arg(field, "linkType");
        
        let source_uuid = Uuid::parse_str(&source_id)?;
        let target_uuid = Uuid::parse_str(&target_id)?;

        // Find the appropriate link type from config
        let actual_link_type = if let Some(lt) = link_type {
            lt
        } else {
            self.find_link_type(&source_type, &target_type)?
        };

        // Get optional metadata
        let metadata = self.get_json_arg(field, "metadata");

        // Create the link
        let link_entity = LinkEntity::new(actual_link_type, source_uuid, target_uuid, metadata);
        let created_link = self.host.link_service.create(link_entity).await?;

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
    async fn unlink_entities_mutation(&self, field: &Field<'_, String>) -> Result<Value> {
        let field_name = field.name.as_str();
        
        // Parse field name: unlinkInvoiceFromOrder -> (invoice, order)
        let parts: Vec<&str> = field_name.strip_prefix("unlink").unwrap_or("")
            .split("From")
            .collect();
        
        if parts.len() != 2 {
            bail!("Invalid unlink mutation format: {}", field_name);
        }

        let source_type = Self::pascal_to_snake(parts[0]);
        let target_type = Self::pascal_to_snake(parts[1]);

        // Get arguments
        let source_id = self.get_string_arg(field, "sourceId")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'sourceId'"))?;
        let target_id = self.get_string_arg(field, "targetId")
            .ok_or_else(|| anyhow::anyhow!("Missing required argument 'targetId'"))?;
        let link_type = self.get_string_arg(field, "linkType");
        
        let source_uuid = Uuid::parse_str(&source_id)?;
        let target_uuid = Uuid::parse_str(&target_id)?;

        // Find the appropriate link type from config
        let actual_link_type = if let Some(lt) = link_type {
            Some(lt)
        } else {
            self.find_link_type(&source_type, &target_type).ok()
        };

        // Find and delete the link
        let links = self.host.link_service
            .find_by_source(&source_uuid, actual_link_type.as_deref(), Some(&target_type))
            .await?;

        for link in links {
            if link.target_id == target_uuid {
                self.host.link_service.delete(&link.id).await?;
                return Ok(Value::Bool(true));
            }
        }

        Ok(Value::Bool(false))
    }

    /// Find link type from configuration
    fn find_link_type(&self, source_type: &str, target_type: &str) -> Result<String> {
        for link_config in &self.host.config.links {
            if link_config.source_type == source_type && link_config.target_type == target_type {
                return Ok(link_config.link_type.clone());
            }
        }
        bail!("No link configuration found for {} -> {}", source_type, target_type);
    }
}

