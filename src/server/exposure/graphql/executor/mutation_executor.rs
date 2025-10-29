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
