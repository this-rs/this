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
