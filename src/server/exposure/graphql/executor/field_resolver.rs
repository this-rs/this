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
