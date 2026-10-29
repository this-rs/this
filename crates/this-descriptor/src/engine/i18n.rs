//! Locale negotiation and message rendering (`{param}` substitution).
//!
//! Pure (wasm32): the same functions run on the server (error bodies, gRPC `Status`,
//! GraphQL `extensions`) and in a Leptos front (rendering a `ContractError` received from a
//! server function, or produced by a client-side `engine::validate::apply`).
//!
//! Rules, identical on every side:
//! - a key is looked up in the requested locale (exact tag, then primary language), then in
//!   the catalog's default locale, then in any bundle that has it (a module that ships only
//!   `en` in a `fr` host still shows English, not its key), and only then rendered as **the
//!   key itself** (never a panic, never an empty string);
//! - a bundle only carries keys of its own namespace (`<module>.`, `this.` for the
//!   framework) and only the `simple` syntax: [`I18nCatalog::add_bundle`] refuses the rest.
//!   A module can therefore neither reword a `this.*` message for every other module nor
//!   shadow another module's keys (the catalog does not know which module raised an error,
//!   so such an override would be global and load-order dependent);
//! - an explicit choice (query parameter, cookie) wins over `Accept-Language`, but only when
//!   the catalog has that locale: an unknown explicit choice falls through to the header.

use crate::check::{DescriptorIssue, issues_to_error};
use crate::error::{ContractError, ErrorCode};
use crate::i18n::{Locale, MessageBundle, MessageKey, MessageSyntax};
use crate::module::ModuleDescriptor;
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
    pub fn with_bundle(mut self, bundle: MessageBundle) -> Result<Self, ContractError> {
        self.add_bundle(bundle)?;
        Ok(self)
    }

    /// Why `bundle` cannot enter a catalog (empty when it can): unsupported syntax, or a
    /// key outside the bundle's namespace (`<module>.`).
    pub fn bundle_issues(bundle: &MessageBundle) -> Vec<DescriptorIssue> {
        let path = format!("i18n/{}/{}.json", bundle.module, bundle.locale);
        let mut issues = Vec::new();
        if bundle.syntax != MessageSyntax::Simple {
            issues.push(DescriptorIssue {
                code: ErrorCode::new("this.descriptor.syntax_unsupported"),
                path: path.clone(),
                message: "only the `simple` syntax is rendered today".into(),
            });
        }
        let ns = format!("{}.", bundle.module);
        if bundle.module.is_empty() {
            issues.push(DescriptorIssue {
                code: ErrorCode::new("this.descriptor.bundle_namespace"),
                path: path.clone(),
                message: "a bundle must name its module".into(),
            });
        }
        for key in bundle.messages.keys() {
            if bundle.module.is_empty() || !key.0.starts_with(&ns) {
                issues.push(DescriptorIssue {
                    code: ErrorCode::new("this.descriptor.bundle_namespace"),
                    path: format!("{path}:{key}"),
                    message: format!("`{key}` is outside the namespace `{ns}`"),
                });
            }
        }
        issues
    }

    /// Add (or replace) a bundle, keyed by `(module, locale)`.
    ///
    /// Refused as a whole (`this.descriptor.invalid`, catalog unchanged) when its syntax is
    /// not `simple` or when one of its keys is outside `<module>.`: a module cannot reword
    /// `this.*` texts nor another module's texts.
    pub fn add_bundle(&mut self, bundle: MessageBundle) -> Result<(), ContractError> {
        let issues = Self::bundle_issues(&bundle);
        if !issues.is_empty() {
            return Err(issues_to_error(&issues));
        }
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
        Ok(())
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

    /// Text of `key`: requested locale, then default locale, then the first bundle (lookup
    /// order) that has it; `None` if no bundle has it.
    pub fn lookup(&self, locale: &Locale, key: &MessageKey) -> Option<&str> {
        let bundles = self.bundles();
        lookup(&bundles, locale, key)
            .or_else(|| lookup(&bundles, &self.default_locale, key))
            .or_else(|| bundles.iter().find_map(|b| b.get(key)))
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

    /// Coverage of a descriptor: every key it declares (`i18n.keys`, the message keys of its
    /// `errors`, entity and field labels) that has no text in one of the locales it declares
    /// (`i18n.locales` plus `i18n.default_locale`). Sorted, without duplicates.
    pub fn missing_for(&self, desc: &ModuleDescriptor) -> Vec<(Locale, MessageKey)> {
        let mut keys: Vec<&MessageKey> = desc.i18n.keys.iter().map(|k| &k.key).collect();
        keys.extend(desc.errors.iter().map(|e| &e.message_key));
        for e in &desc.entities {
            keys.extend(e.i18n.label.iter().chain(e.i18n.label_plural.iter()));
            for f in &e.fields {
                keys.extend(
                    f.i18n
                        .label
                        .iter()
                        .chain(f.i18n.help.iter())
                        .chain(f.i18n.placeholder.iter()),
                );
            }
        }
        keys.sort();
        keys.dedup();
        let mut locales = vec![desc.i18n.default_locale.clone()];
        for l in &desc.i18n.locales {
            if !locales.contains(l) {
                locales.push(l.clone());
            }
        }
        let mut out = Vec::new();
        for l in &locales {
            for k in self.missing(l, keys.iter().copied()) {
                out.push((l.clone(), k));
            }
        }
        out.sort();
        out.dedup();
        out
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
            messages: [(
                MessageKey::new("billing.invoice.already_paid"),
                text.to_string(),
            )]
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
        assert_eq!(
            negotiate("en;q=0", &av, &d),
            d,
            "q=0 means 'not acceptable'"
        );
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
        assert_eq!(
            render(&[], &Locale::new("xx"), &k, &BTreeMap::new()),
            "billing.nope"
        );
        let cat = I18nCatalog::empty(Locale::new("en"));
        assert_eq!(
            cat.render(&Locale::new("en"), &k, &BTreeMap::new()),
            "billing.nope"
        );
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
        let cat = I18nCatalog::framework()
            .with_bundle(billing("fr", "Facture déjà payée"))
            .unwrap();
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
        let mut cat = I18nCatalog::framework()
            .with_bundle(billing("en", "Already paid"))
            .unwrap();
        let err = ContractError::new("billing.invoice.already_paid", crate::ErrorKind::Conflict)
            .with_field(
                FieldViolation::new("amount", "this.validation.positive")
                    .with_param("field", "amount"),
            );
        let e = cat.localize(err.clone(), &Locale::new("en"));
        assert_eq!(e.message, "Already paid");
        assert!(
            e.fields[0].message.contains("amount"),
            "{}",
            e.fields[0].message
        );
        cat.add_bundle(billing("en", "Paid already")).unwrap();
        assert_eq!(
            cat.localize(err.clone(), &Locale::new("en")).message,
            "Paid already"
        );
        assert_eq!(cat.remove_module("billing"), 1);
        // Unknown everywhere: the message already present is kept.
        assert_eq!(
            cat.localize(err, &Locale::new("en")).message,
            "billing.invoice.already_paid"
        );
    }

    #[test]
    fn catalog_refuses_foreign_keys_and_unsupported_syntax() {
        let mut cat = I18nCatalog::framework();
        let before = cat.clone();
        let internal = MessageKey::new("this.internal");
        // A module rewording a framework text would do it for EVERY module: refused whole.
        let mut over = billing("en", "x");
        over.messages.insert(internal.clone(), "Oops".into());
        let err = cat.add_bundle(over).unwrap_err();
        assert_eq!(err.code.as_str(), framework::codes::DESCRIPTOR_INVALID);
        assert!(
            err.params["issues"]
                .as_str()
                .unwrap()
                .contains("this.descriptor.bundle_namespace"),
            "{err:?}"
        );
        assert_eq!(cat, before, "refused bundle leaves the catalog unchanged");
        assert_eq!(
            cat.render(&Locale::new("en"), &internal, &BTreeMap::new()),
            "Internal error"
        );
        // Nor another module's keys.
        let mut steal = billing("en", "x");
        steal
            .messages
            .insert(MessageKey::new("crm.contact.label"), "Hijack".into());
        assert!(cat.add_bundle(steal).is_err());
        // Nor a bundle without module.
        let mut anon = billing("en", "x");
        anon.module.clear();
        assert!(cat.add_bundle(anon).is_err());
        // Fluent is reserved: refused, not rendered as `simple`.
        let mut fluent = billing("en", "{ $n -> [one] x *[other] y }");
        fluent.syntax = MessageSyntax::Fluent;
        let err = I18nCatalog::framework().with_bundle(fluent).unwrap_err();
        assert!(
            err.params["issues"]
                .as_str()
                .unwrap()
                .contains("this.descriptor.syntax_unsupported")
        );
        assert_eq!(cat, before);
        assert!(I18nCatalog::bundle_issues(&billing("en", "ok")).is_empty());
    }

    #[test]
    fn catalog_last_resort_is_any_bundle_before_the_key() {
        // Host default `fr`, module ships only `de`: a `en` request shows German, not the key.
        let cat = I18nCatalog::framework()
            .with_bundle(billing("de", "Bereits bezahlt"))
            .unwrap();
        let k = MessageKey::new("billing.invoice.already_paid");
        assert_eq!(
            cat.render(&Locale::new("en"), &k, &BTreeMap::new()),
            "Bereits bezahlt"
        );
        // `missing` stays strict (coverage reports must not be hidden by the fallback).
        assert_eq!(cat.missing(&Locale::new("en"), [&k]), vec![k.clone()]);
    }

    #[test]
    fn catalog_missing_for_descriptor_covers_declared_keys_and_locales() {
        use crate::error::{ErrorDesc, ErrorKind};
        use crate::i18n::MessageDecl;
        let mut desc = ModuleDescriptor::new("billing", "1.0.0");
        desc.i18n.default_locale = Locale::new("fr");
        desc.i18n.locales = vec![Locale::new("fr"), Locale::new("en")];
        desc.i18n.keys.push(MessageDecl {
            key: MessageKey::new("billing.invoice.already_paid"),
            params: vec![],
        });
        desc.errors.push(ErrorDesc {
            code: ErrorCode::new("billing.invoice.overdue"),
            kind: ErrorKind::Conflict,
            message_key: MessageKey::new("billing.invoice.overdue"),
            params: vec![],
            description: None,
        });
        let cat = I18nCatalog::framework()
            .with_bundle(billing("fr", "Déjà payée"))
            .unwrap();
        let missing = cat.missing_for(&desc);
        assert_eq!(
            missing,
            vec![
                (
                    Locale::new("en"),
                    MessageKey::new("billing.invoice.already_paid")
                ),
                (
                    Locale::new("en"),
                    MessageKey::new("billing.invoice.overdue")
                ),
                (
                    Locale::new("fr"),
                    MessageKey::new("billing.invoice.overdue")
                ),
            ],
            "error message keys count, every declared locale is checked"
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
        assert_eq!(
            cat.resolve(h(Some("en"), Some("fr"), Some("fr"))),
            Locale::new("en")
        );
        assert_eq!(
            cat.resolve(h(None, Some("en-GB"), Some("fr"))),
            Locale::new("en")
        );
        assert_eq!(
            cat.resolve(h(Some("de"), None, Some("en"))),
            Locale::new("en")
        );
        assert_eq!(cat.resolve(h(None, None, None)), Locale::new("fr"));
        assert_eq!(cat.locales(), vec![Locale::new("fr"), Locale::new("en")]);
    }

    #[test]
    fn catalog_missing_lists_untranslated_keys() {
        let cat = I18nCatalog::framework()
            .with_bundle(billing("fr", "Payée"))
            .unwrap();
        let k = [
            MessageKey::new("billing.invoice.already_paid"),
            MessageKey::new("this.internal"),
        ];
        assert_eq!(cat.missing(&Locale::new("fr"), &k), vec![]);
        assert_eq!(cat.missing(&Locale::new("en"), &k), vec![k[0].clone()]);
    }
}
