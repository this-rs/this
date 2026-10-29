//! Errors: stable codes, one kind table for every protocol, one envelope.
//!
//! `ContractError` is THE error that crosses every surface (REST, GraphQL, gRPC,
//! WebSocket, Leptos front). It travels inside `anyhow::Error` between layers of
//! this-rs (`EntityCreator` / `EntityFetcher` are unchanged) and each surface
//! downcasts it (`this::core::errors::from_anyhow`).

use crate::i18n::MessageKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// HTTP header carrying the code of a structured error (`x-this-error: this.validation.failed`).
///
/// Copied from Leptos' `SERVER_FN_ERROR_HEADER`: a client tells a `ContractError` from a raw
/// 500 without reading the body.
pub const ERROR_HEADER: &str = "x-this-error";

/// Media type of the RFC 9457 problem document, served when the client accepts it.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// Stable code, `<namespace>.<subject>.<reason>`: `this.validation.required`,
/// `billing.invoice.already_paid`. The namespace is the module name; `this.` is reserved
/// for the framework (enforced by `ModuleDescriptor::check`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorCode(pub String);

impl ErrorCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// First segment (`billing` for `billing.invoice.already_paid`).
    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl From<&str> for ErrorCode {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
impl From<String> for ErrorCode {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Category of an error. One kind = one HTTP status, one gRPC code, one GraphQL code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorKind {
    BadRequest,
    Validation,
    Unauthenticated,
    PermissionDenied,
    NotFound,
    /// The resource already exists (create conflict).
    Conflict,
    /// Optimistic concurrency conflict (version / etag mismatch on a write).
    Aborted,
    PreconditionFailed,
    RateLimited,
    NotImplemented,
    Unavailable,
    Timeout,
    Internal,
}

impl ErrorKind {
    /// Every kind, in declaration order (used by the exhaustive mapping test).
    pub const ALL: [ErrorKind; 13] = [
        ErrorKind::BadRequest,
        ErrorKind::Validation,
        ErrorKind::Unauthenticated,
        ErrorKind::PermissionDenied,
        ErrorKind::NotFound,
        ErrorKind::Conflict,
        ErrorKind::Aborted,
        ErrorKind::PreconditionFailed,
        ErrorKind::RateLimited,
        ErrorKind::NotImplemented,
        ErrorKind::Unavailable,
        ErrorKind::Timeout,
        ErrorKind::Internal,
    ];

    /// HTTP status code.
    pub const fn http_status(self) -> u16 {
        match self {
            ErrorKind::BadRequest => 400,
            ErrorKind::Validation => 422,
            ErrorKind::Unauthenticated => 401,
            ErrorKind::PermissionDenied => 403,
            ErrorKind::NotFound => 404,
            ErrorKind::Conflict => 409,
            ErrorKind::Aborted => 409,
            ErrorKind::PreconditionFailed => 412,
            ErrorKind::RateLimited => 429,
            ErrorKind::NotImplemented => 501,
            ErrorKind::Unavailable => 503,
            ErrorKind::Timeout => 504,
            ErrorKind::Internal => 500,
        }
    }

    /// gRPC status code, numeric values of `tonic::Code` (no dependency on tonic).
    pub const fn grpc_code(self) -> i32 {
        match self {
            ErrorKind::BadRequest => 3,         // INVALID_ARGUMENT
            ErrorKind::Validation => 3,         // INVALID_ARGUMENT
            ErrorKind::Unauthenticated => 16,   // UNAUTHENTICATED
            ErrorKind::PermissionDenied => 7,   // PERMISSION_DENIED
            ErrorKind::NotFound => 5,           // NOT_FOUND
            ErrorKind::Conflict => 6,           // ALREADY_EXISTS
            ErrorKind::Aborted => 10,           // ABORTED
            ErrorKind::PreconditionFailed => 9, // FAILED_PRECONDITION
            ErrorKind::RateLimited => 8,        // RESOURCE_EXHAUSTED
            ErrorKind::NotImplemented => 12,    // UNIMPLEMENTED
            ErrorKind::Unavailable => 14,       // UNAVAILABLE
            ErrorKind::Timeout => 4,            // DEADLINE_EXCEEDED
            ErrorKind::Internal => 13,          // INTERNAL
        }
    }

    /// GraphQL `extensions.code`.
    pub const fn graphql_code(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "BAD_REQUEST",
            ErrorKind::Validation => "BAD_USER_INPUT",
            ErrorKind::Unauthenticated => "UNAUTHENTICATED",
            ErrorKind::PermissionDenied => "FORBIDDEN",
            ErrorKind::NotFound => "NOT_FOUND",
            ErrorKind::Conflict => "CONFLICT",
            ErrorKind::Aborted => "ABORTED",
            ErrorKind::PreconditionFailed => "PRECONDITION_FAILED",
            ErrorKind::RateLimited => "RATE_LIMITED",
            ErrorKind::NotImplemented => "NOT_IMPLEMENTED",
            ErrorKind::Unavailable => "UNAVAILABLE",
            ErrorKind::Timeout => "TIMEOUT",
            ErrorKind::Internal => "INTERNAL_SERVER_ERROR",
        }
    }

    /// snake_case name, identical to the serde form.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Validation => "validation",
            ErrorKind::Unauthenticated => "unauthenticated",
            ErrorKind::PermissionDenied => "permission_denied",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Aborted => "aborted",
            ErrorKind::PreconditionFailed => "precondition_failed",
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::NotImplemented => "not_implemented",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Internal => "internal",
        }
    }

    /// Short English title, used as the RFC 9457 `title` and the legacy `error` key fallback.
    pub const fn title(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "Bad request",
            ErrorKind::Validation => "Validation failed",
            ErrorKind::Unauthenticated => "Unauthenticated",
            ErrorKind::PermissionDenied => "Forbidden",
            ErrorKind::NotFound => "Not found",
            ErrorKind::Conflict => "Conflict",
            ErrorKind::Aborted => "Aborted",
            ErrorKind::PreconditionFailed => "Precondition failed",
            ErrorKind::RateLimited => "Too many requests",
            ErrorKind::NotImplemented => "Not implemented",
            ErrorKind::Unavailable => "Service unavailable",
            ErrorKind::Timeout => "Timeout",
            ErrorKind::Internal => "Internal server error",
        }
    }
}

/// Declaration of an error code by a module (or by the framework, see `framework`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorDesc {
    pub code: ErrorCode,
    pub kind: ErrorKind,
    pub message_key: MessageKey,
    /// Names of the parameters the message expects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One violation attached to a field (`path` = `amount`, `lines[2].qty`, `target.name`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldViolation {
    pub path: String,
    pub code: ErrorCode,
    pub message_key: MessageKey,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
}

impl FieldViolation {
    /// Violation whose message key equals its code (the framework convention).
    pub fn new(path: impl Into<String>, code: impl Into<ErrorCode>) -> Self {
        let code = code.into();
        Self {
            path: path.into(),
            message_key: MessageKey(code.0.clone()),
            message: String::new(),
            code,
            params: BTreeMap::new(),
        }
    }
    pub fn with_param(mut self, k: &str, v: impl Serialize) -> Self {
        self.params.insert(
            k.to_string(),
            serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
        );
        self
    }
}

/// The single error envelope, on every protocol, on the server AND in the front.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ContractError {
    pub code: ErrorCode,
    pub kind: ErrorKind,
    pub message_key: MessageKey,
    /// Rendered in the negotiated locale (`engine::i18n::localize`). Before localization it
    /// holds the framework default rendering (French, the historical language of the bodies).
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldViolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl ContractError {
    /// Error whose message key equals its code. Prefer [`ContractError::from_desc`] when an
    /// `ErrorDesc` exists: the kind then comes from the declaration, not from the call site.
    pub fn new(code: impl Into<ErrorCode>, kind: ErrorKind) -> Self {
        let code = code.into();
        Self {
            message_key: MessageKey(code.0.clone()),
            message: code.0.clone(),
            code,
            kind,
            params: BTreeMap::new(),
            fields: Vec::new(),
            trace_id: None,
        }
    }

    /// Error built from its declaration: code, kind and message key cannot contradict it.
    pub fn from_desc(desc: &ErrorDesc) -> Self {
        Self {
            code: desc.code.clone(),
            kind: desc.kind,
            message_key: desc.message_key.clone(),
            message: desc.code.0.clone(),
            params: BTreeMap::new(),
            fields: Vec::new(),
            trace_id: None,
        }
    }

    /// Framework error (`this.*`): the kind is looked up in [`crate::framework`]; an
    /// undeclared code is a programming error and becomes `Internal` (and fails the
    /// `all_emitted_codes_are_declared` test).
    pub fn framework(code: &str) -> Self {
        match crate::framework::kind_of(code) {
            Some(kind) => {
                let mut e = Self::new(code, kind);
                e.message = crate::framework::default_message(code, &e.params);
                e
            }
            None => Self::new(code, ErrorKind::Internal),
        }
    }

    pub fn with_param(mut self, k: &str, v: impl Serialize) -> Self {
        self.params.insert(
            k.to_string(),
            serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
        );
        if crate::framework::kind_of(self.code.as_str()).is_some() {
            self.message = crate::framework::default_message(self.code.as_str(), &self.params);
        }
        self
    }
    pub fn with_field(mut self, v: FieldViolation) -> Self {
        self.fields.push(v);
        self
    }
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }
    pub fn with_trace_id(mut self, id: impl Into<String>) -> Self {
        self.trace_id = Some(id.into());
        self
    }

    /// Placeholder returned by the engine stubs that a stream still has to implement.
    pub fn not_implemented(what: &str) -> Self {
        Self::framework(crate::framework::codes::NOT_IMPLEMENTED).with_param("what", what)
    }

    /// HTTP body, backward compatible AND RFC 9457 compatible (additive):
    /// - legacy keys: `error` (message) and, for validation errors, `errors` (list of messages);
    /// - contract keys: `code`, `kind`, `message_key`, `message`, `params`, `fields`;
    /// - problem+json keys: `type` (`urn:this:error:<code>`), `title`, `status`, `detail`.
    pub fn http_body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        body.insert("error".into(), self.message.clone().into());
        if !self.fields.is_empty() || self.kind == ErrorKind::Validation {
            body.insert(
                "errors".into(),
                self.fields
                    .iter()
                    .map(|f| serde_json::Value::String(f.message.clone()))
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        body.insert("code".into(), self.code.0.clone().into());
        body.insert("kind".into(), self.kind.as_str().into());
        body.insert("message_key".into(), self.message_key.0.clone().into());
        body.insert("message".into(), self.message.clone().into());
        if !self.params.is_empty() {
            body.insert(
                "params".into(),
                serde_json::to_value(&self.params).unwrap_or_default(),
            );
        }
        body.insert(
            "fields".into(),
            serde_json::to_value(&self.fields).unwrap_or_default(),
        );
        if let Some(t) = &self.trace_id {
            body.insert("trace_id".into(), t.clone().into());
        }
        body.insert(
            "type".into(),
            format!("urn:this:error:{}", self.code).into(),
        );
        body.insert("title".into(), self.kind.title().into());
        body.insert("status".into(), self.kind.http_status().into());
        body.insert("detail".into(), self.message.clone().into());
        serde_json::Value::Object(body)
    }

    /// GraphQL `extensions` object: `code` (kind table), `this_code`, `kind`, `message_key`,
    /// `fields`, `params`.
    pub fn graphql_extensions(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.kind.graphql_code(),
            "this_code": self.code.0,
            "kind": self.kind.as_str(),
            "message_key": self.message_key.0,
            "params": self.params,
            "fields": self.fields,
        })
    }

    /// The whole envelope as JSON (gRPC `details`, WebSocket error frames, server functions).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

#[cfg(feature = "http")]
impl ContractError {
    /// `http::StatusCode` of this error.
    pub fn http_status_code(&self) -> http::StatusCode {
        http::StatusCode::from_u16(self.kind.http_status())
            .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// gRPC: `Status { code: kind table, message, details: JSON envelope }`.
#[cfg(feature = "grpc")]
impl From<ContractError> for tonic::Status {
    fn from(e: ContractError) -> Self {
        let details = serde_json::to_vec(&e).unwrap_or_default();
        tonic::Status::with_details(
            tonic::Code::from_i32(e.kind.grpc_code()),
            e.message.clone(),
            details.into(),
        )
    }
}

/// GraphQL: `Error { message, extensions }` with the kind table code.
#[cfg(feature = "graphql")]
impl From<ContractError> for async_graphql::Error {
    fn from(e: ContractError) -> Self {
        let ext = e.graphql_extensions();
        let mut err = async_graphql::Error::new(e.message.clone());
        let mut values = async_graphql::ErrorExtensionValues::default();
        if let serde_json::Value::Object(map) = ext {
            for (k, v) in map {
                if let Ok(v) = async_graphql::Value::from_json(v) {
                    values.set(k, v);
                }
            }
        }
        err.extensions = Some(values);
        err
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kind_mapping_is_total() {
        // Every kind maps to a distinct, documented triple; the table is the RFC §3.2 one.
        let expected: [(ErrorKind, u16, i32, &str); 13] = [
            (ErrorKind::BadRequest, 400, 3, "BAD_REQUEST"),
            (ErrorKind::Validation, 422, 3, "BAD_USER_INPUT"),
            (ErrorKind::Unauthenticated, 401, 16, "UNAUTHENTICATED"),
            (ErrorKind::PermissionDenied, 403, 7, "FORBIDDEN"),
            (ErrorKind::NotFound, 404, 5, "NOT_FOUND"),
            (ErrorKind::Conflict, 409, 6, "CONFLICT"),
            (ErrorKind::Aborted, 409, 10, "ABORTED"),
            (ErrorKind::PreconditionFailed, 412, 9, "PRECONDITION_FAILED"),
            (ErrorKind::RateLimited, 429, 8, "RATE_LIMITED"),
            (ErrorKind::NotImplemented, 501, 12, "NOT_IMPLEMENTED"),
            (ErrorKind::Unavailable, 503, 14, "UNAVAILABLE"),
            (ErrorKind::Timeout, 504, 4, "TIMEOUT"),
            (ErrorKind::Internal, 500, 13, "INTERNAL_SERVER_ERROR"),
        ];
        assert_eq!(expected.len(), ErrorKind::ALL.len());
        for (kind, (k, http, grpc, gql)) in ErrorKind::ALL.iter().zip(expected.iter()) {
            assert_eq!(kind, k);
            assert_eq!(kind.http_status(), *http, "{kind:?}");
            assert_eq!(kind.grpc_code(), *grpc, "{kind:?}");
            assert_eq!(kind.graphql_code(), *gql, "{kind:?}");
            // serde name == as_str
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(kind.as_str().into())
            );
        }
        let gql: std::collections::BTreeSet<_> =
            ErrorKind::ALL.iter().map(|k| k.graphql_code()).collect();
        assert_eq!(
            gql.len(),
            ErrorKind::ALL.len(),
            "GraphQL codes are distinct"
        );
    }

    #[test]
    fn http_body_keeps_legacy_keys_and_adds_problem_json() {
        let e = ContractError::framework("this.validation.failed").with_field(FieldViolation {
            message: "amount doit être positif".into(),
            ..FieldViolation::new("amount", "this.validation.positive")
        });
        let b = e.http_body();
        assert_eq!(b["code"], "this.validation.failed");
        assert_eq!(b["kind"], "validation");
        assert_eq!(b["status"], 422);
        assert_eq!(b["errors"][0], "amount doit être positif");
        assert!(b["error"].is_string());
        assert_eq!(b["type"], "urn:this:error:this.validation.failed");
        assert_eq!(b["fields"][0]["path"], "amount");
    }

    #[test]
    fn contract_error_json_roundtrip() {
        let e = ContractError::framework("this.query.malformed_filter")
            .with_param("reason", "eof")
            .with_trace_id("t1");
        let back: ContractError = serde_json::from_value(e.to_json()).unwrap();
        assert_eq!(back, e);
    }
}
