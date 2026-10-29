//! Axum extractor for validated entities
//!
//! This module provides the `Validated<T>` extractor that automatically
//! validates and filters request payloads before they reach handlers.

use super::config::{EntityValidationConfig, legacy_errors_to_contract};
use super::layer::EngineValidated;
use crate::core::errors::ERROR_HEADER;
use crate::descriptor::ContractError;
use crate::descriptor::framework::codes;
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
        // Descriptor path: the REST validation layer already ran the shared engine on this
        // body (and replaced it by the transformed payload). `Validated<T>` only extracts:
        // validating again here would be a second, divergent rule set.
        if req.extensions().get::<EngineValidated>().is_some() {
            return match Json::<Value>::from_request(req, state).await {
                Ok(Json(v)) => Ok(Validated::new(v)),
                Err(e) => Err(invalid_json(e.to_string())),
            };
        }

        // Legacy path (entity without descriptor): unchanged rules.
        // Extract the HTTP method
        let method = req.method().clone();

        // Extract JSON payload
        let Json(payload): Json<Value> = match Json::from_request(req, state).await {
            Ok(json) => json,
            Err(e) => return Err(invalid_json(e.to_string())),
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
            Err(errors) => Err(legacy_validation_response(errors)),
        }
    }
}

/// 400 for an unreadable body: legacy keys (`error: "Invalid JSON"`, `details`) plus the
/// contract keys (`code`, `kind`, ...) and the `x-this-error` header.
fn invalid_json(details: String) -> Response {
    let err = ContractError::framework(codes::TRANSPORT_BAD_REQUEST)
        .with_param("reason", details.clone());
    let mut body = err.http_body();
    body["error"] = json!("Invalid JSON");
    body["details"] = json!(details);
    with_error_header(StatusCode::BAD_REQUEST, body, &err)
}

/// 422 of the legacy rule set, additive: `error` and `errors` (strings) stay EXACTLY as
/// before; `code` / `kind` / `fields` / RFC 9457 keys are added from
/// `ContractError::http_body` so every client can switch on `code`.
pub fn legacy_validation_response(errors: Vec<String>) -> Response {
    let err = legacy_errors_to_contract(&errors);
    let mut body = err.http_body();
    body["error"] = json!("Validation failed");
    body["errors"] = json!(errors);
    with_error_header(StatusCode::UNPROCESSABLE_ENTITY, body, &err)
}

fn with_error_header(status: StatusCode, body: Value, err: &ContractError) -> Response {
    let mut res = (status, Json(body)).into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(err.code.as_str()) {
        res.headers_mut().insert(ERROR_HEADER, v);
    }
    res
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
                    if let Some(s) = value.as_str()
                        && s.len() < 2
                    {
                        return Err(format!("{} too short", field));
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

    async fn body_of(res: Response) -> Value {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn legacy_validated_body_keeps_error_and_errors_keys() {
        let req = json_request("POST", json!({"name": "  a  "}));
        let res = match Validated::<TestEntity>::from_request(req, &()).await {
            Err(r) => r,
            Ok(_) => panic!("expected a refusal"),
        };
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(res.headers()[ERROR_HEADER], "this.validation.failed");
        let body = body_of(res).await;
        // Exactly the historical keys and values...
        assert_eq!(body["error"], "Validation failed");
        assert_eq!(body["errors"], json!(["name too short"]));
        // ...plus the contract keys.
        assert_eq!(body["code"], "this.validation.failed");
        assert_eq!(body["kind"], "validation");
        assert_eq!(body["status"], 422);
        assert_eq!(body["fields"][0]["message"], "name too short");
    }

    #[tokio::test]
    async fn engine_validated_request_is_only_extracted() {
        // The layer validated (and transformed) the body: the legacy rules are NOT run
        // again (`name: null` would fail them).
        let mut req = json_request("POST", json!({"name": null}));
        req.extensions_mut().insert(EngineValidated {
            entity: "test_entity".into(),
            op: crate::descriptor::engine::validate::WriteOp::Create,
        });
        let v = Validated::<TestEntity>::from_request(req, &())
            .await
            .unwrap_or_else(|_| panic!("extracted as is"));
        assert_eq!(v.into_inner(), json!({"name": null}));
    }

    #[tokio::test]
    async fn invalid_json_keeps_legacy_keys_and_adds_code() {
        let req = Request::builder()
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from("{{{"))
            .unwrap();
        let res = match Validated::<TestEntity>::from_request(req, &()).await {
            Err(r) => r,
            Ok(_) => panic!("expected a refusal"),
        };
        let body = body_of(res).await;
        assert_eq!(body["error"], "Invalid JSON");
        assert!(body["details"].is_string());
        assert_eq!(body["code"], "this.transport.bad_request");
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
