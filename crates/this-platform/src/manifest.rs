//! Writing the declarations a set of capabilities needs, and refusing to write an incomplete set.
//!
//! A missing `NSCameraUsageDescription` does not make a permission request fail: the system
//! ends the app the moment it asks. So the generator is **fail-closed**: a capability whose
//! text is missing is an error at build time, never a blank string in the shipped app.
//!
//! The text itself belongs to the app (it is what the person reads), so it is supplied by the
//! caller, keyed by the plist key.

use crate::capability::{Capability, required_android_permissions, required_ios_usage};

/// Keys required by the capabilities, for which the caller gave no non-empty text.
pub fn missing_usage_texts<'a>(
    capabilities: impl IntoIterator<Item = &'a Capability> + Clone,
    texts: &[(&str, &str)],
) -> Vec<&'static str> {
    required_ios_usage(capabilities.clone())
        .into_iter()
        .filter(|key| {
            !texts
                .iter()
                .any(|(k, text)| k == key && !text.trim().is_empty())
        })
        .collect()
}

/// The `Info.plist` entries for a set of capabilities, as XML `<key>` / `<string>` / `<true/>`
/// pairs, ready to be placed inside the root `<dict>`.
///
/// Returns the missing keys instead of a partial plist when any text is absent.
pub fn info_plist_entries<'a>(
    capabilities: impl IntoIterator<Item = &'a Capability> + Clone,
    texts: &[(&str, &str)],
) -> Result<String, Vec<&'static str>> {
    let missing = missing_usage_texts(capabilities.clone(), texts);
    if !missing.is_empty() {
        return Err(missing);
    }
    let mut xml = String::new();
    for key in required_ios_usage(capabilities.clone()) {
        let text = texts
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, t)| *t)
            .unwrap_or_default();
        xml.push_str(&format!(
            "\t<key>{}</key>\n\t<string>{}</string>\n",
            escape(key),
            escape(text)
        ));
    }
    // feature switches: keys whose value is `true`, not a dialog text
    let mut flags: Vec<&'static str> = capabilities
        .into_iter()
        .flat_map(|c| c.keys().ios_flags.iter().copied())
        .collect();
    flags.sort_unstable();
    flags.dedup();
    for key in flags {
        xml.push_str(&format!("\t<key>{}</key>\n\t<true/>\n", escape(key)));
    }
    Ok(xml)
}

/// The `AndroidManifest.xml` `<uses-permission>` lines for a set of capabilities.
pub fn android_permissions<'a>(capabilities: impl IntoIterator<Item = &'a Capability>) -> String {
    required_android_permissions(capabilities)
        .into_iter()
        .map(|p| format!("\t<uses-permission android:name=\"{}\" />\n", escape(p)))
        .collect()
}

/// Minimal XML escaping for text placed in element content and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_text_is_an_error_not_a_blank_entry() {
        let caps = [Capability::Camera, Capability::Location];
        let err = info_plist_entries(&caps, &[("NSCameraUsageDescription", "Scan des codes")])
            .unwrap_err();
        assert_eq!(err, vec!["NSLocationWhenInUseUsageDescription"]);
    }

    #[test]
    fn a_blank_text_counts_as_missing() {
        let caps = [Capability::Camera];
        assert_eq!(
            missing_usage_texts(&caps, &[("NSCameraUsageDescription", "   ")]),
            vec!["NSCameraUsageDescription"]
        );
    }

    #[test]
    fn complete_texts_render_each_key_once_in_order() {
        let caps = [Capability::Location, Capability::Camera];
        let xml = info_plist_entries(
            &caps,
            &[
                (
                    "NSLocationWhenInUseUsageDescription",
                    "Trouver la boutique proche",
                ),
                ("NSCameraUsageDescription", "Scanner un code"),
            ],
        )
        .unwrap();
        let camera = xml.find("NSCameraUsageDescription").unwrap();
        let location = xml.find("NSLocationWhenInUseUsageDescription").unwrap();
        assert!(camera < location, "stable order");
        assert_eq!(
            xml.matches("<key>NSCameraUsageDescription</key>").count(),
            1
        );
        assert!(xml.contains("<string>Scanner un code</string>"));
    }

    #[test]
    fn live_activities_render_a_true_switch_not_a_text() {
        let xml = info_plist_entries(&[Capability::LiveActivity], &[]).unwrap();
        assert!(xml.contains("<key>NSSupportsLiveActivities</key>\n\t<true/>"));
        assert!(!xml.contains("<string>"));
    }

    #[test]
    fn texts_are_escaped_so_the_plist_stays_well_formed() {
        let xml = info_plist_entries(
            &[Capability::Camera],
            &[("NSCameraUsageDescription", "Scanner <tout> & \"plus\"")],
        )
        .unwrap();
        assert!(xml.contains("Scanner &lt;tout&gt; &amp; &quot;plus&quot;"));
    }

    #[test]
    fn android_permissions_are_listed_without_duplicates() {
        let xml = android_permissions(&[
            Capability::Camera,
            Capability::Camera,
            Capability::Notifications,
        ]);
        assert_eq!(xml.matches("android.permission.CAMERA").count(), 1);
        assert!(xml.contains("android.permission.POST_NOTIFICATIONS"));
    }

    #[test]
    fn nothing_is_declared_for_capabilities_that_need_nothing() {
        assert_eq!(info_plist_entries(&[Capability::Share], &[]).unwrap(), "");
        assert_eq!(android_permissions(&[Capability::Clipboard]), "");
    }
}
