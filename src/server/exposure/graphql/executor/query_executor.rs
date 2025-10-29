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
    for entity_type in host.entity_types() {
        if entity_type == field_name {
            return Some(entity_type);
        }
    }
    None
}
