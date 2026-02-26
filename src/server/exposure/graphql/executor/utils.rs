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
    if let Some(stripped) = word.strip_suffix('y') {
        format!("{}ies", stripped)
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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- gql_value_to_json tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_null() {
        let result = gql_value_to_json(&GqlValue::Null);
        assert_eq!(result, Value::Null);
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_int() {
        let num = graphql_parser::query::Number::from(42i32);
        let result = gql_value_to_json(&GqlValue::Int(num));
        assert_eq!(result, serde_json::json!(42));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_float() {
        let result = gql_value_to_json(&GqlValue::Float(3.15));
        assert_eq!(result, serde_json::json!(3.15));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_string() {
        let result = gql_value_to_json(&GqlValue::String("hello".to_string()));
        assert_eq!(result, serde_json::json!("hello"));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_boolean() {
        let result_true = gql_value_to_json(&GqlValue::Boolean(true));
        let result_false = gql_value_to_json(&GqlValue::Boolean(false));
        assert_eq!(result_true, serde_json::json!(true));
        assert_eq!(result_false, serde_json::json!(false));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_enum() {
        let result = gql_value_to_json(&GqlValue::Enum("ACTIVE".to_string()));
        assert_eq!(result, serde_json::json!("ACTIVE"));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_list() {
        let list = GqlValue::List(vec![
            GqlValue::Int(graphql_parser::query::Number::from(1i32)),
            GqlValue::Int(graphql_parser::query::Number::from(2i32)),
        ]);
        let result = gql_value_to_json(&list);
        assert_eq!(result, serde_json::json!([1, 2]));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_object() {
        let mut obj = std::collections::BTreeMap::new();
        obj.insert("name".to_string(), GqlValue::String("Alice".to_string()));
        obj.insert(
            "age".to_string(),
            GqlValue::Int(graphql_parser::query::Number::from(30i32)),
        );
        let result = gql_value_to_json(&GqlValue::Object(obj));
        assert_eq!(result, serde_json::json!({"name": "Alice", "age": 30}));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_gql_value_to_json_variable() {
        let result = gql_value_to_json(&GqlValue::Variable("myVar".to_string()));
        assert_eq!(result, Value::Null);
    }

    // ---- pluralize tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pluralize_regular() {
        assert_eq!(pluralize("order"), "orders");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pluralize_ending_in_y() {
        assert_eq!(pluralize("baby"), "babies");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pluralize_ending_in_s() {
        assert_eq!(pluralize("bus"), "buses");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pluralize_ending_in_ch() {
        assert_eq!(pluralize("church"), "churches");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pluralize_ending_in_sh() {
        assert_eq!(pluralize("dish"), "dishes");
    }

    // ---- pascal_to_snake tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pascal_to_snake_multi_word() {
        assert_eq!(pascal_to_snake("OrderItem"), "order_item");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pascal_to_snake_single_char() {
        assert_eq!(pascal_to_snake("A"), "a");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pascal_to_snake_empty() {
        assert_eq!(pascal_to_snake(""), "");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_pascal_to_snake_already_lower() {
        assert_eq!(pascal_to_snake("order"), "order");
    }

    // ---- camel_to_snake tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_camel_to_snake_multi_word() {
        assert_eq!(camel_to_snake("createdAt"), "created_at");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_camel_to_snake_single_word() {
        assert_eq!(camel_to_snake("id"), "id");
    }

    // ---- mutation_name_to_entity_type tests ----

    #[cfg(feature = "graphql")]
    #[test]
    fn test_mutation_name_to_entity_type_create() {
        assert_eq!(
            mutation_name_to_entity_type("createOrder", "create"),
            "order"
        );
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_mutation_name_to_entity_type_delete_pascal() {
        assert_eq!(
            mutation_name_to_entity_type("deleteUserProfile", "delete"),
            "user_profile"
        );
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_mutation_name_to_entity_type_no_prefix_match() {
        // When prefix doesn't match, the whole name is used
        assert_eq!(
            mutation_name_to_entity_type("createOrder", "delete"),
            "create_order"
        );
    }

    // ---- find_link_type tests ----

    #[cfg(feature = "graphql")]
    fn make_link_def(
        source: &str,
        target: &str,
        link_type: &str,
    ) -> crate::core::link::LinkDefinition {
        crate::core::link::LinkDefinition {
            link_type: link_type.to_string(),
            source_type: source.to_string(),
            target_type: target.to_string(),
            forward_route_name: format!("{}s", target),
            reverse_route_name: source.to_string(),
            description: None,
            required_fields: None,
            auth: None,
        }
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_find_link_type_found() {
        let links = vec![make_link_def("order", "invoice", "has_invoice")];
        let result = find_link_type(&links, "order", "invoice").expect("should find link type");
        assert_eq!(result, "has_invoice");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_find_link_type_not_found() {
        let links = vec![make_link_def("order", "invoice", "has_invoice")];
        let result = find_link_type(&links, "user", "car");
        assert!(result.is_err());
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_find_link_type_multiple_configs() {
        let links = vec![
            make_link_def("order", "invoice", "has_invoice"),
            make_link_def("user", "car", "owner"),
        ];
        let result = find_link_type(&links, "user", "car")
            .expect("should find link type among multiple configs");
        assert_eq!(result, "owner");
    }

    // ---- get_string_arg / get_int_arg / get_json_arg tests ----

    #[cfg(feature = "graphql")]
    fn make_field(arguments: Vec<(String, GqlValue<String>)>) -> Field<String> {
        use graphql_parser::Pos;
        use graphql_parser::query::SelectionSet;
        Field {
            position: Pos { line: 1, column: 1 },
            alias: None,
            name: "test_field".to_string(),
            arguments,
            directives: vec![],
            selection_set: SelectionSet {
                span: (Pos { line: 1, column: 1 }, Pos { line: 1, column: 1 }),
                items: vec![],
            },
        }
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_string_arg_present() {
        let field = make_field(vec![(
            "name".to_string(),
            GqlValue::String("Alice".to_string()),
        )]);
        let result = get_string_arg(&field, "name");
        assert_eq!(result, Some("Alice".to_string()));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_string_arg_missing() {
        let field = make_field(vec![]);
        let result = get_string_arg(&field, "name");
        assert_eq!(result, None);
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_string_arg_wrong_type() {
        let field = make_field(vec![(
            "name".to_string(),
            GqlValue::Int(graphql_parser::query::Number::from(42i32)),
        )]);
        let result = get_string_arg(&field, "name");
        assert_eq!(result, None);
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_int_arg_present() {
        let field = make_field(vec![(
            "limit".to_string(),
            GqlValue::Int(graphql_parser::query::Number::from(10i32)),
        )]);
        let result = get_int_arg(&field, "limit");
        assert_eq!(result, Some(10));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_int_arg_missing() {
        let field = make_field(vec![]);
        let result = get_int_arg(&field, "limit");
        assert_eq!(result, None);
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_int_arg_wrong_type() {
        let field = make_field(vec![(
            "limit".to_string(),
            GqlValue::String("not_a_number".to_string()),
        )]);
        let result = get_int_arg(&field, "limit");
        assert_eq!(result, None);
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_json_arg_present() {
        let field = make_field(vec![(
            "data".to_string(),
            GqlValue::String("hello".to_string()),
        )]);
        let result = get_json_arg(&field, "data");
        assert_eq!(result, Some(serde_json::json!("hello")));
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn test_get_json_arg_missing() {
        let field = make_field(vec![]);
        let result = get_json_arg(&field, "data");
        assert_eq!(result, None);
    }
}
