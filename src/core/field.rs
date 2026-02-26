//! Field value types and validation

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use uuid::Uuid;

/// A polymorphic field value that can hold different types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum FieldValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Uuid(Uuid),
    DateTime(DateTime<Utc>),
    Null,
}

impl FieldValue {
    /// Get the value as a string if possible
    pub fn as_string(&self) -> Option<&str> {
        match self {
            FieldValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get the value as an integer if possible
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            FieldValue::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Get the value as a UUID if possible
    pub fn as_uuid(&self) -> Option<Uuid> {
        match self {
            FieldValue::Uuid(u) => Some(*u),
            _ => None,
        }
    }

    /// Check if the value is null
    pub fn is_null(&self) -> bool {
        matches!(self, FieldValue::Null)
    }
}

/// Field format validators for automatic validation
#[derive(Debug, Clone)]
pub enum FieldFormat {
    Email,
    Uuid,
    Url,
    Phone,
    Custom(Regex),
}

impl FieldFormat {
    /// Validate a field value against this format
    pub fn validate(&self, value: &FieldValue) -> bool {
        let string_value = match value.as_string() {
            Some(s) => s,
            None => return false,
        };

        match self {
            FieldFormat::Email => Self::is_valid_email(string_value),
            FieldFormat::Uuid => Uuid::parse_str(string_value).is_ok(),
            FieldFormat::Url => Self::is_valid_url(string_value),
            FieldFormat::Phone => Self::is_valid_phone(string_value),
            FieldFormat::Custom(regex) => regex.is_match(string_value),
        }
    }

    fn is_valid_email(email: &str) -> bool {
        static EMAIL_REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = EMAIL_REGEX.get_or_init(|| {
            Regex::new(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$").unwrap()
        });
        regex.is_match(email)
    }

    fn is_valid_url(url: &str) -> bool {
        static URL_REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = URL_REGEX.get_or_init(|| Regex::new(r"^https?://[^\s/$.?#].[^\s]*$").unwrap());
        regex.is_match(url)
    }

    fn is_valid_phone(phone: &str) -> bool {
        static PHONE_REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = PHONE_REGEX.get_or_init(|| {
            // At least 8 digits, max 15 (E.164 standard)
            Regex::new(r"^\+?[1-9]\d{7,14}$").unwrap()
        });
        regex.is_match(phone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_value_string() {
        let value = FieldValue::String("test".to_string());
        assert_eq!(value.as_string(), Some("test"));
        assert_eq!(value.as_integer(), None);
        assert!(!value.is_null());
    }

    #[test]
    fn test_field_value_integer() {
        let value = FieldValue::Integer(42);
        assert_eq!(value.as_integer(), Some(42));
        assert_eq!(value.as_string(), None);
    }

    #[test]
    fn test_field_value_null() {
        let value = FieldValue::Null;
        assert!(value.is_null());
        assert_eq!(value.as_string(), None);
    }

    #[test]
    fn test_email_validation() {
        let format = FieldFormat::Email;

        assert!(format.validate(&FieldValue::String("test@example.com".to_string())));
        assert!(format.validate(&FieldValue::String(
            "user.name+tag@example.co.uk".to_string()
        )));
        assert!(!format.validate(&FieldValue::String("invalid-email".to_string())));
        assert!(!format.validate(&FieldValue::String("@example.com".to_string())));
    }

    #[test]
    fn test_uuid_validation() {
        let format = FieldFormat::Uuid;
        let valid_uuid = Uuid::new_v4().to_string();

        assert!(format.validate(&FieldValue::String(valid_uuid)));
        assert!(!format.validate(&FieldValue::String("not-a-uuid".to_string())));
    }

    #[test]
    fn test_url_validation() {
        let format = FieldFormat::Url;

        assert!(format.validate(&FieldValue::String("https://example.com".to_string())));
        assert!(format.validate(&FieldValue::String(
            "http://test.com/path?query=1".to_string()
        )));
        assert!(!format.validate(&FieldValue::String("not a url".to_string())));
    }

    #[test]
    fn test_phone_validation() {
        let format = FieldFormat::Phone;

        assert!(format.validate(&FieldValue::String("+33612345678".to_string())));
        assert!(format.validate(&FieldValue::String("33612345678".to_string())));
        assert!(!format.validate(&FieldValue::String("123".to_string())));
    }

    #[test]
    fn test_custom_regex_validation() {
        let format = FieldFormat::Custom(Regex::new(r"^[A-Z]{3}\d{3}$").unwrap());

        assert!(format.validate(&FieldValue::String("ABC123".to_string())));
        assert!(!format.validate(&FieldValue::String("abc123".to_string())));
        assert!(!format.validate(&FieldValue::String("ABCD123".to_string())));
    }

    // --- FieldValue variant coverage ---

    #[test]
    fn test_field_value_float() {
        let value = FieldValue::Float(3.14);
        assert_eq!(value.as_string(), None);
        assert_eq!(value.as_integer(), None);
        assert_eq!(value.as_uuid(), None);
        assert!(!value.is_null());
    }

    #[test]
    fn test_field_value_boolean() {
        let value = FieldValue::Boolean(true);
        assert_eq!(value.as_string(), None);
        assert_eq!(value.as_integer(), None);
        assert!(!value.is_null());
    }

    #[test]
    fn test_field_value_datetime() {
        let now = chrono::Utc::now();
        let value = FieldValue::DateTime(now);
        assert_eq!(value.as_string(), None);
        assert_eq!(value.as_integer(), None);
        assert_eq!(value.as_uuid(), None);
        assert!(!value.is_null());
    }

    #[test]
    fn test_field_value_uuid() {
        let id = Uuid::new_v4();
        let value = FieldValue::Uuid(id);
        assert_eq!(value.as_uuid(), Some(id));
        assert_eq!(value.as_string(), None);
        assert_eq!(value.as_integer(), None);
        assert!(!value.is_null());
    }

    // --- Serde roundtrip ---

    #[test]
    fn test_serde_roundtrip_string() {
        let original = FieldValue::String("hello".to_string());
        let json = serde_json::to_string(&original).expect("serialize should succeed");
        let restored: FieldValue =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_serde_roundtrip_integer() {
        let original = FieldValue::Integer(42);
        let json = serde_json::to_string(&original).expect("serialize should succeed");
        let restored: FieldValue =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_serde_roundtrip_float() {
        let original = FieldValue::Float(2.718);
        let json = serde_json::to_string(&original).expect("serialize should succeed");
        let restored: FieldValue =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_serde_roundtrip_boolean() {
        let original = FieldValue::Boolean(false);
        let json = serde_json::to_string(&original).expect("serialize should succeed");
        let restored: FieldValue =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(original, restored);
    }

    #[test]
    fn test_serde_roundtrip_null() {
        let original = FieldValue::Null;
        let json = serde_json::to_string(&original).expect("serialize should succeed");
        let restored: FieldValue =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(original, restored);
    }

    // --- FieldFormat with non-string values ---

    #[test]
    fn test_format_validate_rejects_non_string() {
        let format = FieldFormat::Email;
        assert!(!format.validate(&FieldValue::Integer(42)));
        assert!(!format.validate(&FieldValue::Boolean(true)));
        assert!(!format.validate(&FieldValue::Null));
    }
}
