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
