//! Locales on the server side. The engine (negotiation, rendering) is pure and lives in
//! `this_descriptor::engine::i18n`; the `errors-i18n` stream adds here the extractors
//! (HTTP `Accept-Language`, gRPC metadata `accept-language`).

pub use crate::descriptor::engine::i18n::{localize, negotiate, render};
pub use crate::descriptor::{Locale, MessageBundle, MessageKey};

/// Framework bundles shipped by this-rs (`fr` default, `en`).
pub fn framework_bundles() -> Vec<MessageBundle> {
    ["fr", "en"]
        .iter()
        .filter_map(|l| crate::descriptor::framework::bundle(&Locale::new(*l)))
        .collect()
}
