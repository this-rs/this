//! Typed error handling for the this-rs framework
//!
//! This module provides a comprehensive error type hierarchy that enables
//! clients to handle errors specifically rather than dealing with generic
//! `anyhow::Error` types.
//!
//! # Error Categories
//!
//! - [`EntityError`]: Errors related to entity operations (CRUD)
//! - [`LinkError`]: Errors related to link operations
//! - [`ConfigError`]: Errors related to configuration parsing and validation
//! - [`ValidationError`]: Errors related to input validation
//! - [`StorageError`]: Errors related to storage backends
//! - [`GraphQLError`]: Errors related to GraphQL operations
//!
//! # Example
//!
//! ```rust,ignore
//! use this::prelude::*;
//!
//! async fn get_entity(id: Uuid) -> Result<Entity, ThisError> {
//!     service.get(&id).await?.ok_or(ThisError::Entity(EntityError::NotFound {
//!         entity_type: "user".to_string(),
//!         id,
//!     }))
//! }
//!
//! // Client can match specific errors
//! match result {
//!     Ok(entity) => println!("Found: {:?}", entity),
//!     Err(ThisError::Entity(EntityError::NotFound { id, .. })) => {
//!         println!("Entity {} not found", id);
//!     }
//!     Err(e) => eprintln!("Other error: {}", e),
//! }
//! ```

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::fmt;
use uuid::Uuid;

/// The main error type for the this-rs framework
///
/// This enum encompasses all possible errors that can occur within the framework.
/// Each variant contains a more specific error type for that category.
#[derive(Debug)]
pub enum ThisError {
    /// Entity-related errors (CRUD operations)
    Entity(EntityError),

    /// Link-related errors
    Link(LinkError),

    /// Configuration errors
    Config(ConfigError),

    /// Validation errors
    Validation(ValidationError),

    /// Storage backend errors
    Storage(StorageError),

    /// GraphQL-specific errors
    GraphQL(GraphQLError),

    /// HTTP/Request errors
    Request(RequestError),

    /// Internal framework errors (should not happen in normal operation)
    Internal(String),
}

impl fmt::Display for ThisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThisError::Entity(e) => write!(f, "{}", e),
            ThisError::Link(e) => write!(f, "{}", e),
            ThisError::Config(e) => write!(f, "{}", e),
            ThisError::Validation(e) => write!(f, "{}", e),
            ThisError::Storage(e) => write!(f, "{}", e),
            ThisError::GraphQL(e) => write!(f, "{}", e),
            ThisError::Request(e) => write!(f, "{}", e),
            ThisError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for ThisError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ThisError::Entity(e) => Some(e),
            ThisError::Link(e) => Some(e),
            ThisError::Config(e) => Some(e),
            ThisError::Validation(e) => Some(e),
            ThisError::Storage(e) => Some(e),
            ThisError::GraphQL(e) => Some(e),
            ThisError::Request(e) => Some(e),
            ThisError::Internal(_) => None,
        }
    }
}

/// Error response structure for HTTP responses
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// Error code for programmatic handling
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Optional additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ThisError {
    /// Get the HTTP status code for this error
    pub fn status_code(&self) -> StatusCode {
        match self {
            ThisError::Entity(e) => e.status_code(),
            ThisError::Link(e) => e.status_code(),
            ThisError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ThisError::Validation(_) => StatusCode::BAD_REQUEST,
            ThisError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ThisError::GraphQL(e) => e.status_code(),
            ThisError::Request(e) => e.status_code(),
            ThisError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Get the error code for this error
    pub fn error_code(&self) -> &'static str {
        match self {
            ThisError::Entity(e) => e.error_code(),
            ThisError::Link(e) => e.error_code(),
            ThisError::Config(_) => "CONFIG_ERROR",
            ThisError::Validation(_) => "VALIDATION_ERROR",
            ThisError::Storage(_) => "STORAGE_ERROR",
            ThisError::GraphQL(e) => e.error_code(),
            ThisError::Request(e) => e.error_code(),
            ThisError::Internal(_) => "INTERNAL_ERROR",
        }
    }

    /// Convert to an error response
    pub fn to_response(&self) -> ErrorResponse {
        ErrorResponse {
            code: self.error_code().to_string(),
            message: self.to_string(),
            details: self.details(),
        }
    }

    /// Get additional details for the error
    fn details(&self) -> Option<serde_json::Value> {
        match self {
            ThisError::Entity(EntityError::NotFound { entity_type, id }) => {
                Some(serde_json::json!({
                    "entity_type": entity_type,
                    "id": id.to_string()
                }))
            }
            ThisError::Link(LinkError::NotFound {
                source_id,
                target_id,
                link_type,
            }) => Some(serde_json::json!({
                "source_id": source_id.to_string(),
                "target_id": target_id.to_string(),
                "link_type": link_type
            })),
            ThisError::Validation(ValidationError::FieldErrors(errors)) => {
                Some(serde_json::json!({ "fields": errors }))
            }
            _ => None,
        }
    }
}

impl IntoResponse for ThisError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = Json(self.to_response());
        (status, body).into_response()
    }
}

// =============================================================================
// Entity Errors
// =============================================================================

/// Errors related to entity operations
#[derive(Debug)]
pub enum EntityError {
    /// Entity was not found
    NotFound {
        entity_type: String,
        id: Uuid,
    },

    /// Entity already exists (conflict)
    AlreadyExists {
        entity_type: String,
        id: Uuid,
    },

    /// Entity type is not registered
    UnknownType {
        entity_type: String,
    },

    /// Failed to serialize/deserialize entity
    SerializationError {
        entity_type: String,
        message: String,
    },

    /// Entity operation failed
    OperationFailed {
        entity_type: String,
        operation: String,
        message: String,
    },
}

impl fmt::Display for EntityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntityError::NotFound { entity_type, id } => {
                write!(f, "{} with id '{}' not found", entity_type, id)
            }
            EntityError::AlreadyExists { entity_type, id } => {
                write!(f, "{} with id '{}' already exists", entity_type, id)
            }
            EntityError::UnknownType { entity_type } => {
                write!(f, "Unknown entity type: {}", entity_type)
            }
            EntityError::SerializationError {
                entity_type,
                message,
            } => {
                write!(
                    f,
                    "Failed to serialize/deserialize {}: {}",
                    entity_type, message
                )
            }
            EntityError::OperationFailed {
                entity_type,
                operation,
                message,
            } => {
                write!(
                    f,
                    "Failed to {} {}: {}",
                    operation, entity_type, message
                )
            }
        }
    }
}

impl std::error::Error for EntityError {}

impl EntityError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            EntityError::NotFound { .. } => StatusCode::NOT_FOUND,
            EntityError::AlreadyExists { .. } => StatusCode::CONFLICT,
            EntityError::UnknownType { .. } => StatusCode::BAD_REQUEST,
            EntityError::SerializationError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            EntityError::OperationFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            EntityError::NotFound { .. } => "ENTITY_NOT_FOUND",
            EntityError::AlreadyExists { .. } => "ENTITY_ALREADY_EXISTS",
            EntityError::UnknownType { .. } => "UNKNOWN_ENTITY_TYPE",
            EntityError::SerializationError { .. } => "ENTITY_SERIALIZATION_ERROR",
            EntityError::OperationFailed { .. } => "ENTITY_OPERATION_FAILED",
        }
    }
}

impl From<EntityError> for ThisError {
    fn from(err: EntityError) -> Self {
        ThisError::Entity(err)
    }
}

// =============================================================================
// Link Errors
// =============================================================================

/// Errors related to link operations
#[derive(Debug)]
pub enum LinkError {
    /// Link was not found
    NotFound {
        source_id: Uuid,
        target_id: Uuid,
        link_type: String,
    },

    /// Link not found by ID
    NotFoundById {
        id: Uuid,
    },

    /// Link already exists
    AlreadyExists {
        source_id: Uuid,
        target_id: Uuid,
        link_type: String,
    },

    /// Invalid link type
    InvalidLinkType {
        link_type: String,
        message: String,
    },

    /// Route not found for entity
    RouteNotFound {
        entity_type: String,
        route_name: String,
    },

    /// Link chain validation failed (for nested routes)
    ChainValidationFailed {
        message: String,
    },

    /// Link operation failed
    OperationFailed {
        operation: String,
        message: String,
    },
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::NotFound {
                source_id,
                target_id,
                link_type,
            } => {
                write!(
                    f,
                    "Link '{}' from '{}' to '{}' not found",
                    link_type, source_id, target_id
                )
            }
            LinkError::NotFoundById { id } => {
                write!(f, "Link with id '{}' not found", id)
            }
            LinkError::AlreadyExists {
                source_id,
                target_id,
                link_type,
            } => {
                write!(
                    f,
                    "Link '{}' from '{}' to '{}' already exists",
                    link_type, source_id, target_id
                )
            }
            LinkError::InvalidLinkType { link_type, message } => {
                write!(f, "Invalid link type '{}': {}", link_type, message)
            }
            LinkError::RouteNotFound {
                entity_type,
                route_name,
            } => {
                write!(
                    f,
                    "Route '{}' not found for entity type '{}'",
                    route_name, entity_type
                )
            }
            LinkError::ChainValidationFailed { message } => {
                write!(f, "Link chain validation failed: {}", message)
            }
            LinkError::OperationFailed { operation, message } => {
                write!(f, "Link {} failed: {}", operation, message)
            }
        }
    }
}

impl std::error::Error for LinkError {}

impl LinkError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            LinkError::NotFound { .. } => StatusCode::NOT_FOUND,
            LinkError::NotFoundById { .. } => StatusCode::NOT_FOUND,
            LinkError::AlreadyExists { .. } => StatusCode::CONFLICT,
            LinkError::InvalidLinkType { .. } => StatusCode::BAD_REQUEST,
            LinkError::RouteNotFound { .. } => StatusCode::NOT_FOUND,
            LinkError::ChainValidationFailed { .. } => StatusCode::BAD_REQUEST,
            LinkError::OperationFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            LinkError::NotFound { .. } => "LINK_NOT_FOUND",
            LinkError::NotFoundById { .. } => "LINK_NOT_FOUND",
            LinkError::AlreadyExists { .. } => "LINK_ALREADY_EXISTS",
            LinkError::InvalidLinkType { .. } => "INVALID_LINK_TYPE",
            LinkError::RouteNotFound { .. } => "ROUTE_NOT_FOUND",
            LinkError::ChainValidationFailed { .. } => "CHAIN_VALIDATION_FAILED",
            LinkError::OperationFailed { .. } => "LINK_OPERATION_FAILED",
        }
    }
}

impl From<LinkError> for ThisError {
    fn from(err: LinkError) -> Self {
        ThisError::Link(err)
    }
}

// =============================================================================
// Config Errors
// =============================================================================

/// Errors related to configuration
#[derive(Debug)]
pub enum ConfigError {
    /// Failed to parse configuration file
    ParseError {
        file: Option<String>,
        message: String,
    },

    /// Missing required field in configuration
    MissingField {
        field: String,
        context: String,
    },

    /// Invalid value in configuration
    InvalidValue {
        field: String,
        value: String,
        message: String,
    },

    /// Configuration file not found
    FileNotFound {
        path: String,
    },

    /// IO error while reading configuration
    IoError {
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::ParseError { file, message } => {
                if let Some(file) = file {
                    write!(f, "Failed to parse config file '{}': {}", file, message)
                } else {
                    write!(f, "Failed to parse config: {}", message)
                }
            }
            ConfigError::MissingField { field, context } => {
                write!(f, "Missing required field '{}' in {}", field, context)
            }
            ConfigError::InvalidValue {
                field,
                value,
                message,
            } => {
                write!(
                    f,
                    "Invalid value '{}' for field '{}': {}",
                    value, field, message
                )
            }
            ConfigError::FileNotFound { path } => {
                write!(f, "Configuration file not found: {}", path)
            }
            ConfigError::IoError { message } => {
                write!(f, "IO error: {}", message)
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<ConfigError> for ThisError {
    fn from(err: ConfigError) -> Self {
        ThisError::Config(err)
    }
}

// =============================================================================
// Validation Errors
// =============================================================================

/// Errors related to input validation
#[derive(Debug)]
pub enum ValidationError {
    /// Single field validation error
    FieldError {
        field: String,
        message: String,
    },

    /// Multiple field validation errors
    FieldErrors(Vec<FieldValidationError>),

    /// Invalid JSON format
    InvalidJson {
        message: String,
    },

    /// Missing required argument
    MissingArgument {
        argument: String,
    },

    /// Invalid UUID format
    InvalidUuid {
        value: String,
    },
}

/// A single field validation error
#[derive(Debug, Clone, Serialize)]
pub struct FieldValidationError {
    pub field: String,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::FieldError { field, message } => {
                write!(f, "Validation error for field '{}': {}", field, message)
            }
            ValidationError::FieldErrors(errors) => {
                let msgs: Vec<String> = errors
                    .iter()
                    .map(|e| format!("{}: {}", e.field, e.message))
                    .collect();
                write!(f, "Validation errors: {}", msgs.join(", "))
            }
            ValidationError::InvalidJson { message } => {
                write!(f, "Invalid JSON: {}", message)
            }
            ValidationError::MissingArgument { argument } => {
                write!(f, "Missing required argument: {}", argument)
            }
            ValidationError::InvalidUuid { value } => {
                write!(f, "Invalid UUID format: {}", value)
            }
        }
    }
}

impl std::error::Error for ValidationError {}

impl From<ValidationError> for ThisError {
    fn from(err: ValidationError) -> Self {
        ThisError::Validation(err)
    }
}

// =============================================================================
// Storage Errors
// =============================================================================

/// Errors related to storage backends
#[derive(Debug)]
pub enum StorageError {
    /// Connection error
    ConnectionError {
        backend: String,
        message: String,
    },

    /// Query execution error
    QueryError {
        backend: String,
        message: String,
    },

    /// Transaction error
    TransactionError {
        message: String,
    },

    /// Data integrity error
    IntegrityError {
        message: String,
    },

    /// Backend not available
    Unavailable {
        backend: String,
    },
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::ConnectionError { backend, message } => {
                write!(f, "Failed to connect to {}: {}", backend, message)
            }
            StorageError::QueryError { backend, message } => {
                write!(f, "{} query error: {}", backend, message)
            }
            StorageError::TransactionError { message } => {
                write!(f, "Transaction error: {}", message)
            }
            StorageError::IntegrityError { message } => {
                write!(f, "Data integrity error: {}", message)
            }
            StorageError::Unavailable { backend } => {
                write!(f, "Storage backend '{}' is unavailable", backend)
            }
        }
    }
}

impl std::error::Error for StorageError {}

impl From<StorageError> for ThisError {
    fn from(err: StorageError) -> Self {
        ThisError::Storage(err)
    }
}

// =============================================================================
// GraphQL Errors
// =============================================================================

/// Errors related to GraphQL operations
#[derive(Debug)]
pub enum GraphQLError {
    /// Query parsing error
    ParseError {
        message: String,
    },

    /// Query execution error
    ExecutionError {
        message: String,
    },

    /// Invalid operation
    InvalidOperation {
        operation: String,
        message: String,
    },

    /// Field resolution error
    FieldResolutionError {
        field: String,
        message: String,
    },
}

impl fmt::Display for GraphQLError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphQLError::ParseError { message } => {
                write!(f, "GraphQL parse error: {}", message)
            }
            GraphQLError::ExecutionError { message } => {
                write!(f, "GraphQL execution error: {}", message)
            }
            GraphQLError::InvalidOperation { operation, message } => {
                write!(f, "Invalid GraphQL operation '{}': {}", operation, message)
            }
            GraphQLError::FieldResolutionError { field, message } => {
                write!(f, "Failed to resolve field '{}': {}", field, message)
            }
        }
    }
}

impl std::error::Error for GraphQLError {}

impl GraphQLError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            GraphQLError::ParseError { .. } => StatusCode::BAD_REQUEST,
            GraphQLError::ExecutionError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            GraphQLError::InvalidOperation { .. } => StatusCode::BAD_REQUEST,
            GraphQLError::FieldResolutionError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            GraphQLError::ParseError { .. } => "GRAPHQL_PARSE_ERROR",
            GraphQLError::ExecutionError { .. } => "GRAPHQL_EXECUTION_ERROR",
            GraphQLError::InvalidOperation { .. } => "GRAPHQL_INVALID_OPERATION",
            GraphQLError::FieldResolutionError { .. } => "GRAPHQL_FIELD_RESOLUTION_ERROR",
        }
    }
}

impl From<GraphQLError> for ThisError {
    fn from(err: GraphQLError) -> Self {
        ThisError::GraphQL(err)
    }
}

// =============================================================================
// Request Errors
// =============================================================================

/// Errors related to HTTP requests
#[derive(Debug)]
pub enum RequestError {
    /// Invalid path format
    InvalidPath {
        path: String,
        message: String,
    },

    /// Invalid entity ID format
    InvalidEntityId {
        id: String,
    },

    /// Invalid request body
    InvalidBody {
        message: String,
    },

    /// Missing required header
    MissingHeader {
        header: String,
    },

    /// Unauthorized request
    Unauthorized {
        message: String,
    },

    /// Forbidden operation
    Forbidden {
        message: String,
    },

    /// Method not allowed
    MethodNotAllowed {
        method: String,
        path: String,
    },
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestError::InvalidPath { path, message } => {
                write!(f, "Invalid path '{}': {}", path, message)
            }
            RequestError::InvalidEntityId { id } => {
                write!(f, "Invalid entity ID format: '{}'", id)
            }
            RequestError::InvalidBody { message } => {
                write!(f, "Invalid request body: {}", message)
            }
            RequestError::MissingHeader { header } => {
                write!(f, "Missing required header: {}", header)
            }
            RequestError::Unauthorized { message } => {
                write!(f, "Unauthorized: {}", message)
            }
            RequestError::Forbidden { message } => {
                write!(f, "Forbidden: {}", message)
            }
            RequestError::MethodNotAllowed { method, path } => {
                write!(f, "Method {} not allowed on {}", method, path)
            }
        }
    }
}

impl std::error::Error for RequestError {}

impl RequestError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            RequestError::InvalidPath { .. } => StatusCode::BAD_REQUEST,
            RequestError::InvalidEntityId { .. } => StatusCode::BAD_REQUEST,
            RequestError::InvalidBody { .. } => StatusCode::BAD_REQUEST,
            RequestError::MissingHeader { .. } => StatusCode::BAD_REQUEST,
            RequestError::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            RequestError::Forbidden { .. } => StatusCode::FORBIDDEN,
            RequestError::MethodNotAllowed { .. } => StatusCode::METHOD_NOT_ALLOWED,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            RequestError::InvalidPath { .. } => "INVALID_PATH",
            RequestError::InvalidEntityId { .. } => "INVALID_ENTITY_ID",
            RequestError::InvalidBody { .. } => "INVALID_BODY",
            RequestError::MissingHeader { .. } => "MISSING_HEADER",
            RequestError::Unauthorized { .. } => "UNAUTHORIZED",
            RequestError::Forbidden { .. } => "FORBIDDEN",
            RequestError::MethodNotAllowed { .. } => "METHOD_NOT_ALLOWED",
        }
    }
}

impl From<RequestError> for ThisError {
    fn from(err: RequestError) -> Self {
        ThisError::Request(err)
    }
}

// =============================================================================
// Conversions from external errors
// =============================================================================

impl From<serde_json::Error> for ThisError {
    fn from(err: serde_json::Error) -> Self {
        ThisError::Validation(ValidationError::InvalidJson {
            message: err.to_string(),
        })
    }
}

impl From<std::io::Error> for ThisError {
    fn from(err: std::io::Error) -> Self {
        ThisError::Config(ConfigError::IoError {
            message: err.to_string(),
        })
    }
}

impl From<serde_yaml::Error> for ThisError {
    fn from(err: serde_yaml::Error) -> Self {
        ThisError::Config(ConfigError::ParseError {
            file: None,
            message: err.to_string(),
        })
    }
}

impl From<uuid::Error> for ThisError {
    fn from(err: uuid::Error) -> Self {
        ThisError::Validation(ValidationError::InvalidUuid {
            value: err.to_string(),
        })
    }
}

/// Convert from anyhow::Error for backwards compatibility
impl From<anyhow::Error> for ThisError {
    fn from(err: anyhow::Error) -> Self {
        // Try to downcast to known error types
        if let Some(this_err) = err.downcast_ref::<ThisError>() {
            // Can't move out of reference, so we recreate
            ThisError::Internal(this_err.to_string())
        } else {
            ThisError::Internal(err.to_string())
        }
    }
}

// =============================================================================
// Result type alias
// =============================================================================

/// A specialized Result type for this-rs operations
pub type ThisResult<T> = Result<T, ThisError>;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_error_display() {
        let err = EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::nil(),
        };
        assert!(err.to_string().contains("user"));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_entity_error_status_code() {
        let err = EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::nil(),
        };
        assert_eq!(err.status_code(), StatusCode::NOT_FOUND);

        let err = EntityError::AlreadyExists {
            entity_type: "user".to_string(),
            id: Uuid::nil(),
        };
        assert_eq!(err.status_code(), StatusCode::CONFLICT);
    }

    #[test]
    fn test_link_error_display() {
        let err = LinkError::NotFound {
            source_id: Uuid::nil(),
            target_id: Uuid::nil(),
            link_type: "owner".to_string(),
        };
        assert!(err.to_string().contains("owner"));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_validation_error_multiple_fields() {
        let err = ValidationError::FieldErrors(vec![
            FieldValidationError {
                field: "name".to_string(),
                message: "required".to_string(),
            },
            FieldValidationError {
                field: "email".to_string(),
                message: "invalid format".to_string(),
            },
        ]);
        let display = err.to_string();
        assert!(display.contains("name"));
        assert!(display.contains("email"));
    }

    #[test]
    fn test_this_error_conversion() {
        let entity_err = EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::nil(),
        };
        let this_err: ThisError = entity_err.into();
        assert_eq!(this_err.status_code(), StatusCode::NOT_FOUND);
        assert_eq!(this_err.error_code(), "ENTITY_NOT_FOUND");
    }

    #[test]
    fn test_error_response_serialization() {
        let err = ThisError::Entity(EntityError::NotFound {
            entity_type: "user".to_string(),
            id: Uuid::nil(),
        });
        let response = err.to_response();
        assert_eq!(response.code, "ENTITY_NOT_FOUND");
        assert!(response.details.is_some());
    }

    #[test]
    fn test_request_error_status_codes() {
        assert_eq!(
            RequestError::Unauthorized {
                message: "test".to_string()
            }
            .status_code(),
            StatusCode::UNAUTHORIZED
        );

        assert_eq!(
            RequestError::Forbidden {
                message: "test".to_string()
            }
            .status_code(),
            StatusCode::FORBIDDEN
        );

        assert_eq!(
            RequestError::InvalidPath {
                path: "/test".to_string(),
                message: "invalid".to_string()
            }
            .status_code(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn test_storage_error() {
        let err = StorageError::ConnectionError {
            backend: "PostgreSQL".to_string(),
            message: "connection refused".to_string(),
        };
        assert!(err.to_string().contains("PostgreSQL"));
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn test_graphql_error() {
        let err = GraphQLError::ParseError {
            message: "unexpected token".to_string(),
        };
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(err.error_code(), "GRAPHQL_PARSE_ERROR");
    }

    #[test]
    fn test_config_error() {
        let err = ConfigError::FileNotFound {
            path: "/etc/config.yaml".to_string(),
        };
        assert!(err.to_string().contains("/etc/config.yaml"));
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let this_err: ThisError = json_err.into();
        assert!(matches!(
            this_err,
            ThisError::Validation(ValidationError::InvalidJson { .. })
        ));
    }
}
