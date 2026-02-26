//! Reusable field filters
//!
//! These filters transform entity field values before validation

use anyhow::Result;
use serde_json::{Value, json};

/// Filter: trim whitespace from string
pub fn trim() -> impl Fn(&str, Value) -> Result<Value> + Send + Sync + Clone {
    |_: &str, value: Value| {
        if let Some(s) = value.as_str() {
            Ok(Value::String(s.trim().to_string()))
        } else {
            Ok(value)
        }
    }
}

/// Filter: convert string to uppercase
pub fn uppercase() -> impl Fn(&str, Value) -> Result<Value> + Send + Sync + Clone {
    |_: &str, value: Value| {
        if let Some(s) = value.as_str() {
            Ok(Value::String(s.to_uppercase()))
        } else {
            Ok(value)
        }
    }
}

/// Filter: convert string to lowercase
pub fn lowercase() -> impl Fn(&str, Value) -> Result<Value> + Send + Sync + Clone {
    |_: &str, value: Value| {
        if let Some(s) = value.as_str() {
            Ok(Value::String(s.to_lowercase()))
        } else {
            Ok(value)
        }
    }
}

/// Filter: round number to specified decimal places
pub fn round_decimals(
    decimals: u32,
) -> impl Fn(&str, Value) -> Result<Value> + Send + Sync + Clone {
    move |_: &str, value: Value| {
        if let Some(num) = value.as_f64() {
            let factor = 10_f64.powi(decimals as i32);
            let rounded = (num * factor).round() / factor;
            Ok(json!(rounded))
        } else {
            Ok(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // === trim() ===

    #[test]
    fn test_trim_removes_whitespace() {
        let f = trim();
        let result = f("name", json!("  hello  ")).expect("should not fail");
        assert_eq!(result, json!("hello"));
    }

    #[test]
    fn test_trim_no_whitespace_unchanged() {
        let f = trim();
        let result = f("name", json!("hello")).expect("should not fail");
        assert_eq!(result, json!("hello"));
    }

    #[test]
    fn test_trim_non_string_passthrough() {
        let f = trim();
        let result = f("age", json!(42)).expect("should not fail");
        assert_eq!(result, json!(42));
    }

    #[test]
    fn test_trim_empty_string() {
        let f = trim();
        let result = f("name", json!("   ")).expect("should not fail");
        assert_eq!(result, json!(""));
    }

    #[test]
    fn test_trim_null_passthrough() {
        let f = trim();
        let result = f("name", json!(null)).expect("should not fail");
        assert_eq!(result, json!(null));
    }

    // === uppercase() ===

    #[test]
    fn test_uppercase_converts_string() {
        let f = uppercase();
        let result = f("code", json!("hello")).expect("should not fail");
        assert_eq!(result, json!("HELLO"));
    }

    #[test]
    fn test_uppercase_already_uppercase() {
        let f = uppercase();
        let result = f("code", json!("HELLO")).expect("should not fail");
        assert_eq!(result, json!("HELLO"));
    }

    #[test]
    fn test_uppercase_non_string_passthrough() {
        let f = uppercase();
        let result = f("count", json!(42)).expect("should not fail");
        assert_eq!(result, json!(42));
    }

    #[test]
    fn test_uppercase_mixed_case() {
        let f = uppercase();
        let result = f("code", json!("Hello World")).expect("should not fail");
        assert_eq!(result, json!("HELLO WORLD"));
    }

    // === lowercase() ===

    #[test]
    fn test_lowercase_converts_string() {
        let f = lowercase();
        let result = f("email", json!("Hello@WORLD.com")).expect("should not fail");
        assert_eq!(result, json!("hello@world.com"));
    }

    #[test]
    fn test_lowercase_already_lowercase() {
        let f = lowercase();
        let result = f("email", json!("hello")).expect("should not fail");
        assert_eq!(result, json!("hello"));
    }

    #[test]
    fn test_lowercase_non_string_passthrough() {
        let f = lowercase();
        let result = f("count", json!(true)).expect("should not fail");
        assert_eq!(result, json!(true));
    }

    // === round_decimals() ===

    #[test]
    fn test_round_decimals_two_places() {
        let f = round_decimals(2);
        let result = f("price", json!(3.14159)).expect("should not fail");
        assert_eq!(result, json!(3.14));
    }

    #[test]
    fn test_round_decimals_zero_places() {
        let f = round_decimals(0);
        let result = f("count", json!(3.7)).expect("should not fail");
        assert_eq!(result, json!(4.0));
    }

    #[test]
    fn test_round_decimals_rounds_up() {
        let f = round_decimals(1);
        let result = f("price", json!(2.55)).expect("should not fail");
        assert_eq!(result, json!(2.6));
    }

    #[test]
    fn test_round_decimals_non_number_passthrough() {
        let f = round_decimals(2);
        let result = f("name", json!("hello")).expect("should not fail");
        assert_eq!(result, json!("hello"));
    }

    #[test]
    fn test_round_decimals_negative_number() {
        let f = round_decimals(1);
        let result = f("temp", json!(-3.456)).expect("should not fail");
        assert_eq!(result, json!(-3.5));
    }

    #[test]
    fn test_round_decimals_integer_unchanged() {
        let f = round_decimals(2);
        let result = f("count", json!(42.0)).expect("should not fail");
        assert_eq!(result, json!(42.0));
    }
}
