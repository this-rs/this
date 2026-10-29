//! Locale of a request, on every surface, with ONE priority order (`I18nCatalog::resolve`):
//!
//! 1. explicit choice in the URL: `?lang=en` (or `?locale=en`);
//! 2. persisted choice: cookie `this_locale=en`;
//! 3. `Accept-Language` (HTTP header, gRPC metadata `accept-language`);
//! 4. the catalog's default locale.
//!
//! An explicit choice the catalog does not have falls through to the next hint. gRPC has no
//! URL: its explicit choice is the metadata `x-this-locale`.

use super::framework_catalog::LocaleCatalog;
use crate::descriptor::{I18nCatalog, Locale, LocaleHints};
use axum::extract::FromRequestParts;
use axum::http::{HeaderMap, header, request::Parts};
use std::convert::Infallible;

/// Query parameters read as an explicit locale choice, in order.
pub const LOCALE_QUERY_PARAMS: [&str; 2] = ["lang", "locale"];
/// Cookie holding the persisted locale choice.
pub const LOCALE_COOKIE: &str = "this_locale";
/// gRPC metadata (and HTTP header) for an explicit locale choice.
pub const LOCALE_HEADER: &str = "x-this-locale";

/// Owned locale hints read from a request (borrowed by [`LocaleHints`] for resolution).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestHints {
    pub query: Option<String>,
    pub cookie: Option<String>,
    pub accept_language: Option<String>,
}

impl RequestHints {
    pub fn as_hints(&self) -> LocaleHints<'_> {
        LocaleHints {
            query: self.query.as_deref(),
            cookie: self.cookie.as_deref(),
            accept_language: self.accept_language.as_deref(),
        }
    }

    /// Hints of an HTTP request: query string, `this_locale` cookie, `x-this-locale` header
    /// (treated as an explicit choice, after the query), `Accept-Language`.
    pub fn from_http(headers: &HeaderMap, query: Option<&str>) -> Self {
        let explicit = query
            .and_then(locale_from_query)
            .or_else(|| header_str(headers, LOCALE_HEADER).map(str::to_string));
        Self {
            query: explicit,
            cookie: headers
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .find_map(|c| cookie_value(c, LOCALE_COOKIE)),
            accept_language: header_str(headers, header::ACCEPT_LANGUAGE.as_str())
                .map(str::to_string),
        }
    }

    /// Resolve against a catalog.
    pub fn resolve(&self, catalog: &I18nCatalog) -> Locale {
        catalog.resolve(self.as_hints())
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// First of [`LOCALE_QUERY_PARAMS`] present in a raw query string (percent-decoded).
pub fn locale_from_query(query: &str) -> Option<String> {
    // axum's `Query` (serde_urlencoded underneath, no direct dependency).
    let uri: axum::http::Uri = format!("/?{query}").parse().ok()?;
    let axum::extract::Query(pairs) =
        axum::extract::Query::<Vec<(String, String)>>::try_from_uri(&uri).ok()?;
    LOCALE_QUERY_PARAMS.iter().find_map(|name| {
        pairs
            .iter()
            .find(|(k, v)| k == name && !v.trim().is_empty())
            .map(|(_, v)| v.trim().to_string())
    })
}

/// Value of cookie `name` in a `Cookie` header value.
pub fn cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    cookie_header.split(';').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == name)
            .then(|| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    })
}

/// Locale of an HTTP request (parts), against the catalog installed in its extensions by
/// the `localize_errors` middleware, else the framework catalog.
pub fn locale_of_parts(parts: &Parts) -> Locale {
    let hints = RequestHints::from_http(&parts.headers, parts.uri.query());
    match parts.extensions.get::<LocaleCatalog>() {
        Some(c) => hints.resolve(&c.load()),
        None => hints.resolve(super::framework_catalog()),
    }
}

/// Axum extractor: the negotiated locale of the request (never rejects).
///
/// `RequestLocale` and not `Locale`: `Locale` is a `this-descriptor` type and
/// `FromRequestParts` an axum trait (orphan rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLocale(pub Locale);

impl RequestLocale {
    pub fn locale(&self) -> &Locale {
        &self.0
    }
    pub fn into_inner(self) -> Locale {
        self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestLocale {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(l) = parts.extensions.get::<RequestLocale>() {
            return Ok(l.clone());
        }
        Ok(RequestLocale(locale_of_parts(parts)))
    }
}

/// gRPC: locale from the request metadata (`x-this-locale` explicit, then `cookie`, then
/// `accept-language`), resolved against `catalog`.
#[cfg(feature = "grpc")]
pub fn locale_from_metadata(meta: &tonic::metadata::MetadataMap, catalog: &I18nCatalog) -> Locale {
    let get = |k: &str| {
        meta.get(k)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let hints = RequestHints {
        query: get(LOCALE_HEADER).map(str::to_string),
        cookie: get("cookie").and_then(|c| cookie_value(c, LOCALE_COOKIE)),
        accept_language: get("accept-language").map(str::to_string),
    };
    hints.resolve(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    async fn extract(req: Request<()>) -> Locale {
        let (mut parts, _) = req.into_parts();
        RequestLocale::from_request_parts(&mut parts, &())
            .await
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn locale_extractor_reads_accept_language() {
        let req = |al: &str| {
            Request::builder()
                .uri("/invoices")
                .header(header::ACCEPT_LANGUAGE, al)
                .body(())
                .unwrap()
        };
        assert_eq!(
            extract(req("en-US,en;q=0.9,fr;q=0.5")).await,
            Locale::new("en")
        );
        assert_eq!(extract(req("fr-CA")).await, Locale::new("fr"));
        assert_eq!(
            extract(req("de")).await,
            Locale::new("fr"),
            "default locale"
        );
        let none = Request::builder().uri("/").body(()).unwrap();
        assert_eq!(extract(none).await, Locale::new("fr"));
    }

    #[tokio::test]
    async fn locale_extractor_priority_query_cookie_header() {
        let req = Request::builder()
            .uri("/invoices?page=2&lang=en")
            .header(header::COOKIE, "a=b; this_locale=fr")
            .header(header::ACCEPT_LANGUAGE, "fr")
            .body(())
            .unwrap();
        assert_eq!(extract(req).await, Locale::new("en"), "query wins");
        let req = Request::builder()
            .uri("/invoices")
            .header(header::COOKIE, "a=b; this_locale=en")
            .header(header::ACCEPT_LANGUAGE, "fr")
            .body(())
            .unwrap();
        assert_eq!(extract(req).await, Locale::new("en"), "cookie beats header");
        let req = Request::builder()
            .uri("/invoices?lang=xx")
            .header(header::ACCEPT_LANGUAGE, "en")
            .body(())
            .unwrap();
        assert_eq!(
            extract(req).await,
            Locale::new("en"),
            "unknown explicit falls through"
        );
        // The catalog installed by the middleware decides what is available.
        let cat =
            LocaleCatalog::new(I18nCatalog::framework().with_default_locale(Locale::new("en")));
        let mut req = Request::builder().uri("/").body(()).unwrap();
        req.extensions_mut().insert(cat);
        assert_eq!(extract(req).await, Locale::new("en"));
    }

    #[test]
    fn query_and_cookie_parsing() {
        assert_eq!(
            locale_from_query("locale=en-GB&x=1").as_deref(),
            Some("en-GB")
        );
        assert_eq!(locale_from_query("lang=%20fr%20").as_deref(), Some("fr"));
        assert_eq!(locale_from_query("lang=&locale=en").as_deref(), Some("en"));
        assert_eq!(locale_from_query("x=1"), None);
        assert_eq!(
            cookie_value("this_locale=\"en\"", LOCALE_COOKIE).as_deref(),
            Some("en")
        );
        assert_eq!(cookie_value("xthis_locale=en", LOCALE_COOKIE), None);
    }

    #[cfg(feature = "grpc")]
    #[test]
    fn grpc_locale_from_metadata() {
        let cat = I18nCatalog::framework();
        let mut m = tonic::metadata::MetadataMap::new();
        assert_eq!(locale_from_metadata(&m, &cat), Locale::new("fr"));
        m.insert("accept-language", "en-US;q=0.9, fr;q=0.1".parse().unwrap());
        assert_eq!(locale_from_metadata(&m, &cat), Locale::new("en"));
        m.insert("cookie", "this_locale=fr".parse().unwrap());
        assert_eq!(locale_from_metadata(&m, &cat), Locale::new("fr"));
        m.insert(LOCALE_HEADER, "en".parse().unwrap());
        assert_eq!(locale_from_metadata(&m, &cat), Locale::new("en"));
    }
}
