//! GraphQL: `ContractError` as `async_graphql::Error`.
//!
//! `extensions.code` comes from the kind table (`BAD_USER_INPUT`, `FORBIDDEN`,
//! `NOT_FOUND`...); `extensions` also carries `this_code`, `kind`, `message_key`, `params`
//! and `fields` (with their localized messages).
//!
//! Never `?` a `ContractError` in a resolver: async-graphql's blanket
//! `impl<T: Display> From<T> for Error` keeps the message only. Use
//! [`GraphqlResultExt::map_gql`] / [`GraphqlResultExt::map_gql_in`] instead.
//!
//! Locale: the HTTP handler puts the [`RequestLocale`] (and the `LocaleCatalog`) in the
//! request data (`request.data(locale)`); [`to_graphql_error_in`] reads them from the
//! resolver context.

use super::from_anyhow;
use crate::core::i18n::{LocaleCatalog, RequestLocale, framework_catalog};
use crate::descriptor::{ContractError, I18nCatalog, Locale};

/// GraphQL error (`extensions.code` from the kind table, plus `this_code`, `fields`...).
pub fn to_graphql_error(err: ContractError) -> async_graphql::Error {
    err.into_graphql_error()
}

/// Same, rendered in `locale`.
pub fn to_graphql_error_localized(
    err: ContractError,
    catalog: &I18nCatalog,
    locale: &Locale,
) -> async_graphql::Error {
    to_graphql_error(catalog.localize(err, locale))
}

/// Same, in the locale found in the resolver context (default locale otherwise).
pub fn to_graphql_error_in(
    ctx: &async_graphql::Context<'_>,
    err: ContractError,
) -> async_graphql::Error {
    let shared = ctx.data_opt::<LocaleCatalog>().map(LocaleCatalog::load);
    let catalog: &I18nCatalog = shared.as_deref().unwrap_or_else(|| framework_catalog());
    let locale = ctx
        .data_opt::<RequestLocale>()
        .map(|l| l.0.clone())
        .unwrap_or_else(|| catalog.default_locale().clone());
    to_graphql_error_localized(err, catalog, &locale)
}

/// GraphQL error of any `anyhow::Error` (contract kept, anything else `this.internal`).
pub fn graphql_error_from_anyhow(err: &anyhow::Error) -> async_graphql::Error {
    to_graphql_error(from_anyhow(err))
}

/// `Result` → `async_graphql::Result` with the contract table.
pub trait GraphqlResultExt<T> {
    /// Framework default message (French).
    fn map_gql(self) -> async_graphql::Result<T>;
    /// Message in the locale of the request (resolver context).
    fn map_gql_in(self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<T>;
}

impl<T> GraphqlResultExt<T> for Result<T, anyhow::Error> {
    fn map_gql(self) -> async_graphql::Result<T> {
        self.map_err(|e| graphql_error_from_anyhow(&e))
    }
    fn map_gql_in(self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<T> {
        self.map_err(|e| to_graphql_error_in(ctx, from_anyhow(&e)))
    }
}

impl<T> GraphqlResultExt<T> for Result<T, ContractError> {
    fn map_gql(self) -> async_graphql::Result<T> {
        self.map_err(to_graphql_error)
    }
    fn map_gql_in(self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<T> {
        self.map_err(|e| to_graphql_error_in(ctx, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::errors::{codes, forbidden, not_found, validation};
    use crate::descriptor::FieldViolation;
    use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema, Value};

    fn ext(e: &async_graphql::Error, k: &str) -> Option<Value> {
        e.extensions.as_ref().and_then(|x| x.get(k).cloned())
    }

    #[test]
    fn contract_error_to_graphql_extensions() {
        let e = to_graphql_error(ContractError::framework("this.validation.failed"));
        assert_eq!(ext(&e, "code"), Some(Value::from("BAD_USER_INPUT")));
        assert_eq!(
            ext(&e, "this_code"),
            Some(Value::from("this.validation.failed"))
        );

        let invalid = validation([FieldViolation::new("amount", codes::VALIDATION_POSITIVE)
            .with_param("field", "amount")
            .with_param("value", -1)]);
        let e = to_graphql_error_localized(invalid, &I18nCatalog::framework(), &Locale::new("en"));
        assert_eq!(e.message, "Validation failed");
        assert_eq!(ext(&e, "kind"), Some(Value::from("validation")));
        assert_eq!(
            ext(&e, "message_key"),
            Some(Value::from("this.validation.failed"))
        );
        let fields = ext(&e, "fields").unwrap().into_json().unwrap();
        assert_eq!(fields[0]["path"], "amount");
        assert_eq!(
            fields[0]["message"],
            "Field 'amount' must be positive (value: -1)"
        );

        for (err, code) in [
            (not_found("invoice", 1), "NOT_FOUND"),
            (forbidden("create"), "FORBIDDEN"),
            (
                ContractError::framework(codes::INTERNAL),
                "INTERNAL_SERVER_ERROR",
            ),
        ] {
            assert_eq!(ext(&to_graphql_error(err), "code"), Some(Value::from(code)));
        }
        let r: Result<(), anyhow::Error> = Err(anyhow::anyhow!("token=secret"));
        let e = r.map_gql().unwrap_err();
        assert_eq!(ext(&e, "this_code"), Some(Value::from("this.internal")));
        assert!(!e.message.contains("secret"));
    }

    struct Query;

    #[Object]
    impl Query {
        async fn invoice(
            &self,
            ctx: &async_graphql::Context<'_>,
            id: i32,
        ) -> async_graphql::Result<i32> {
            Err::<i32, ContractError>(not_found("invoice", id)).map_gql_in(ctx)
        }
    }

    #[tokio::test]
    async fn graphql_error_localized_from_request_data() {
        let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
        let res = schema
            .execute(
                async_graphql::Request::new("{ invoice(id: 5) }")
                    .data(RequestLocale(Locale::new("en")))
                    .data(LocaleCatalog::framework()),
            )
            .await;
        let err = &res.errors[0];
        assert_eq!(err.message, "invoice not found: 5");
        let code = err.extensions.as_ref().and_then(|x| x.get("code").cloned());
        assert_eq!(code, Some(Value::from("NOT_FOUND")));
        // Without request data: default locale (fr).
        let res = schema.execute("{ invoice(id: 5) }").await;
        assert_eq!(res.errors[0].message, "invoice introuvable: 5");
    }
}
