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

pub use self::http::{
    ApiError, ApiResult, ErrorExtras, accepts_problem_json, error_response, error_response_with,
    localize_errors,
};
pub use crate::descriptor::error::{ERROR_HEADER, PROBLEM_JSON};
pub use crate::descriptor::framework::codes;
pub use crate::descriptor::{ContractError, ErrorCode, ErrorKind, FieldViolation};

#[cfg(feature = "graphql")]
pub use self::graphql::{GraphqlResultExt, to_graphql_error, to_graphql_error_localized};
#[cfg(feature = "grpc")]
pub use self::grpc::{StatusResultExt, status_for, to_status, to_status_localized};

/// Recover a `ContractError` from an `anyhow::Error` (searching the whole cause chain);
/// anything else becomes `this.internal` (the original text is NOT exposed to clients, it
/// is logged at `error`: a 500 is a server bug).
pub fn from_anyhow(err: &anyhow::Error) -> ContractError {
    for cause in err.chain() {
        if let Some(ApiError(c)) = cause.downcast_ref::<ApiError>() {
            return c.clone();
        }
        if let Some(c) = cause.downcast_ref::<ContractError>() {
            return c.clone();
        }
    }
    tracing::error!(error = %err, "error without contract: reported as this.internal");
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
        // An `ApiError` carried by anyhow (REST helper reused below) keeps its contract too.
        let api = anyhow::Error::new(ApiError(forbidden("delete"))).context("handler");
        assert_eq!(from_anyhow(&api).code.as_str(), codes::AUTH_FORBIDDEN);
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

/// The same error, raised in the same layer (inside `anyhow`, as an `EntityCreator` would),
/// reaches REST, gRPC and GraphQL with the same stable code, the mapped status of each
/// protocol and the same text in the locale of the request; a module code declared in a
/// descriptor (`ErrorDesc`) and translated by the module's own bundles behaves exactly like
/// a framework code.
#[cfg(all(test, feature = "grpc", feature = "graphql"))]
mod cross_protocol_tests {
    use super::*;
    use crate::core::i18n::{LOCALE_HEADER, LocaleCatalog, RequestLocale};
    use crate::descriptor::{ErrorDesc, Locale, MessageBundle, MessageKey};
    use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
    use axum::body::Body;
    use axum::http::{Request, header};
    use axum::{Router, routing::get};
    use tonic::metadata::MetadataMap;
    use tower::ServiceExt;

    const PAID: &str = "billing.invoice.already_paid";

    fn catalog() -> LocaleCatalog {
        let bundle = |l: &str, t: &str| MessageBundle {
            module: "billing".into(),
            locale: Locale::new(l),
            messages: [(MessageKey::new(PAID), t.to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let c = LocaleCatalog::framework();
        c.add_bundles([
            bundle("fr", "Facture {number} déjà payée"),
            bundle("en", "Invoice {number} already paid"),
        ])
        .unwrap();
        c
    }

    fn errors() -> Vec<(ContractError, u16, tonic::Code, &'static str)> {
        let desc = ErrorDesc {
            code: PAID.into(),
            kind: ErrorKind::Conflict,
            message_key: MessageKey::new(PAID),
            params: vec!["number".into()],
            description: None,
        };
        vec![
            (
                ContractError::from_desc(&desc).with_param("number", "F-1"),
                409,
                tonic::Code::AlreadyExists,
                "CONFLICT",
            ),
            (
                validation([FieldViolation::new("amount", codes::VALIDATION_POSITIVE)
                    .with_param("field", "amount")
                    .with_param("value", -1)]),
                422,
                tonic::Code::InvalidArgument,
                "BAD_USER_INPUT",
            ),
            (
                forbidden("create"),
                403,
                tonic::Code::PermissionDenied,
                "FORBIDDEN",
            ),
            (
                not_found("invoice", 9),
                404,
                tonic::Code::NotFound,
                "NOT_FOUND",
            ),
        ]
    }

    /// The layer below every surface: a contract error inside `anyhow`, behind contexts.
    fn service_layer(err: &ContractError) -> anyhow::Result<i32> {
        Err(anyhow::Error::new(err.clone())
            .context("repository")
            .context("service"))
    }

    struct Query;

    #[Object]
    impl Query {
        async fn invoice(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<i32> {
            service_layer(ctx.data_unchecked::<ContractError>()).map_gql_in(ctx)
        }
    }

    async fn rest(
        err: &ContractError,
        cat: &LocaleCatalog,
        lang: &str,
    ) -> (u16, String, serde_json::Value) {
        let e = err.clone();
        let app = Router::new()
            .route(
                "/invoices",
                get(move || {
                    let e = e.clone();
                    async move {
                        service_layer(&e)
                            .map(|n| n.to_string())
                            .map_err(ApiError::from)
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                cat.clone(),
                localize_errors,
            ));
        let req = Request::builder()
            .uri(format!("/invoices?lang={lang}"))
            .header(header::ACCEPT_LANGUAGE, "de")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        let status = res.status().as_u16();
        let code = res.headers()[ERROR_HEADER].to_str().unwrap().to_string();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, code, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn same_error_same_code_and_text_on_every_surface() {
        let cat = catalog();
        let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
        for (err, http, grpc, gql) in errors() {
            let code = err.code.as_str().to_string();
            for lang in ["fr", "en"] {
                let expected = cat.load().localize(err.clone(), &Locale::new(lang));
                assert!(
                    !expected.message.contains('{') && expected.message != code,
                    "{code}/{lang}: real text expected, got {}",
                    expected.message
                );

                // REST: `?lang=` beats Accept-Language (de, unavailable anyway).
                let (status, header_code, body) = rest(&err, &cat, lang).await;
                assert_eq!(status, http, "{code}/{lang} REST status");
                assert_eq!(header_code, code);
                assert_eq!(body["code"], code.as_str());
                assert_eq!(body["message"], expected.message.as_str(), "{code}/{lang}");
                for (i, f) in expected.fields.iter().enumerate() {
                    assert_eq!(body["fields"][i]["message"], f.message.as_str());
                }

                // gRPC: explicit choice in metadata.
                let mut meta = MetadataMap::new();
                meta.insert(LOCALE_HEADER, lang.parse().unwrap());
                meta.insert("accept-language", "de".parse().unwrap());
                let s = service_layer(&err)
                    .map_status_for(&meta, &cat.load())
                    .unwrap_err();
                assert_eq!(s.code(), grpc, "{code}/{lang} gRPC code");
                assert_eq!(s.metadata().get(ERROR_HEADER).unwrap(), code.as_str());
                assert_eq!(s.message(), expected.message, "{code}/{lang}");
                let back = grpc::contract_error_of(&s).unwrap();
                assert_eq!(
                    back, expected,
                    "{code}/{lang}: gRPC details = REST envelope"
                );

                // GraphQL: locale and catalog in the request data.
                let res = schema
                    .execute(
                        async_graphql::Request::new("{ invoice }")
                            .data(err.clone())
                            .data(RequestLocale(Locale::new(lang)))
                            .data(cat.clone()),
                    )
                    .await;
                let e = &res.errors[0];
                assert_eq!(e.message, expected.message, "{code}/{lang} GraphQL");
                let ext = e.extensions.as_ref().unwrap();
                assert_eq!(ext.get("code"), Some(&async_graphql::Value::from(gql)));
                assert_eq!(
                    ext.get("this_code"),
                    Some(&async_graphql::Value::from(code.as_str()))
                );
            }
        }
    }
}
