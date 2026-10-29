//! REST: `ContractError` as an HTTP response, localized in the request locale.
//!
//! - [`ApiError`] (`IntoResponse`): status from the kind table, body
//!   `ContractError::http_body` (legacy `error` / `errors` keys kept, contract keys,
//!   RFC 9457 keys), header `x-this-error: <code>`. The error itself is also stored in the
//!   response extensions.
//! - [`localize_errors`] middleware: installs the [`LocaleCatalog`] for the extractors,
//!   resolves the request locale (query, cookie, `Accept-Language`), and re-renders every
//!   `ContractError` response in that locale, as `application/problem+json` when the client
//!   accepts it (`Content-Language` set). Handlers never deal with locales.
//!
//! Without the middleware, the body carries the framework default rendering (French).

use super::{ERROR_HEADER, PROBLEM_JSON, from_anyhow, http_status};
use crate::core::i18n::{LocaleCatalog, RequestHints, framework_catalog};
use crate::descriptor::{ContractError, I18nCatalog, Locale};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// REST response: newtype because of the orphan rule (`IntoResponse` and `ContractError`
/// are both foreign to this-rs).
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError(pub ContractError);

/// Handler result whose error side is a contract error. `?` works on
/// `Result<_, ContractError>` and on `Result<_, anyhow::Error>` (through [`from_anyhow`]).
pub type ApiResult<T> = Result<T, ApiError>;

impl From<ContractError> for ApiError {
    fn from(e: ContractError) -> Self {
        Self(e)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(from_anyhow(&e))
    }
}

impl ApiError {
    pub fn into_inner(self) -> ContractError {
        self.0
    }

    /// Response in an explicit locale (when the middleware is not installed).
    pub fn into_response_in(
        self,
        catalog: &I18nCatalog,
        locale: &Locale,
        problem: bool,
    ) -> Response {
        error_response(self.0, catalog, locale, problem)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        render(self.0, None)
    }
}

/// `true` when the `Accept` header asks for `application/problem+json` (RFC 9457).
pub fn accepts_problem_json(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|part| {
            let mut it = part.split(';');
            let media = it.next().unwrap_or("").trim();
            let refused = it.any(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .and_then(|q| q.trim().parse::<f32>().ok())
                    == Some(0.0)
            });
            media.eq_ignore_ascii_case(PROBLEM_JSON) && !refused
        })
}

/// Error response localized in `locale` (`problem`: serve `application/problem+json`).
pub fn error_response(
    err: ContractError,
    catalog: &I18nCatalog,
    locale: &Locale,
    problem: bool,
) -> Response {
    let localized = catalog.localize(err, locale);
    let mut res = render(localized, Some(locale));
    if problem {
        res.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
    }
    res
}

fn render(err: ContractError, locale: Option<&Locale>) -> Response {
    let status = http_status(&err);
    let mut res = (status, axum::Json(err.http_body())).into_response();
    let headers = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(err.code.as_str()) {
        headers.insert(ERROR_HEADER, v);
    }
    if let Some(l) = locale
        && let Ok(v) = HeaderValue::from_str(l.as_str())
    {
        headers.insert(header::CONTENT_LANGUAGE, v);
    }
    res.extensions_mut().insert(err);
    res
}

/// Middleware: localize every `ContractError` response in the request locale.
///
/// ```ignore
/// let catalog = LocaleCatalog::framework(); // + module bundles
/// router.layer(axum::middleware::from_fn_with_state(catalog, localize_errors))
/// ```
pub async fn localize_errors(
    State(catalog): State<LocaleCatalog>,
    mut req: Request,
    next: Next,
) -> Response {
    let hints = RequestHints::from_http(req.headers(), req.uri().query());
    let problem = accepts_problem_json(req.headers());
    let snapshot = catalog.load();
    let locale = hints.resolve(&snapshot);
    req.extensions_mut().insert(catalog.clone());
    req.extensions_mut()
        .insert(crate::core::i18n::RequestLocale(locale.clone()));
    let res = next.run(req).await;
    match res.extensions().get::<ContractError>() {
        Some(err) => {
            let err = err.clone();
            let mut out = error_response(err, &snapshot, &locale, problem);
            // Keep headers set by outer handlers (CORS, tracing...) that the body rewrite
            // would otherwise drop; ours (content-type, language, error code) win.
            for (name, value) in res.headers() {
                if !out.headers().contains_key(name) {
                    out.headers_mut().insert(name.clone(), value.clone());
                }
            }
            out
        }
        None => res,
    }
}

/// Response for an error in the framework catalog's default locale (French), for code that
/// has no request at hand.
pub fn default_error_response(err: ContractError) -> Response {
    let cat = framework_catalog();
    error_response(err, cat, cat.default_locale(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::errors::{codes, not_found, validation};
    use crate::descriptor::FieldViolation;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    async fn body_json(res: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn invalid_amount() -> ContractError {
        validation([FieldViolation::new("amount", codes::VALIDATION_POSITIVE)
            .with_param("field", "amount")
            .with_param("value", -1)])
    }

    #[test]
    fn api_error_status_body_and_header() {
        let res = ApiError(ContractError::framework("this.query.malformed_filter")).into_response();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(res.headers()[ERROR_HEADER], "this.query.malformed_filter");
    }

    #[tokio::test]
    async fn api_error_http_body_shape() {
        let res = ApiError(invalid_amount()).into_response();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(res.headers()[ERROR_HEADER], "this.validation.failed");
        assert_eq!(res.headers()[header::CONTENT_TYPE], "application/json");
        assert!(res.extensions().get::<ContractError>().is_some());
        let b = body_json(res).await;
        // Legacy keys (Validated<T> clients).
        assert_eq!(b["error"], "Validation échouée");
        assert!(b["errors"].is_array());
        // Contract keys.
        assert_eq!(b["code"], "this.validation.failed");
        assert_eq!(b["kind"], "validation");
        assert_eq!(b["message_key"], "this.validation.failed");
        assert_eq!(b["message"], "Validation échouée");
        assert_eq!(b["fields"][0]["path"], "amount");
        assert_eq!(b["fields"][0]["code"], "this.validation.positive");
        assert_eq!(b["fields"][0]["params"]["value"], -1);
        // RFC 9457 keys.
        assert_eq!(b["type"], "urn:this:error:this.validation.failed");
        assert_eq!(b["title"], "Validation failed");
        assert_eq!(b["status"], 422);
        assert_eq!(b["detail"], "Validation échouée");
        // `?` from anyhow keeps the contract, hides the rest.
        let e: ApiError = anyhow::Error::new(not_found("invoice", 3)).into();
        assert_eq!(e.0.code.as_str(), codes::ENTITY_NOT_FOUND);
        let e: ApiError = anyhow::anyhow!("secret").into();
        assert_eq!(e.0.code.as_str(), codes::INTERNAL);
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/invoices",
                get(|| async { Err::<(), _>(ApiError(invalid_amount())) }),
            )
            .route(
                "/invoices/{id}",
                get(|| async { Err::<(), ApiError>(not_found("invoice", 42).into()) }),
            )
            .route(
                "/locale",
                get(|l: crate::core::i18n::RequestLocale| async move { l.0.to_string() }),
            )
            .route("/ok", get(|| async { "fine" }))
            .layer(axum::middleware::from_fn_with_state(
                LocaleCatalog::framework(),
                localize_errors,
            ))
    }

    #[tokio::test]
    async fn localized_body_in_requested_locale() {
        let req = |uri: &str, al: &str| {
            HttpRequest::builder()
                .uri(uri)
                .header(header::ACCEPT_LANGUAGE, al)
                .body(Body::empty())
                .unwrap()
        };
        let res = app()
            .oneshot(req("/invoices", "en-US,en;q=0.9"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(res.headers()[header::CONTENT_LANGUAGE], "en");
        let b = body_json(res).await;
        assert_eq!(b["message"], "Validation failed");
        assert_eq!(b["error"], "Validation failed");
        assert_eq!(
            b["fields"][0]["message"],
            "Field 'amount' must be positive (value: -1)"
        );
        assert_eq!(
            b["errors"][0],
            "Field 'amount' must be positive (value: -1)"
        );

        let b = body_json(app().oneshot(req("/invoices", "fr")).await.unwrap()).await;
        assert_eq!(
            b["fields"][0]["message"],
            "Le champ 'amount' doit être positif (valeur: -1)"
        );

        // Explicit choice in the URL beats the header.
        let b = body_json(
            app()
                .oneshot(req("/invoices/42?lang=en", "fr"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(b["message"], "invoice not found: 42");
        assert_eq!(b["status"], 404);

        // The extractor sees the same locale as the middleware.
        let res = app().oneshot(req("/locale", "en")).await.unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 64).await.unwrap();
        assert_eq!(&bytes[..], b"en");

        // Non-error responses are untouched.
        let res = app().oneshot(req("/ok", "en")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get(header::CONTENT_LANGUAGE).is_none());
    }

    #[tokio::test]
    async fn problem_json_when_accepted() {
        let req = HttpRequest::builder()
            .uri("/invoices/1")
            .header(
                header::ACCEPT,
                "application/problem+json, application/json;q=0.5",
            )
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(res.headers()[header::CONTENT_TYPE], PROBLEM_JSON);
        assert_eq!(res.headers()[ERROR_HEADER], "this.entity.not_found");
        let mut h = HeaderMap::new();
        h.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/problem+json;q=0"),
        );
        assert!(!accepts_problem_json(&h));
        h.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
        assert!(!accepts_problem_json(&h), "plain JSON stays the default");
    }
}
