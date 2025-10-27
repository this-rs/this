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
