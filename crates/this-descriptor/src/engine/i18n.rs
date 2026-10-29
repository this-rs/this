//! Locale negotiation and message rendering (`{param}` substitution).
//!
//! Pure (wasm32): the same functions run on the server (error bodies, gRPC `Status`,
//! GraphQL `extensions`) and in a Leptos front (rendering a `ContractError` received from a
//! server function, or produced by a client-side `engine::validate::apply`).
//!
//! Rules, identical on every side:
//! - a key is looked up in the requested locale (exact tag, then primary language), then in
//!   the catalog's default locale, and only then rendered as **the key itself** (never a
//!   panic, never an empty string);
//! - module bundles are searched before the framework's, so a module may reword a `this.*`
//!   message for its own users;
//! - an explicit choice (query parameter, cookie) wins over `Accept-Language`, but only when
//!   the catalog has that locale: an unknown explicit choice falls through to the header.

use crate::error::ContractError;
use crate::i18n::{Locale, MessageBundle, MessageKey};
use serde_json::Value;
use std::collections::BTreeMap;

/// Substitute `{name}` placeholders. Unknown placeholders are kept verbatim; strings are
/// inserted without quotes, other values as JSON.
pub fn substitute(template: &str, params: &BTreeMap<String, Value>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match params.get(name) {
                    Some(Value::String(s)) => out.push_str(s),
                    Some(v) => out.push_str(&v.to_string()),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The available locale matching `tag`: exact tag (case-insensitive), then primary language
/// (`fr-CA` → `fr`, and `fr` → `fr-FR` when only a regional bundle exists).
pub fn match_locale(tag: &str, available: &[Locale]) -> Option<Locale> {
    let tag = tag.trim();
    if tag.is_empty() || tag == "*" {
        return None;
    }
    if let Some(l) = available.iter().find(|l| l.0.eq_ignore_ascii_case(tag)) {
        return Some(l.clone());
    }
    let lang = Locale::new(tag).language();
    available.iter().find(|l| l.language() == lang).cloned()
}

/// Pick the best available locale for an `Accept-Language` value (quality-sorted, ties in
/// header order, exact tag first, then primary language), else `default`. `q=0` excludes a
/// tag; `*` selects `default`.
pub fn negotiate(accept: &str, available: &[Locale], default: &Locale) -> Locale {
    let mut prefs: Vec<(f32, usize, &str)> = accept
        .split(',')
        .enumerate()
        .filter_map(|(i, part)| {
            let mut it = part.trim().split(';');
            let tag = it.next()?.trim();
            if tag.is_empty() {
                return None;
            }
            let q = it
                .find_map(|p| {
                    p.trim()
                        .strip_prefix("q=")
                        .and_then(|q| q.trim().parse::<f32>().ok())
                })
                .unwrap_or(1.0);
            (q > 0.0 && q.is_finite()).then_some((q.min(1.0), i, tag))
        })
        .collect();
    prefs.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    for (_, _, tag) in &prefs {
        if *tag == "*" {
            return default.clone();
        }
        if let Some(l) = match_locale(tag, available) {
            return l;
        }
    }
    default.clone()
}

/// Text of `key` in `locale` (exact locale first, then primary language), `None` if absent.
pub fn lookup<'a>(
    bundles: &[&'a MessageBundle],
    locale: &Locale,
    key: &MessageKey,
) -> Option<&'a str> {
    let lang = locale.language();
    let exact = bundles.iter().filter(|b| &b.locale == locale);
    let by_lang = bundles
        .iter()
        .filter(|b| &b.locale != locale && b.locale.language() == lang);
    exact.chain(by_lang).find_map(|b| b.get(key))
}

/// Render `key` in `locale`. `bundles` are searched in order (module bundles before the
/// framework's); a bundle matches on exact locale, then on primary language. A missing key
/// renders as the key itself (never a panic). For the default-locale fallback, use
/// [`I18nCatalog::render`].
pub fn render(
    bundles: &[&MessageBundle],
    locale: &Locale,
    key: &MessageKey,
    params: &BTreeMap<String, Value>,
) -> String {
    match lookup(bundles, locale, key) {
        Some(t) => substitute(t, params),
        None => key.0.clone(),
    }
}

/// Localize an error and its field violations.
pub fn localize(
    mut err: ContractError,
    bundles: &[&MessageBundle],
    locale: &Locale,
) -> ContractError {
    err.message = render(bundles, locale, &err.message_key, &err.params);
    for f in &mut err.fields {
        f.message = render(bundles, locale, &f.message_key, &f.params);
    }
    err
}

/// Locale hints carried by a request, in priority order. Each surface fills what it has
/// (HTTP: query `?lang=`, cookie, `Accept-Language`; gRPC: metadata; front: user setting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocaleHints<'a> {
    /// Explicit choice in the URL (`?lang=en`).
    pub query: Option<&'a str>,
    /// Persisted choice (cookie `this_locale`).
    pub cookie: Option<&'a str>,
    /// Raw `Accept-Language` value.
    pub accept_language: Option<&'a str>,
}

/// Every message bundle a host (or a front) knows, with its default locale.
///
/// One catalog per process: the framework bundles (`fr`, `en`) plus the bundles of each
/// module (compiled: `include_str!`; dynamic: `ModuleStore::bundle`). Adding the bundle of
/// a `(module, locale)` already present replaces it (hot reload).
#[derive(Debug, Clone, PartialEq)]
pub struct I18nCatalog {
    default_locale: Locale,
    /// Module bundles, searched first.
    modules: Vec<MessageBundle>,
    /// Framework bundles (module `this`), searched last.
    framework: Vec<MessageBundle>,
}

impl Default for I18nCatalog {
    fn default() -> Self {
        Self::framework()
    }
}

impl I18nCatalog {
    /// Empty catalog (no bundle at all): every key renders as itself.
    pub fn empty(default_locale: Locale) -> Self {
        Self {
            default_locale,
            modules: Vec::new(),
            framework: Vec::new(),
        }
    }

    /// The framework catalog: `this.*` messages in `fr` (default) and `en`.
    pub fn framework() -> Self {
        let framework = ["fr", "en"]
            .iter()
            .filter_map(|l| crate::framework::bundle(&Locale::new(*l)))
            .collect();
        Self {
            default_locale: Locale::new("fr"),
            modules: Vec::new(),
            framework,
        }
    }

    /// Change the default locale (fallback when neither hints nor bundles match).
    pub fn with_default_locale(mut self, locale: Locale) -> Self {
        self.default_locale = locale;
        self
    }

    /// Add (or replace) a bundle. A bundle of module `this` goes to the framework layer.
    pub fn with_bundle(mut self, bundle: MessageBundle) -> Self {
        self.add_bundle(bundle);
        self
    }

    /// Add (or replace) a bundle, keyed by `(module, locale)`.
    pub fn add_bundle(&mut self, bundle: MessageBundle) {
        let layer = if bundle.module == crate::framework::FRAMEWORK_MODULE {
            &mut self.framework
        } else {
            &mut self.modules
        };
        match layer
            .iter_mut()
            .find(|b| b.module == bundle.module && b.locale == bundle.locale)
        {
            Some(slot) => *slot = bundle,
            None => layer.push(bundle),
        }
    }

    /// Remove every bundle of a module (unload). Returns how many were removed.
    pub fn remove_module(&mut self, module: &str) -> usize {
        let before = self.modules.len();
        self.modules.retain(|b| b.module != module);
        before - self.modules.len()
    }

    pub fn default_locale(&self) -> &Locale {
        &self.default_locale
    }

    /// Distinct locales having at least one bundle, default locale first.
    pub fn locales(&self) -> Vec<Locale> {
        let mut out = vec![self.default_locale.clone()];
        for b in self.modules.iter().chain(self.framework.iter()) {
            if !out.contains(&b.locale) {
                out.push(b.locale.clone());
            }
        }
        out
    }

    /// All bundles, in lookup order (modules, then framework).
    pub fn bundles(&self) -> Vec<&MessageBundle> {
        self.modules.iter().chain(self.framework.iter()).collect()
    }

    /// Best locale for an `Accept-Language` value.
    pub fn negotiate(&self, accept: &str) -> Locale {
        negotiate(accept, &self.locales(), &self.default_locale)
    }

    /// Resolve the locale of a request: query, then cookie (each only if available), then
    /// `Accept-Language`, then the default locale.
    pub fn resolve(&self, hints: LocaleHints<'_>) -> Locale {
        let available = self.locales();
        for explicit in [hints.query, hints.cookie].into_iter().flatten() {
            if let Some(l) = match_locale(explicit, &available) {
                return l;
            }
        }
        match hints.accept_language {
            Some(a) => negotiate(a, &available, &self.default_locale),
            None => self.default_locale.clone(),
        }
    }

    /// Text of `key`: requested locale, then default locale; `None` if neither has it.
    pub fn lookup(&self, locale: &Locale, key: &MessageKey) -> Option<&str> {
        let bundles = self.bundles();
        lookup(&bundles, locale, key).or_else(|| lookup(&bundles, &self.default_locale, key))
    }

    /// Render `key` with `params`; missing everywhere → the key itself.
    pub fn render(
        &self,
        locale: &Locale,
        key: &MessageKey,
        params: &BTreeMap<String, Value>,
    ) -> String {
        match self.lookup(locale, key) {
            Some(t) => substitute(t, params),
            None => key.0.clone(),
        }
    }

    /// Localize an error and its field violations. A key missing everywhere keeps the
    /// message already in the error when it has one (a module's `with_message` text is
    /// better than its bare key), otherwise renders the key.
    pub fn localize(&self, mut err: ContractError, locale: &Locale) -> ContractError {
        if let Some(t) = self.lookup(locale, &err.message_key) {
            err.message = substitute(t, &err.params);
        } else if err.message.is_empty() {
            err.message = err.message_key.0.clone();
        }
        for f in &mut err.fields {
            if let Some(t) = self.lookup(locale, &f.message_key) {
                f.message = substitute(t, &f.params);
            } else if f.message.is_empty() {
                f.message = f.message_key.0.clone();
            }
        }
        err
    }

    /// Keys of `keys` that have no text in `locale` (its exact bundle or its language).
    /// Used by coverage checks (`this doctor`, framework catalog test).
    pub fn missing<'k>(
        &self,
        locale: &Locale,
        keys: impl IntoIterator<Item = &'k MessageKey>,
    ) -> Vec<MessageKey> {
        let bundles = self.bundles();
        keys.into_iter()
            .filter(|k| lookup(&bundles, locale, k).is_none())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::FieldViolation;
    use crate::framework;

    fn params(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn billing(locale: &str, text: &str) -> MessageBundle {
        MessageBundle {
            module: "billing".into(),
            locale: Locale::new(locale),
            messages: [(MessageKey::new("billing.invoice.already_paid"), text.to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn negotiate_picks_best_quality() {
        let av = [Locale::new("fr"), Locale::new("en")];
        let d = Locale::new("fr");
        // en-US is not available: its language (en) wins over fr;q=0.8.
        assert_eq!(
            negotiate("en-US,en;q=0.9,fr;q=0.8", &av, &d),
            Locale::new("en")
        );
        // Quality, not order, decides.
        assert_eq!(negotiate("fr;q=0.3, en;q=0.7", &av, &d), Locale::new("en"));
        // Unavailable best choice skipped.
        assert_eq!(negotiate("de;q=1, en;q=0.5", &av, &d), Locale::new("en"));
        // Case-insensitive, spaces tolerated, q > 1 clamped (equal → header order).
        assert_eq!(negotiate(" EN ; q=2 , fr", &av, &d), Locale::new("en"));
        // Regional bundle reached from a bare language.
        assert_eq!(
            negotiate("fr", &[Locale::new("fr-CA")], &Locale::new("en")),
            Locale::new("fr-CA")
        );
    }

    #[test]
    fn negotiate_falls_back_to_default() {
        let av = [Locale::new("fr"), Locale::new("en")];
        let d = Locale::new("fr");
        assert_eq!(negotiate("de", &av, &d), d);
        assert_eq!(negotiate("", &av, &d), d);
        assert_eq!(negotiate("*", &av, &d), d);
        assert_eq!(negotiate("en;q=0", &av, &d), d, "q=0 means 'not acceptable'");
        assert_eq!(negotiate(";;,,;q=", &av, &d), d, "garbage never panics");
    }

    #[test]
    fn render_substitutes_params() {
        let en = framework::bundle(&Locale::new("en")).unwrap();
        let p = params(&[("field", Value::String("amount".into()))]);
        let k = MessageKey::new("this.validation.required");
        assert_eq!(
            render(&[&en], &Locale::new("en-GB"), &k, &p),
            "Field 'amount' is required"
        );
        // Non-string values as JSON, unknown placeholders kept, unclosed brace kept.
        let p = params(&[("n", serde_json::json!(-1)), ("s", "x".into())]);
        assert_eq!(substitute("{s}={n} {other} {", &p), "x=-1 {other} {");
    }

    #[test]
    fn missing_key_renders_key_not_panic() {
        let en = framework::bundle(&Locale::new("en")).unwrap();
        let k = MessageKey::new("billing.nope");
        assert_eq!(
            render(&[&en], &Locale::new("en"), &k, &BTreeMap::new()),
            "billing.nope"
        );
        assert_eq!(render(&[], &Locale::new("xx"), &k, &BTreeMap::new()), "billing.nope");
        let cat = I18nCatalog::empty(Locale::new("en"));
        assert_eq!(cat.render(&Locale::new("en"), &k, &BTreeMap::new()), "billing.nope");
        let e = cat.localize(
            ContractError::new("billing.nope", crate::ErrorKind::Conflict).with_message(""),
            &Locale::new("en"),
        );
        assert_eq!(e.message, "billing.nope");
    }

    #[test]
    fn localize_switches_language() {
        let fr = framework::bundle(&Locale::new("fr")).unwrap();
        let en = framework::bundle(&Locale::new("en")).unwrap();
        let e = ContractError::framework(framework::codes::VALIDATION_FAILED);
        assert_eq!(
            localize(e.clone(), &[&fr, &en], &Locale::new("en")).message,
            "Validation failed"
        );
        assert_eq!(
            localize(e, &[&fr, &en], &Locale::new("fr")).message,
            "Validation échouée"
        );
    }

    #[test]
    fn catalog_falls_back_to_default_locale_then_key() {
        let cat = I18nCatalog::framework().with_bundle(billing("fr", "Facture déjà payée"));
        let k = MessageKey::new("billing.invoice.already_paid");
        // No English billing bundle: default locale (fr), not the key.
        assert_eq!(
            cat.render(&Locale::new("en"), &k, &BTreeMap::new()),
            "Facture déjà payée"
        );
        // German: neither requested nor framework; default locale text.
        assert_eq!(
            cat.render(
                &Locale::new("de"),
                &MessageKey::new("this.internal"),
                &BTreeMap::new()
            ),
            "Erreur interne"
        );
    }

    #[test]
    fn catalog_module_bundles_win_and_replace() {
        let mut cat = I18nCatalog::framework().with_bundle(billing("en", "Already paid"));
        let err = ContractError::new("billing.invoice.already_paid", crate::ErrorKind::Conflict)
            .with_field(FieldViolation::new("amount", "this.validation.positive").with_param("field", "amount"));
        let e = cat.localize(err.clone(), &Locale::new("en"));
        assert_eq!(e.message, "Already paid");
        assert!(e.fields[0].message.contains("amount"), "{}", e.fields[0].message);
        cat.add_bundle(billing("en", "Paid already"));
        assert_eq!(cat.localize(err.clone(), &Locale::new("en")).message, "Paid already");
        assert_eq!(cat.remove_module("billing"), 1);
        // Unknown everywhere: the message already present is kept.
        assert_eq!(
            cat.localize(err, &Locale::new("en")).message,
            "billing.invoice.already_paid"
        );
        // A module may reword a framework message.
        let mut over = billing("en", "x");
        over.messages.insert(MessageKey::new("this.internal"), "Oops".into());
        let cat = I18nCatalog::framework().with_bundle(over);
        assert_eq!(
            cat.render(&Locale::new("en"), &MessageKey::new("this.internal"), &BTreeMap::new()),
            "Oops"
        );
    }

    #[test]
    fn catalog_resolve_prefers_query_then_cookie_then_header() {
        let cat = I18nCatalog::framework();
        let h = |q, c, a| LocaleHints {
            query: q,
            cookie: c,
            accept_language: a,
        };
        assert_eq!(cat.resolve(h(Some("en"), Some("fr"), Some("fr"))), Locale::new("en"));
        assert_eq!(cat.resolve(h(None, Some("en-GB"), Some("fr"))), Locale::new("en"));
        assert_eq!(cat.resolve(h(Some("de"), None, Some("en"))), Locale::new("en"));
        assert_eq!(cat.resolve(h(None, None, None)), Locale::new("fr"));
        assert_eq!(cat.locales(), vec![Locale::new("fr"), Locale::new("en")]);
    }

    #[test]
    fn catalog_missing_lists_untranslated_keys() {
        let cat = I18nCatalog::framework().with_bundle(billing("fr", "Payée"));
        let k = [
            MessageKey::new("billing.invoice.already_paid"),
            MessageKey::new("this.internal"),
        ];
        assert_eq!(cat.missing(&Locale::new("fr"), &k), vec![]);
        assert_eq!(cat.missing(&Locale::new("en"), &k), vec![k[0].clone()]);
    }
}
