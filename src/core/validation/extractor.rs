//! Axum extractor for validated entities
//!
//! This module provides the `Validated<T>` extractor that automatically
//! validates and filters request payloads before they reach handlers.

use super::config::EntityValidationConfig;
use axum::{
    Json,
    extract::{FromRequest, Request},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

/// Trait for entities that support validation
///
/// This is automatically implemented by the `impl_data_entity_validated!` macro
pub trait ValidatableEntity {
    /// Get the validation configuration for a specific operation
    fn validation_config(operation: &str) -> EntityValidationConfig;
}

/// Axum extractor that validates and filters entity data
///
/// # Usage
///
/// ```rust,ignore
/// pub async fn create_invoice(
///     Validated::<Invoice>(payload): Validated<Invoice>,
/// ) -> Result<Json<Invoice>, StatusCode> {
///     // payload is already validated and filtered!
/// }
/// ```
pub struct Validated<T>(pub Value, std::marker::PhantomData<T>);

impl<T> Validated<T> {
    /// Create a new validated payload
    pub fn new(payload: Value) -> Self {
        Self(payload, std::marker::PhantomData)
    }

    /// Get the inner payload
    pub fn into_inner(self) -> Value {
        self.0
    }
}

// Allow dereferencing to Value
impl<T> std::ops::Deref for Validated<T> {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, T> FromRequest<S> for Validated<T>
where
    S: Send + Sync,
    T: ValidatableEntity + Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // Extract the HTTP method
        let method = req.method().clone();

        // Extract JSON payload
        let Json(payload): Json<Value> = match Json::from_request(req, state).await {
            Ok(json) => json,
            Err(e) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "Invalid JSON",
                        "details": e.to_string()
                    })),
                )
                    .into_response());
            }
        };

        // Determine operation from HTTP method
        let operation = match method.as_str() {
            "POST" => "create",
            "PUT" | "PATCH" => "update",
            _ => "create", // default
        };

        // Get validation config from entity
        let config = T::validation_config(operation);

        // Validate and filter
        match config.validate_and_filter(payload) {
            Ok(validated_payload) => Ok(Validated::new(validated_payload)),
            Err(errors) => Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": "Validation failed",
                    "errors": errors
                })),
            )
                .into_response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::Request;
    use serde_json::json;

    /// Dummy entity that implements ValidatableEntity for testing.
    /// - "create" operation: requires "name" (not null), min length 2
    /// - "update" operation: no validators (everything passes)
    struct TestEntity;

    impl ValidatableEntity for TestEntity {
        fn validation_config(operation: &str) -> EntityValidationConfig {
            let mut config = EntityValidationConfig::new("test_entity");
            if operation == "create" {
                config.add_validator("name", |field, value| {
                    if value.is_null() {
                        Err(format!("{} is required", field))
                    } else {
                        Ok(())
                    }
                });
                config.add_validator("name", |field, value| {
                    if let Some(s) = value.as_str() {
                        if s.len() < 2 {
                            return Err(format!("{} too short", field));
                        }
                    }
                    Ok(())
                });
                config.add_filter("name", |_field, value| {
                    if let Some(s) = value.as_str() {
                        Ok(Value::String(s.trim().to_string()))
                    } else {
                        Ok(value)
                    }
                });
            }
            config
        }
    }

    /// Helper: build an HTTP request with JSON body and given method.
    fn json_request(method: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    // === Validated::new / into_inner / Deref ===

    #[test]
    fn test_validated_new_and_into_inner() {
        let val = json!({"name": "test"});
        let validated = Validated::<TestEntity>::new(val.clone());
        assert_eq!(validated.into_inner(), val);
    }

    #[test]
    fn test_validated_deref() {
        let val = json!({"key": 42});
        let validated = Validated::<TestEntity>::new(val);
        // Deref allows accessing Value methods directly
        assert_eq!(validated["key"], 42);
        assert!(validated.is_object());
    }

    // === FromRequest ===

    #[tokio::test]
    async fn test_from_request_post_valid_payload() {
        let req = json_request("POST", json!({"name": "  Alice  "}));
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        assert!(result.is_ok());
        let validated = result.unwrap();
        // Filter should have trimmed whitespace
        assert_eq!(validated.0["name"], "Alice");
    }

    #[tokio::test]
    async fn test_from_request_post_validation_failure() {
        // "name" is null → required validator fails
        let req = json_request("POST", json!({"name": null}));
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        assert!(result.is_err());
        match result {
            Err(response) => assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY),
            Ok(_) => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn test_from_request_post_too_short_after_trim() {
        // "  a  " → trim → "a" (length 1 < 2) → fails
        let req = json_request("POST", json!({"name": "  a  "}));
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        assert!(result.is_err());
        match result {
            Err(response) => assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY),
            Ok(_) => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn test_from_request_put_uses_update_operation() {
        // "update" operation has no validators → everything passes
        let req = json_request("PUT", json!({"name": null}));
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_from_request_patch_uses_update_operation() {
        let req = json_request("PATCH", json!({"name": null}));
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_from_request_get_defaults_to_create_operation() {
        // GET defaults to "create" operation → name=null fails validation
        let req = json_request("GET", json!({"name": null}));
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_from_request_invalid_json_returns_400() {
        let req = Request::builder()
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from("not valid json {{{"))
            .unwrap();
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        match result {
            Err(response) => assert_eq!(response.status(), StatusCode::BAD_REQUEST),
            Ok(_) => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn test_from_request_missing_content_type_returns_400() {
        let req = Request::builder()
            .method("POST")
            .body(Body::from(r#"{"name": "test"}"#))
            .unwrap();
        let result = Validated::<TestEntity>::from_request(req, &()).await;
        match result {
            Err(response) => assert_eq!(response.status(), StatusCode::BAD_REQUEST),
            Ok(_) => panic!("expected error"),
        }
    }
}
