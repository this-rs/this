//! The host's message catalog: framework bundles (`this.*`, `fr` default, `en`) plus the
//! bundles of every module, shared by all surfaces and swappable at runtime (hot-loaded
//! modules add their bundles, unloaded modules remove them).

use crate::descriptor::check::issues_to_error;
use crate::descriptor::{ContractError, I18nCatalog, Locale, MessageBundle};
use arc_swap::ArcSwap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

/// Framework bundles shipped by this-rs (`fr` default, `en`).
pub fn framework_bundles() -> Vec<MessageBundle> {
    ["fr", "en"]
        .iter()
        .filter_map(|l| crate::descriptor::framework::bundle(&Locale::new(*l)))
        .collect()
}

/// Process-wide framework-only catalog, used when no [`LocaleCatalog`] was installed
/// (an `ApiError` rendered outside the `localize_errors` middleware, a unit test...).
pub fn framework_catalog() -> &'static I18nCatalog {
    static CATALOG: OnceLock<I18nCatalog> = OnceLock::new();
    CATALOG.get_or_init(I18nCatalog::framework)
}

/// Shared, lock-free handle on the host catalog (`ArcSwap` snapshot: readers never hold a
/// guard across `.await`, writers publish a new catalog).
///
/// Installed by the `localize_errors` middleware as a request extension (REST, GraphQL
/// over HTTP) and handed to gRPC services; cheap to clone.
#[derive(Clone)]
pub struct LocaleCatalog(Arc<ArcSwap<I18nCatalog>>);

impl LocaleCatalog {
    pub fn new(catalog: I18nCatalog) -> Self {
        Self(Arc::new(ArcSwap::from_pointee(catalog)))
    }

    /// Framework bundles only (`fr` default, `en`).
    pub fn framework() -> Self {
        Self::new(I18nCatalog::framework())
    }

    /// Current snapshot.
    pub fn load(&self) -> Arc<I18nCatalog> {
        self.0.load_full()
    }

    /// Copy-on-write update (add the bundles of a loaded module, remove an unloaded one).
    pub fn update(&self, f: impl Fn(&mut I18nCatalog)) {
        self.0.rcu(|cur| {
            let mut next = I18nCatalog::clone(cur);
            f(&mut next);
            next
        });
    }

    /// Add (or replace) module bundles, all or nothing: when one bundle is refused
    /// (`I18nCatalog::bundle_issues`: foreign key, unsupported syntax), none is published
    /// and the error lists every issue.
    // `ContractError` is the envelope every surface returns unboxed (same allowance as the
    // `this-descriptor` crate); this runs at module load, not on a hot path.
    #[allow(clippy::result_large_err)]
    pub fn add_bundles(
        &self,
        bundles: impl IntoIterator<Item = MessageBundle>,
    ) -> Result<(), ContractError> {
        let bundles: Vec<MessageBundle> = bundles.into_iter().collect();
        let issues: Vec<_> = bundles
            .iter()
            .flat_map(I18nCatalog::bundle_issues)
            .collect();
        if !issues.is_empty() {
            return Err(issues_to_error(&issues));
        }
        self.update(|c| {
            for b in &bundles {
                // Cannot fail: every bundle passed `bundle_issues` above.
                let _ = c.add_bundle(b.clone());
            }
        });
        Ok(())
    }

    /// Remove every bundle of an unloaded module. Returns how many were removed.
    pub fn remove_module(&self, module: &str) -> usize {
        let removed = std::sync::atomic::AtomicUsize::new(0);
        self.update(|c| removed.store(c.remove_module(module), Ordering::Relaxed));
        removed.into_inner()
    }
}

impl Default for LocaleCatalog {
    fn default() -> Self {
        Self::framework()
    }
}

impl std::fmt::Debug for LocaleCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocaleCatalog")
            .field("locales", &self.load().locales())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::MessageKey;
    use crate::descriptor::framework::{self, FRAMEWORK_ERRORS};
    use std::collections::BTreeSet;

    /// Codes the RFC (§3.2, "Codes du framework") lists by name, plus the codes the error
    /// envelope promises to every surface (`entity.not_found`, `auth.forbidden`...).
    const RFC_NAMED_CODES: &[&str] = &[
        "this.validation.failed",
        "this.validation.required",
        "this.validation.type_mismatch",
        "this.validation.positive",
        "this.validation.string_length",
        "this.validation.min_value",
        "this.validation.max_value",
        "this.validation.in_list",
        "this.validation.date_format",
        "this.validation.pattern",
        "this.validation.email",
        "this.validation.url",
        "this.validation.read_only",
        "this.validation.immutable",
        "this.validation.unknown_field",
        "this.validation.not_object",
        "this.validation.custom",
        "this.module.duplicate_module",
        "this.module.duplicate_plural",
        "this.module.duplicate_error_code",
        "this.module.plural_mismatch",
        "this.module.identity_mismatch",
        "this.module.reference_unknown",
        "this.module.illegal_transition",
        "this.vfs.precondition_failed",
        "this.not_implemented",
        "this.internal",
        "this.entity.not_found",
        "this.auth.forbidden",
        "this.auth.unauthenticated",
    ];

    /// Namespaces the RFC lists with a wildcard (`this.query.*`...): at least one code each.
    const RFC_NAMESPACES: &[&str] = &[
        "this.validation.",
        "this.query.",
        "this.auth.",
        "this.entity.",
        "this.module.",
        "this.descriptor.",
        "this.vfs.",
        "this.transport.",
    ];

    #[test]
    fn catalog_covers_all_framework_codes() {
        let cat = framework_catalog();
        let codes: BTreeSet<&str> = FRAMEWORK_ERRORS.iter().map(|e| e.0).collect();
        for c in RFC_NAMED_CODES {
            assert!(codes.contains(c), "RFC code {c} is not in FRAMEWORK_ERRORS");
        }
        for ns in RFC_NAMESPACES {
            assert!(codes.iter().any(|c| c.starts_with(ns)), "no code in {ns}*");
        }
        let keys: Vec<MessageKey> = codes.iter().map(|c| MessageKey::new(*c)).collect();
        for locale in ["fr", "en"] {
            let l = Locale::new(locale);
            assert_eq!(cat.missing(&l, &keys), vec![], "untranslated in {locale}");
        }
        // Each text carries every declared parameter, and fr != en (no copy-paste).
        for (code, _, params, fr, en) in FRAMEWORK_ERRORS {
            let key = MessageKey::new(*code);
            let tfr = cat.lookup(&Locale::new("fr"), &key).unwrap();
            let ten = cat.lookup(&Locale::new("en"), &key).unwrap();
            assert_eq!((tfr, ten), (*fr, *en), "{code}");
            for p in *params {
                let ph = format!("{{{p}}}");
                assert!(tfr.contains(&ph) && ten.contains(&ph), "{code}: {ph}");
            }
            assert_ne!(tfr, ten, "{code}: fr and en identical");
        }
        assert_eq!(framework_bundles().len(), 2);
        assert_eq!(cat.default_locale(), &Locale::new("fr"));
        assert_eq!(
            framework::descriptor().i18n.keys.len(),
            FRAMEWORK_ERRORS.len()
        );
    }

    #[test]
    fn locale_catalog_hot_adds_and_removes_module_bundles() {
        let cat = LocaleCatalog::framework();
        let before = cat.load();
        let bundle = |module: &str, key: &str| MessageBundle {
            module: module.into(),
            locale: Locale::new("de"),
            messages: [(MessageKey::new(key), "Bezahlt".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        cat.add_bundles([bundle("billing", "billing.paid")])
            .unwrap();
        assert!(cat.load().locales().contains(&Locale::new("de")));
        assert!(
            !before.locales().contains(&Locale::new("de")),
            "old snapshot unchanged"
        );
        // All or nothing: one foreign key refuses the whole batch.
        let snapshot = cat.load();
        let err = cat
            .add_bundles([
                bundle("crm", "crm.contact"),
                bundle("crm", "this.auth.forbidden"),
            ])
            .unwrap_err();
        assert_eq!(err.code.as_str(), "this.descriptor.invalid");
        assert_eq!(*cat.load(), *snapshot, "nothing published");
        assert_eq!(cat.remove_module("billing"), 1);
        assert!(!cat.load().locales().contains(&Locale::new("de")));
        assert_eq!(cat.remove_module("billing"), 0);
    }
}
