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
