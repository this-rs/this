//! Locale negotiation and message rendering (`{param}` substitution).

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

/// Pick the best available locale for an `Accept-Language` value (quality-sorted, exact
/// tag first, then primary language), else `default`.
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
                        .and_then(|q| q.parse::<f32>().ok())
                })
                .unwrap_or(1.0);
            (q > 0.0).then_some((q, i, tag))
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
        if let Some(l) = available.iter().find(|l| l.0.eq_ignore_ascii_case(tag)) {
            return l.clone();
        }
        let lang = Locale::new(*tag).language();
        if let Some(l) = available.iter().find(|l| l.language() == lang) {
            return l.clone();
        }
    }
    default.clone()
}

/// Render `key` in `locale`. `bundles` are searched in order (module bundles before the
/// framework's); a bundle matches on exact locale, then on primary language. A missing key
/// renders as the key itself (never a panic).
pub fn render(
    bundles: &[&MessageBundle],
    locale: &Locale,
    key: &MessageKey,
    params: &BTreeMap<String, Value>,
) -> String {
    let lang = locale.language();
    let exact = bundles.iter().filter(|b| &b.locale == locale);
    let by_lang = bundles.iter().filter(|b| b.locale.language() == lang);
    for b in exact.chain(by_lang) {
        if let Some(t) = b.get(key) {
            return substitute(t, params);
        }
    }
    key.0.clone()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework;

    #[test]
    fn negotiate_picks_best_quality_and_falls_back() {
        let av = [Locale::new("fr"), Locale::new("en")];
        let d = Locale::new("fr");
        assert_eq!(
            negotiate("en-US,en;q=0.9,fr;q=0.8", &av, &d),
            Locale::new("en")
        );
        assert_eq!(negotiate("de;q=1, fr;q=0.5", &av, &d), Locale::new("fr"));
        assert_eq!(negotiate("de", &av, &d), d);
        assert_eq!(negotiate("", &av, &d), d);
    }

    #[test]
    fn render_substitutes_params_and_missing_key_renders_key() {
        let en = framework::bundle(&Locale::new("en")).unwrap();
        let mut p = BTreeMap::new();
        p.insert("field".to_string(), Value::String("amount".into()));
        let k = MessageKey::new("this.validation.required");
        assert_eq!(
            render(&[&en], &Locale::new("en-GB"), &k, &p),
            "Field 'amount' is required"
        );
        assert_eq!(
            render(&[&en], &Locale::new("en"), &MessageKey::new("x.y"), &p),
            "x.y"
        );
        assert_eq!(substitute("{a} {b}", &p), "{a} {b}");
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
}
