//! `ContractError` across the layers of this-rs.
//!
//! `ContractError` travels inside `anyhow::Error` (the `EntityCreator` / `EntityFetcher`
//! traits are unchanged). Every surface calls [`from_anyhow`] instead of
//! `map_err(|e| Status::internal(..))`: a business or validation error keeps its code and
//! kind; only an unknown error becomes `Internal`.
//!
//! Minimal adapters (the table only) live here so the `auth` and `integration` streams do
//! not reimplement them; the `errors-i18n` stream enriches them (localization, details).

pub use crate::descriptor::error::{ERROR_HEADER, PROBLEM_JSON};
pub use crate::descriptor::{ContractError, ErrorCode, ErrorKind, FieldViolation};

/// Recover a `ContractError` from an `anyhow::Error` (searching the whole cause chain);
/// anything else becomes `this.internal` (the original text is NOT exposed to clients).
pub fn from_anyhow(err: &anyhow::Error) -> ContractError {
    for cause in err.chain() {
        if let Some(c) = cause.downcast_ref::<ContractError>() {
            return c.clone();
        }
    }
    tracing::debug!(error = %err, "error without contract: reported as this.internal");
    ContractError::framework(crate::descriptor::framework::codes::INTERNAL)
}

/// HTTP status code of a contract error.
pub fn http_status(err: &ContractError) -> axum::http::StatusCode {
    axum::http::StatusCode::from_u16(err.kind.http_status())
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
}

/// gRPC status (code from the kind table, JSON envelope in `details`).
#[cfg(feature = "grpc")]
pub fn to_status(err: ContractError) -> tonic::Status {
    err.into()
}

/// GraphQL error (`extensions.code` from the kind table, plus `this_code`, `fields`...).
#[cfg(feature = "graphql")]
pub fn to_graphql_error(err: ContractError) -> async_graphql::Error {
    err.into_graphql_error()
}

/// REST response: newtype because of the orphan rule (`IntoResponse` and `ContractError`
/// are both foreign to this-rs). Body = `ContractError::http_body` (legacy `error` /
/// `errors` keys kept), header `x-this-error: <code>`.
#[derive(Debug, Clone)]
pub struct ApiError(pub ContractError);

impl From<ContractError> for ApiError {
    fn from(e: ContractError) -> Self {
        Self(e)
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = http_status(&self.0);
        let mut res = (status, axum::Json(self.0.http_body())).into_response();
        if let Ok(v) = axum::http::HeaderValue::from_str(self.0.code.as_str()) {
            res.headers_mut().insert(ERROR_HEADER, v);
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn from_anyhow_downcasts_contract_error() {
        let e = ContractError::framework("this.validation.failed");
        let wrapped = anyhow::Error::new(e.clone()).context("while creating invoice");
        assert_eq!(from_anyhow(&wrapped), e);
    }

    #[test]
    fn from_anyhow_unknown_is_internal() {
        let e = from_anyhow(&anyhow::anyhow!("db password=secret"));
        assert_eq!(e.code.as_str(), "this.internal");
        assert_eq!(e.kind, ErrorKind::Internal);
        assert!(!e.message.contains("secret"));
    }

    #[test]
    fn api_error_status_body_and_header() {
        let res = ApiError(ContractError::framework("this.query.malformed_filter")).into_response();
        assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(res.headers()[ERROR_HEADER], "this.query.malformed_filter");
    }

    #[cfg(feature = "graphql")]
    #[test]
    fn contract_error_to_graphql_extensions() {
        let e = to_graphql_error(ContractError::framework("this.validation.failed"));
        let ext = e.extensions.expect("extensions");
        assert_eq!(
            ext.get("code"),
            Some(&async_graphql::Value::from("BAD_USER_INPUT"))
        );
        assert_eq!(
            ext.get("this_code"),
            Some(&async_graphql::Value::from("this.validation.failed"))
        );
    }

    #[cfg(feature = "grpc")]
    #[test]
    fn contract_error_to_grpc_code() {
        let s = to_status(ContractError::framework("this.validation.failed"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
        let back: ContractError = serde_json::from_slice(s.details()).unwrap();
        assert_eq!(back.code.as_str(), "this.validation.failed");
    }
}
