//! Reusable field validators
//!
//! These validators are used by the macro system to validate entity fields

use serde_json::Value;

/// Validator: field is required (not null)
pub fn required() -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    |field: &str, value: &Value| {
        if value.is_null() {
            Err(format!("Le champ '{}' est requis", field))
        } else {
            Ok(())
        }
    }
}

/// Validator: field is optional (always valid)
pub fn optional() -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    |_: &str, _: &Value| Ok(())
}

/// Validator: number must be positive
pub fn positive() -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    |field: &str, value: &Value| {
        if let Some(num) = value.as_f64() {
            if num <= 0.0 {
                Err(format!(
                    "Le champ '{}' doit être positif (valeur: {})",
                    field, num
                ))
            } else {
                Ok(())
            }
        } else {
            Ok(()) // Si ce n'est pas un nombre, on laisse passer (autre validateur gérera)
        }
    }
}

/// Validator: string length must be within range
pub fn string_length(
    min: usize,
    max: usize,
) -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    move |field: &str, value: &Value| {
        if let Some(s) = value.as_str() {
            let len = s.len();
            if len < min {
                Err(format!(
                    "'{}' doit avoir au moins {} caractères (actuellement: {})",
                    field, min, len
                ))
            } else if len > max {
                Err(format!(
                    "'{}' ne doit pas dépasser {} caractères (actuellement: {})",
                    field, max, len
                ))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}

/// Validator: number must not exceed maximum
pub fn max_value(max: f64) -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    move |field: &str, value: &Value| {
        if let Some(num) = value.as_f64() {
            if num > max {
                Err(format!(
                    "'{}' ne doit pas dépasser {} (valeur: {})",
                    field, max, num
                ))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}

/// Validator: value must be in allowed list
pub fn in_list(
    allowed: Vec<String>,
) -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    move |field: &str, value: &Value| {
        if let Some(s) = value.as_str() {
            if !allowed.contains(&s.to_string()) {
                Err(format!(
                    "'{}' doit être l'une des valeurs: {:?} (valeur actuelle: {})",
                    field, allowed, s
                ))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}

/// Validator: date must match format
pub fn date_format(
    format: &'static str,
) -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    move |field: &str, value: &Value| {
        if let Some(s) = value.as_str() {
            match chrono::NaiveDate::parse_from_str(s, format) {
                Ok(_) => Ok(()),
                Err(_) => Err(format!(
                    "'{}' doit être au format {} (valeur actuelle: {})",
                    field, format, s
                )),
            }
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // === required() ===

    #[test]
    fn test_required_null_value_returns_error() {
        let v = required();
        let result = v("name", &json!(null));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requis"));
    }

    #[test]
    fn test_required_string_value_returns_ok() {
        let v = required();
        assert!(v("name", &json!("hello")).is_ok());
    }

    #[test]
    fn test_required_number_value_returns_ok() {
        let v = required();
        assert!(v("age", &json!(42)).is_ok());
    }

    #[test]
    fn test_required_bool_value_returns_ok() {
        let v = required();
        assert!(v("active", &json!(true)).is_ok());
    }

    #[test]
    fn test_required_object_value_returns_ok() {
        let v = required();
        assert!(v("data", &json!({"key": "val"})).is_ok());
    }

    #[test]
    fn test_required_empty_string_returns_ok() {
        let v = required();
        assert!(v("name", &json!("")).is_ok());
    }

    #[test]
    fn test_required_array_returns_ok() {
        let v = required();
        assert!(v("tags", &json!([1, 2, 3])).is_ok());
    }

    // === optional() ===

    #[test]
    fn test_optional_always_ok_for_null() {
        let v = optional();
        assert!(v("field", &json!(null)).is_ok());
    }

    #[test]
    fn test_optional_always_ok_for_string() {
        let v = optional();
        assert!(v("field", &json!("value")).is_ok());
    }

    // === positive() ===

    #[test]
    fn test_positive_negative_number_returns_error() {
        let v = positive();
        let result = v("price", &json!(-5.0));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("positif"));
    }

    #[test]
    fn test_positive_zero_returns_error() {
        let v = positive();
        assert!(v("price", &json!(0.0)).is_err());
    }

    #[test]
    fn test_positive_positive_number_returns_ok() {
        let v = positive();
        assert!(v("price", &json!(42.5)).is_ok());
    }

    #[test]
    fn test_positive_non_number_passthrough() {
        let v = positive();
        assert!(v("name", &json!("hello")).is_ok());
    }

    #[test]
    fn test_positive_integer_positive() {
        let v = positive();
        assert!(v("count", &json!(1)).is_ok());
    }

    #[test]
    fn test_positive_integer_negative() {
        let v = positive();
        assert!(v("count", &json!(-1)).is_err());
    }

    // === string_length() ===

    #[test]
    fn test_string_length_too_short_returns_error() {
        let v = string_length(3, 50);
        let result = v("name", &json!("ab"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("au moins 3"));
    }

    #[test]
    fn test_string_length_too_long_returns_error() {
        let v = string_length(1, 5);
        let result = v("name", &json!("abcdef"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("dépasser 5"));
    }

    #[test]
    fn test_string_length_exact_min_returns_ok() {
        let v = string_length(3, 10);
        assert!(v("name", &json!("abc")).is_ok());
    }

    #[test]
    fn test_string_length_exact_max_returns_ok() {
        let v = string_length(1, 5);
        assert!(v("name", &json!("abcde")).is_ok());
    }

    #[test]
    fn test_string_length_within_range_returns_ok() {
        let v = string_length(2, 10);
        assert!(v("name", &json!("hello")).is_ok());
    }

    #[test]
    fn test_string_length_non_string_passthrough() {
        let v = string_length(5, 10);
        assert!(v("age", &json!(42)).is_ok());
    }

    // === max_value() ===

    #[test]
    fn test_max_value_over_returns_error() {
        let v = max_value(100.0);
        let result = v("score", &json!(101.0));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("dépasser 100"));
    }

    #[test]
    fn test_max_value_equal_returns_ok() {
        let v = max_value(100.0);
        assert!(v("score", &json!(100.0)).is_ok());
    }

    #[test]
    fn test_max_value_under_returns_ok() {
        let v = max_value(100.0);
        assert!(v("score", &json!(50.0)).is_ok());
    }

    #[test]
    fn test_max_value_non_number_passthrough() {
        let v = max_value(100.0);
        assert!(v("name", &json!("hello")).is_ok());
    }

    #[test]
    fn test_max_value_negative() {
        let v = max_value(0.0);
        assert!(v("temp", &json!(-10.0)).is_ok());
    }

    // === in_list() ===

    #[test]
    fn test_in_list_value_in_list_returns_ok() {
        let v = in_list(vec!["active".into(), "inactive".into(), "pending".into()]);
        assert!(v("status", &json!("active")).is_ok());
    }

    #[test]
    fn test_in_list_value_not_in_list_returns_error() {
        let v = in_list(vec!["active".into(), "inactive".into()]);
        let result = v("status", &json!("deleted"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("valeurs"));
    }

    #[test]
    fn test_in_list_non_string_passthrough() {
        let v = in_list(vec!["yes".into(), "no".into()]);
        assert!(v("flag", &json!(42)).is_ok());
    }

    #[test]
    fn test_in_list_empty_list_always_error_for_strings() {
        let v = in_list(vec![]);
        assert!(v("status", &json!("anything")).is_err());
    }

    // === date_format() ===

    #[test]
    fn test_date_format_valid_date_returns_ok() {
        let v = date_format("%Y-%m-%d");
        assert!(v("birthday", &json!("2024-01-15")).is_ok());
    }

    #[test]
    fn test_date_format_invalid_date_returns_error() {
        let v = date_format("%Y-%m-%d");
        let result = v("birthday", &json!("not-a-date"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("format"));
    }

    #[test]
    fn test_date_format_non_string_passthrough() {
        let v = date_format("%Y-%m-%d");
        assert!(v("birthday", &json!(12345)).is_ok());
    }

    #[test]
    fn test_date_format_wrong_format_returns_error() {
        let v = date_format("%d/%m/%Y");
        assert!(v("date", &json!("2024-01-15")).is_err());
    }

    #[test]
    fn test_date_format_correct_custom_format() {
        let v = date_format("%d/%m/%Y");
        assert!(v("date", &json!("15/01/2024")).is_ok());
    }
}
