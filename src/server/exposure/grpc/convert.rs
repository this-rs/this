//! Conversion utilities between serde_json::Value and prost_types::Struct
//!
//! gRPC uses `google.protobuf.Struct` for dynamic data, while this-rs
//! framework works with `serde_json::Value`. This module bridges both worlds.

use prost_types::value::Kind;
use prost_types::{ListValue, Struct, Value};

/// Convert a `serde_json::Value` to a `prost_types::Struct`
///
/// Only JSON objects can be converted to Struct. Other types will
/// return an empty Struct.
pub fn json_to_struct(json: &serde_json::Value) -> Struct {
    match json {
        serde_json::Value::Object(map) => {
            let fields = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect();
            Struct { fields }
        }
        _ => Struct::default(),
    }
}

/// Convert a `serde_json::Value` to a `prost_types::Value`
pub fn json_to_value(json: &serde_json::Value) -> Value {
    let kind = match json {
        serde_json::Value::Null => Some(Kind::NullValue(0)),
        serde_json::Value::Bool(b) => Some(Kind::BoolValue(*b)),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                Some(Kind::NumberValue(f))
            } else {
                Some(Kind::StringValue(n.to_string()))
            }
        }
        serde_json::Value::String(s) => Some(Kind::StringValue(s.clone())),
        serde_json::Value::Array(arr) => {
            let values = arr.iter().map(json_to_value).collect();
            Some(Kind::ListValue(ListValue { values }))
        }
        serde_json::Value::Object(map) => {
            let fields = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect();
            Some(Kind::StructValue(Struct { fields }))
        }
    };
    Value { kind }
}

/// Convert a `prost_types::Struct` to a `serde_json::Value`
pub fn struct_to_json(s: &Struct) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = s
        .fields
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json(v)))
        .collect();
    serde_json::Value::Object(map)
}

/// Convert a `prost_types::Value` to a `serde_json::Value`
pub fn value_to_json(v: &Value) -> serde_json::Value {
    match &v.kind {
        Some(Kind::NullValue(_)) => serde_json::Value::Null,
        Some(Kind::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Kind::NumberValue(n)) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Some(Kind::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Kind::ListValue(list)) => {
            let values: Vec<serde_json::Value> = list.values.iter().map(value_to_json).collect();
            serde_json::Value::Array(values)
        }
        Some(Kind::StructValue(s)) => struct_to_json(s),
        None => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_to_struct_simple() {
        let json = json!({"name": "Alice", "age": 30, "active": true});
        let s = json_to_struct(&json);

        assert_eq!(s.fields.len(), 3);
        assert!(matches!(
            s.fields.get("name").unwrap().kind,
            Some(Kind::StringValue(_))
        ));
        assert!(matches!(
            s.fields.get("age").unwrap().kind,
            Some(Kind::NumberValue(_))
        ));
        assert!(matches!(
            s.fields.get("active").unwrap().kind,
            Some(Kind::BoolValue(true))
        ));
    }

    #[test]
    fn test_json_to_struct_nested() {
        let json = json!({"user": {"name": "Alice"}, "tags": ["admin", "user"]});
        let s = json_to_struct(&json);

        assert!(matches!(
            s.fields.get("user").unwrap().kind,
            Some(Kind::StructValue(_))
        ));
        assert!(matches!(
            s.fields.get("tags").unwrap().kind,
            Some(Kind::ListValue(_))
        ));
    }

    #[test]
    fn test_json_to_struct_null_fields() {
        let json = json!({"name": null});
        let s = json_to_struct(&json);

        assert!(matches!(
            s.fields.get("name").unwrap().kind,
            Some(Kind::NullValue(_))
        ));
    }

    #[test]
    fn test_struct_to_json_roundtrip() {
        let original = json!({
            "name": "Alice",
            "age": 30.0,
            "active": true,
            "address": null,
            "tags": ["admin", "user"],
            "profile": {"bio": "Hello"}
        });

        let s = json_to_struct(&original);
        let result = struct_to_json(&s);

        assert_eq!(original, result);
    }

    #[test]
    fn test_empty_struct() {
        let json = json!({});
        let s = json_to_struct(&json);
        assert!(s.fields.is_empty());

        let back = struct_to_json(&s);
        assert_eq!(back, json!({}));
    }

    #[test]
    fn test_non_object_to_struct() {
        // Non-object JSON should produce empty struct
        let json = json!("not an object");
        let s = json_to_struct(&json);
        assert!(s.fields.is_empty());
    }
}
