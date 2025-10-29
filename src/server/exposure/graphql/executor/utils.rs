//! Utility functions for GraphQL execution

use anyhow::Result;
use graphql_parser::query::{Field, Value as GqlValue};
use serde_json::{Value, json};

/// Get string argument from field
pub fn get_string_arg(field: &Field<String>, arg_name: &str) -> Option<String> {
    field
        .arguments
        .iter()
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
pub fn get_int_arg(field: &Field<String>, arg_name: &str) -> Option<i32> {
    field
        .arguments
        .iter()
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
pub fn get_json_arg(field: &Field<String>, arg_name: &str) -> Option<Value> {
    field
        .arguments
        .iter()
        .find(|(name, _)| name.as_str() == arg_name)
        .map(|(_, value)| gql_value_to_json(value))
}

/// Convert GraphQL value to JSON
pub fn gql_value_to_json(value: &GqlValue<String>) -> Value {
    match value {
        GqlValue::Null => Value::Null,
        GqlValue::Int(i) => json!(i.as_i64().unwrap_or(0)),
        GqlValue::Float(f) => json!(f),
        GqlValue::String(s) => json!(s),
        GqlValue::Boolean(b) => json!(b),
        GqlValue::Enum(e) => json!(e),
        GqlValue::List(list) => Value::Array(list.iter().map(gql_value_to_json).collect()),
        GqlValue::Object(obj) => {
            let mut map = serde_json::Map::new();
            for (k, v) in obj {
                map.insert(k.clone(), gql_value_to_json(v));
            }
            Value::Object(map)
        }
        GqlValue::Variable(_) => Value::Null, // Variables should be resolved before this
    }
}

/// Simple pluralization (can be improved)
pub fn pluralize(word: &str) -> String {
    if word.ends_with('y') {
        format!("{}ies", &word[..word.len() - 1])
    } else if word.ends_with('s') || word.ends_with("sh") || word.ends_with("ch") {
        format!("{}es", word)
    } else {
        format!("{}s", word)
    }
}

/// Convert PascalCase to snake_case
pub fn pascal_to_snake(s: &str) -> String {
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
pub fn camel_to_snake(s: &str) -> String {
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

/// Convert mutation name to entity type (e.g., "createOrder" -> "order")
pub fn mutation_name_to_entity_type(mutation_name: &str, prefix: &str) -> String {
    let name_without_prefix = mutation_name.strip_prefix(prefix).unwrap_or(mutation_name);
    pascal_to_snake(name_without_prefix)
}

/// Find link type from configuration
pub fn find_link_type(
    links: &[crate::core::link::LinkDefinition],
    source_type: &str,
    target_type: &str,
) -> Result<String> {
    for link_config in links {
        if link_config.source_type == source_type && link_config.target_type == target_type {
            return Ok(link_config.link_type.clone());
        }
    }
    anyhow::bail!(
        "No link configuration found for {} -> {}",
        source_type,
        target_type
    )
}
