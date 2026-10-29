//! `ContractError` across the layers of this-rs.
//!
//! `ContractError` travels inside `anyhow::Error` (the `EntityCreator` / `EntityFetcher`
//! traits are unchanged). Every surface calls [`from_anyhow`] instead of
//! `map_err(|e| Status::internal(..))`: a business or validation error keeps its code and
//! kind; only an unknown error becomes `this.internal`.
//!
//! One table (`ErrorKind`: HTTP / gRPC / GraphQL), one envelope, one catalog:
//! - [`http`]: [`ApiError`] (`IntoResponse`, header `x-this-error`, RFC 9457 when
//!   accepted) and the [`localize_errors`] middleware (message in the request locale);
//! - [`grpc`] (feature `grpc`): `Status` with the mapped code, the JSON envelope in
//!   `details`, `x-this-error` metadata, localized message;
//! - [`graphql`] (feature `graphql`): `async_graphql::Error` with `extensions.code` from the
//!   table plus `this_code`, `kind`, `message_key`, `params`, `fields`.
//!
//! The constructors below ([`not_found`], [`forbidden`]...) are the stable codes the ad hoc
//! `StatusCode::NOT_FOUND` / `Status::not_found` / `Error::new("...")` sites migrate to.

pub mod http;

#[cfg(feature = "grpc")]
pub mod grpc;

#[cfg(feature = "graphql")]
pub mod graphql;

pub use self::http::{ApiError, ApiResult, accepts_problem_json, error_response, localize_errors};
pub use crate::descriptor::error::{ERROR_HEADER, PROBLEM_JSON};
pub use crate::descriptor::framework::codes;
pub use crate::descriptor::{ContractError, ErrorCode, ErrorKind, FieldViolation};

#[cfg(feature = "graphql")]
pub use self::graphql::{GraphqlResultExt, to_graphql_error, to_graphql_error_localized};
#[cfg(feature = "grpc")]
pub use self::grpc::{StatusResultExt, status_for, to_status, to_status_localized};

/// Recover a `ContractError` from an `anyhow::Error` (searching the whole cause chain);
/// anything else becomes `this.internal` (the original text is NOT exposed to clients, it
/// is logged at `debug`).
pub fn from_anyhow(err: &anyhow::Error) -> ContractError {
    for cause in err.chain() {
        if let Some(c) = cause.downcast_ref::<ContractError>() {
            return c.clone();
        }
    }
    tracing::debug!(error = %err, "error without contract: reported as this.internal");
    ContractError::framework(codes::INTERNAL)
}

/// HTTP status code of a contract error.
pub fn http_status(err: &ContractError) -> axum::http::StatusCode {
    axum::http::StatusCode::from_u16(err.kind.http_status())
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
}

/// `this.entity.not_found` (`{entity} not found: {id}`).
pub fn not_found(entity: &str, id: impl std::fmt::Display) -> ContractError {
    ContractError::framework(codes::ENTITY_NOT_FOUND)
        .with_param("entity", entity)
        .with_param("id", id.to_string())
}

/// `this.entity.conflict` (`{entity} already exists`).
pub fn conflict(entity: &str) -> ContractError {
    ContractError::framework(codes::ENTITY_CONFLICT).with_param("entity", entity)
}

/// `this.auth.unauthenticated`.
pub fn unauthenticated() -> ContractError {
    ContractError::framework(codes::AUTH_UNAUTHENTICATED)
}

/// `this.auth.forbidden` (`Operation denied: {operation}`).
pub fn forbidden(operation: impl std::fmt::Display) -> ContractError {
    ContractError::framework(codes::AUTH_FORBIDDEN).with_param("operation", operation.to_string())
}

/// `this.transport.bad_request` (`Bad request: {reason}`): a malformed request that no
/// more specific code (`this.query.*`, `this.validation.*`) describes.
pub fn bad_request(reason: impl std::fmt::Display) -> ContractError {
    ContractError::framework(codes::TRANSPORT_BAD_REQUEST).with_param("reason", reason.to_string())
}

/// `this.validation.failed` carrying `violations` (one per field).
pub fn validation(violations: impl IntoIterator<Item = FieldViolation>) -> ContractError {
    violations.into_iter().fold(
        ContractError::framework(codes::VALIDATION_FAILED),
        |e, v| e.with_field(v),
    )
}

/// `this.internal` (no detail ever reaches the client).
pub fn internal() -> ContractError {
    ContractError::framework(codes::INTERNAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_anyhow_downcasts_contract_error() {
        let e = ContractError::framework("this.validation.failed");
        let wrapped = anyhow::Error::new(e.clone()).context("while creating invoice");
        assert_eq!(from_anyhow(&wrapped), e);
        // Deeper in the chain, behind two contexts.
        let deep = anyhow::Error::new(not_found("invoice", 7))
            .context("fetch")
            .context("resolver");
        assert_eq!(from_anyhow(&deep).code.as_str(), codes::ENTITY_NOT_FOUND);
    }

    #[test]
    fn from_anyhow_unknown_is_internal() {
        let e = from_anyhow(&anyhow::anyhow!("db password=secret"));
        assert_eq!(e.code.as_str(), "this.internal");
        assert_eq!(e.kind, ErrorKind::Internal);
        assert!(!e.message.contains("secret"));
        assert!(e.params.is_empty() && e.fields.is_empty());
    }

    #[test]
    fn constructors_use_stable_codes_and_kinds() {
        let cases = [
            (not_found("invoice", 1), "this.entity.not_found", 404),
            (conflict("invoice"), "this.entity.conflict", 409),
            (unauthenticated(), "this.auth.unauthenticated", 401),
            (forbidden("create"), "this.auth.forbidden", 403),
            (bad_request("eof"), "this.transport.bad_request", 400),
            (validation([]), "this.validation.failed", 422),
            (internal(), "this.internal", 500),
        ];
        for (e, code, status) in cases {
            assert_eq!(e.code.as_str(), code);
            assert_eq!(http_status(&e).as_u16(), status, "{code}");
            assert!(
                !e.message.contains('{'),
                "{code}: params filled: {}",
                e.message
            );
        }
        assert_eq!(not_found("invoice", 1).message, "invoice introuvable: 1");
    }
}
