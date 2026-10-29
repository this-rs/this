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
use crate::descriptor::framework::codes;
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

/// Legacy keys a surface keeps ADDING to the contract body (`error`, `errors`, `scope`,
/// `policy`...): carried in the response extensions so that [`localize_errors`] re-renders
/// the localized contract body without losing them.
#[derive(Debug, Clone, Default)]
pub struct ErrorExtras(pub serde_json::Map<String, serde_json::Value>);

/// Contract response (status from `err`, or `status` when a legacy surface keeps its own)
/// with legacy `extras` merged over the body. The contract keys stay.
pub fn error_response_with(
    err: ContractError,
    status: Option<axum::http::StatusCode>,
    extras: serde_json::Map<String, serde_json::Value>,
) -> Response {
    let mut res = render(err, None);
    if let Some(s) = status {
        *res.status_mut() = s;
    }
    let body = merged_body(
        res.extensions().get::<ContractError>(),
        res.status(),
        &extras,
    );
    if let Some(body) = body {
        *res.body_mut() = axum::body::Body::from(serde_json::to_vec(&body).unwrap_or_default());
    }
    res.extensions_mut().insert(ErrorExtras(extras));
    res
}

fn merged_body(
    err: Option<&ContractError>,
    status: axum::http::StatusCode,
    extras: &serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    let mut body = err?.http_body();
    if let Some(o) = body.as_object_mut() {
        o.insert("status".into(), status.as_u16().into());
        for (k, v) in extras {
            o.insert(k.clone(), v.clone());
        }
    }
    Some(body)
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
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    if res.extensions().get::<ContractError>().is_none() {
        res = envelope_bare(res, &method, &path).await;
    }
    let Some(err) = res.extensions().get::<ContractError>().cloned() else {
        return res;
    };
    relocalize(res, snapshot.localize(err, &locale), &locale, problem)
}

/// A 4xx response that carries no contract (axum rejections: `JsonRejection`, `Path`,
/// `Query`...; the router's 404 / 405; a handler returning a bare `StatusCode`) gets the
/// envelope by its status, which it KEEPS. Bodies that are not plain text (HTML of a
/// frontend, JSON of a custom handler) are left alone.
async fn envelope_bare(res: Response, method: &axum::http::Method, path: &str) -> Response {
    use axum::http::StatusCode as S;
    let status = res.status();
    if !status.is_client_error() || status == S::TOO_MANY_REQUESTS {
        return res;
    }
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if let Some(c) = &ctype
        && !c.starts_with("text/plain")
    {
        return res;
    }
    let (parts, body) = res.into_parts();
    let text = axum::body::to_bytes(body, 8 * 1024)
        .await
        .ok()
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .unwrap_or_default();
    let reason = if text.trim().is_empty() {
        status.canonical_reason().unwrap_or("error").to_string()
    } else {
        text.trim().to_string()
    };
    let err = match status {
        S::NOT_FOUND => {
            ContractError::framework(codes::TRANSPORT_NOT_FOUND).with_param("path", path)
        }
        S::METHOD_NOT_ALLOWED => ContractError::framework(codes::TRANSPORT_METHOD_NOT_ALLOWED)
            .with_param("method", method.as_str()),
        S::UNAUTHORIZED => ContractError::framework(codes::AUTH_UNAUTHENTICATED),
        S::FORBIDDEN => super::forbidden(format!("{method} {path}")),
        _ => super::bad_request(reason),
    };
    let mut out = render(err.clone(), None);
    *out.status_mut() = status;
    // Keep what the inner layer set (`WWW-Authenticate`, `Allow`...), except the body headers.
    for (k, v) in parts.headers.iter() {
        if k != header::CONTENT_TYPE && k != header::CONTENT_LENGTH {
            out.headers_mut().append(k.clone(), v.clone());
        }
    }
    out
}

/// Replace the body of an error response by `err` (already localized), IN PLACE: status,
/// every other header (all values of a multi-valued one such as `set-cookie`) and every
/// extension set by the handler or inner layers are kept. A stale `content-length` is
/// dropped; `vary` gains the request parts the body now depends on.
fn relocalize(res: Response, err: ContractError, locale: &Locale, problem: bool) -> Response {
    let (mut parts, _old_body) = res.into_parts();
    let extras = parts
        .extensions
        .get::<ErrorExtras>()
        .map(|e| e.0.clone())
        .unwrap_or_default();
    let body = merged_body(Some(&err), parts.status, &extras)
        .map(|b| serde_json::to_vec(&b).unwrap_or_default())
        .unwrap_or_default();
    let h = &mut parts.headers;
    h.remove(header::CONTENT_LENGTH);
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if problem {
            PROBLEM_JSON
        } else {
            "application/json"
        }),
    );
    if let Ok(v) = HeaderValue::from_str(locale.as_str()) {
        h.insert(header::CONTENT_LANGUAGE, v);
    }
    if let Ok(v) = HeaderValue::from_str(err.code.as_str()) {
        h.insert(ERROR_HEADER, v);
    }
    h.append(header::VARY, HeaderValue::from_static(LOCALE_VARY));
    parts.extensions.insert(err);
    Response::from_parts(parts, axum::body::Body::from(body))
}

/// Request parts a localized error body depends on (`?lang=` is in the URL already).
const LOCALE_VARY: &str = "accept, accept-language, cookie, x-this-locale";

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

    /// Responses that carry no contract (axum's own 404 / 405, extractor rejections as plain
    /// text, a bare `StatusCode`) get the envelope of their status, which they KEEP, in the
    /// request locale.
    #[tokio::test]
    async fn bare_4xx_responses_get_the_envelope_in_the_request_locale() {
        use axum::routing::post;
        let app = Router::new()
            .route("/x", get(|| async { "ok" }))
            .route("/bare", get(|| async { StatusCode::NOT_FOUND }))
            .route(
                "/json",
                post(|axum::Json(_v): axum::Json<serde_json::Value>| async { "ok" }),
            )
            .layer(axum::middleware::from_fn_with_state(
                LocaleCatalog::framework(),
                localize_errors,
            ));
        let call = |method: &str, uri: &str, lang: &str| {
            let app = app.clone();
            let req = HttpRequest::builder()
                .method(method)
                .uri(uri)
                .header(header::ACCEPT_LANGUAGE, lang)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{nope"))
                .unwrap();
            async move { app.oneshot(req).await.unwrap() }
        };
        for (method, uri, status, code) in [
            ("GET", "/missing", 404u16, "this.transport.not_found"),
            ("GET", "/bare", 404, "this.transport.not_found"),
            ("DELETE", "/x", 405, "this.transport.method_not_allowed"),
            ("POST", "/json", 400, "this.transport.bad_request"),
        ] {
            let fr = call(method, uri, "fr").await;
            assert_eq!(fr.status().as_u16(), status, "{method} {uri}");
            assert_eq!(fr.headers()[ERROR_HEADER], code, "{method} {uri}");
            assert_eq!(fr.headers()[header::CONTENT_LANGUAGE], "fr");
            let fr = body_json(fr).await;
            let en = body_json(call(method, uri, "en").await).await;
            assert_eq!(fr["code"], code);
            assert_eq!(fr["status"], status);
            assert_ne!(fr["message"], en["message"], "{method} {uri}: fr vs en");
        }
        // Not a plain-text rejection: a JSON body of a handler is left alone.
        let app = Router::new()
            .route(
                "/j",
                get(|| async {
                    (
                        StatusCode::NOT_FOUND,
                        axum::Json(serde_json::json!({"mine": 1})),
                    )
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                LocaleCatalog::framework(),
                localize_errors,
            ));
        let res = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/j")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(res).await, serde_json::json!({"mine": 1}));
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

    #[derive(Clone, Debug, PartialEq)]
    struct Marker(u8);

    #[tokio::test]
    async fn localize_errors_keeps_multi_valued_headers_and_extensions() {
        let app = Router::new()
            .route(
                "/logout",
                get(|| async {
                    let mut res = ApiError(not_found("session", "s1")).into_response();
                    let h = res.headers_mut();
                    h.append(
                        header::SET_COOKIE,
                        HeaderValue::from_static("a=; Max-Age=0"),
                    );
                    h.append(
                        header::SET_COOKIE,
                        HeaderValue::from_static("b=; Max-Age=0"),
                    );
                    // Stale length of the French body: must not survive the rewrite.
                    h.insert(header::CONTENT_LENGTH, HeaderValue::from_static("3"));
                    res.extensions_mut().insert(Marker(7));
                    res
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                LocaleCatalog::framework(),
                localize_errors,
            ));
        let req = HttpRequest::builder()
            .uri("/logout")
            .header(header::ACCEPT_LANGUAGE, "en")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let cookies: Vec<_> = res.headers().get_all(header::SET_COOKIE).iter().collect();
        assert_eq!(cookies.len(), 2, "every set-cookie kept: {cookies:?}");
        // The stale "3" is gone; if a length is present (axum sets it from the new body's
        // exact size), it is the real one.
        let length = res
            .headers()
            .get(header::CONTENT_LENGTH)
            .map(|v| v.to_str().unwrap().parse::<usize>().unwrap());
        assert_ne!(length, Some(3), "stale content-length kept");
        assert_eq!(res.extensions().get::<Marker>(), Some(&Marker(7)));
        assert_eq!(
            res.extensions().get::<ContractError>().unwrap().message,
            "session not found: s1",
            "extension holds the localized error"
        );
        let vary: Vec<_> = res.headers().get_all(header::VARY).iter().collect();
        assert!(
            vary.iter()
                .any(|v| v.to_str().unwrap().contains("accept-language")),
            "{vary:?}"
        );
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        if let Some(n) = length {
            assert_eq!(n, bytes.len(), "content-length matches the localized body");
        }
        let b: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(b["message"], "session not found: s1");
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
