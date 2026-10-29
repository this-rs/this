//! Locales on the server side. The engine (negotiation, rendering, `I18nCatalog`) is pure
//! and lives in `this_descriptor::engine::i18n` (shared with wasm32 fronts); this module adds
//! the host catalog (`LocaleCatalog`) and the extractors (HTTP query / cookie /
//! `Accept-Language`, gRPC metadata).

pub mod extract;
pub mod framework_catalog;

pub use crate::descriptor::engine::i18n::{localize, negotiate, render};
pub use crate::descriptor::{I18nCatalog, Locale, LocaleHints, MessageBundle, MessageKey};
#[cfg(feature = "grpc")]
pub use extract::locale_from_metadata;
pub use extract::{
    LOCALE_COOKIE, LOCALE_HEADER, LOCALE_QUERY_PARAMS, RequestHints, RequestLocale, locale_of_parts,
};
pub use framework_catalog::{LocaleCatalog, framework_bundles, framework_catalog};
