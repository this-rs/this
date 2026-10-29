//! Leptos glue: `ContractError` as the error type of a server function.
//!
//! ```ignore
//! #[server]
//! pub async fn pay(id: Uuid) -> Result<Invoice, ContractError> { ... }
//! ```
//!
//! The impl lives HERE (feature `leptos`) because of the orphan rule: `FromServerFnError`
//! is foreign, `ContractError` is local to this crate. Checked against
//! `server_fn 0.9.0-rc0` (`server_fn/src/error.rs`: `type Encoder`, `from_server_fn_error`,
//! `status_code`, `ser -> ServerFnErrorResponseParts`, `de`).
//!
//! - Wire format: the whole envelope as JSON (`JsonEncoding`), identical to the gRPC
//!   `details` and to `ContractError::to_json`; the front gets `code`, `kind`, `fields`,
//!   `params` and re-localizes with `I18nCatalog::localize` in the user's locale.
//! - HTTP status: the kind table (`ErrorKind::http_status`), never a blanket 500.
//! - Transport failures map to the reserved `this.transport.*` codes. Server-side failures
//!   (`ServerError`, `MiddlewareError`, `Registration`, `Response`, `Serialization`) become
//!   `this.internal` WITHOUT their text, like `from_anyhow` on the server, unless the text is
//!   itself a serialized `ContractError` (then it is kept as is).

use crate::error::ContractError;
use crate::framework::codes;
use server_fn::codec::JsonEncoding;
use server_fn::error::{FromServerFnError, ServerFnErrorErr};

/// `ServerFnErrorErr` → `ContractError` (see the module doc for the table).
pub fn from_server_fn_error_err(value: ServerFnErrorErr) -> ContractError {
    match value {
        ServerFnErrorErr::Request(reason) => {
            ContractError::framework(codes::TRANSPORT_UNAVAILABLE).with_param("reason", reason)
        }
        ServerFnErrorErr::UnsupportedRequestMethod(reason) | ServerFnErrorErr::Args(reason) => {
            ContractError::framework(codes::TRANSPORT_BAD_REQUEST).with_param("reason", reason)
        }
        ServerFnErrorErr::MissingArg(field) => {
            ContractError::framework(codes::TRANSPORT_MISSING_ARG).with_param("field", field)
        }
        ServerFnErrorErr::ServerError(text) | ServerFnErrorErr::MiddlewareError(text) => {
            serde_json::from_str::<ContractError>(&text)
                .unwrap_or_else(|_| ContractError::framework(codes::INTERNAL))
        }
        ServerFnErrorErr::Registration(_)
        | ServerFnErrorErr::Response(_)
        | ServerFnErrorErr::Serialization(_)
        | ServerFnErrorErr::Deserialization(_) => ContractError::framework(codes::INTERNAL),
    }
}

impl FromServerFnError for ContractError {
    type Encoder = JsonEncoding;

    fn from_server_fn_error(value: ServerFnErrorErr) -> Self {
        from_server_fn_error_err(value)
    }

    fn status_code(&self) -> http::StatusCode {
        self.http_status_code()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorKind, FieldViolation};

    #[test]
    fn server_fn_error_roundtrip_keeps_envelope_and_status() {
        let e = ContractError::framework(codes::VALIDATION_FAILED).with_field(
            FieldViolation::new("amount", codes::VALIDATION_POSITIVE)
                .with_param("field", "amount")
                .with_param("value", -1),
        );
        let parts = e.ser();
        assert_eq!(parts.status_code, http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(parts.content_type, "application/json");
        let back = ContractError::de(parts.body);
        assert_eq!(back, e);
    }

    #[test]
    fn server_fn_transport_errors_map_to_reserved_codes() {
        let e = ContractError::from_server_fn_error(ServerFnErrorErr::MissingArg("id".into()));
        assert_eq!(e.code.as_str(), codes::TRANSPORT_MISSING_ARG);
        assert_eq!(e.params["field"], "id");
        let e = ContractError::from_server_fn_error(ServerFnErrorErr::Request("offline".into()));
        assert_eq!(e.kind, ErrorKind::Unavailable);
        assert_eq!(e.status_code(), http::StatusCode::SERVICE_UNAVAILABLE);
        // Server text never leaks...
        let e = ContractError::from_server_fn_error(ServerFnErrorErr::ServerError(
            "db password=secret".into(),
        ));
        assert_eq!(e.code.as_str(), codes::INTERNAL);
        assert!(!e.message.contains("secret"));
        // ...unless it already is a contract envelope.
        let inner = ContractError::framework(codes::AUTH_FORBIDDEN).with_param("operation", "create");
        let e = ContractError::from_server_fn_error(ServerFnErrorErr::ServerError(
            inner.to_json().to_string(),
        ));
        assert_eq!(e, inner);
        // Undecodable body on the client: internal, not a panic.
        let e = ContractError::de(server_fn::Bytes::from_static(b"<html>502</html>"));
        assert_eq!(e.code.as_str(), codes::INTERNAL);
    }
}
