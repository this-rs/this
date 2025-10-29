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

        return Ok(resolved);
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
