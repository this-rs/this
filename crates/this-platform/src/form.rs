//! What system runs the app, and what kind of screen it has.

use serde::{Deserialize, Serialize};

/// The operating system that dresses the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
    Ios,
    Android,
    /// A browser tab, or an installed web page.
    #[default]
    Web,
}

impl Platform {
    pub fn id(self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Web => "web",
        }
    }

    pub fn is_desktop(self) -> bool {
        matches!(self, Platform::Macos | Platform::Windows | Platform::Linux)
    }

    pub fn is_mobile(self) -> bool {
        matches!(self, Platform::Ios | Platform::Android)
    }

    /// The screen this system usually runs on, when the shell does not say more.
    pub fn default_form_factor(self) -> FormFactor {
        match self {
            Platform::Macos | Platform::Windows | Platform::Linux => FormFactor::Desktop,
            Platform::Ios | Platform::Android => FormFactor::Phone,
            Platform::Web => FormFactor::Web,
        }
    }
}

/// The kind of screen the app is shown on. Several form factors can share one platform
/// (an iPad and an iPhone are both [`Platform::Ios`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormFactor {
    Desktop,
    Phone,
    Tablet,
    /// A companion screen (Apple Watch, Wear OS). Reserved: no shell targets it yet.
    Watch,
    #[default]
    Web,
}

impl FormFactor {
    /// A native window or app, as opposed to a browser tab.
    pub fn is_native(self) -> bool {
        !matches!(self, FormFactor::Web)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_system_has_a_default_screen() {
        assert_eq!(Platform::Macos.default_form_factor(), FormFactor::Desktop);
        assert_eq!(Platform::Ios.default_form_factor(), FormFactor::Phone);
        assert_eq!(Platform::Web.default_form_factor(), FormFactor::Web);
    }

    #[test]
    fn desktop_and_mobile_are_disjoint() {
        for p in [
            Platform::Macos,
            Platform::Windows,
            Platform::Linux,
            Platform::Ios,
            Platform::Android,
            Platform::Web,
        ] {
            assert!(!(p.is_desktop() && p.is_mobile()), "{p:?}");
        }
    }

    #[test]
    fn the_wire_names_are_stable() {
        assert_eq!(serde_json::to_value(Platform::Ios).unwrap(), "ios");
        assert_eq!(serde_json::to_value(FormFactor::Phone).unwrap(), "phone");
    }
}
