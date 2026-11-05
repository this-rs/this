//! Où tourne l'app, et comment elle parle au domaine.

use serde::{Deserialize, Serialize};

/// Où tourne l'app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Desktop,
    Mobile,
    Web,
}

impl Surface {
    /// Nom court affiché dans l'en-tête.
    pub fn id(self) -> &'static str {
        match self {
            Surface::Desktop => "desktop",
            Surface::Mobile => "mobile",
            Surface::Web => "web",
        }
    }

    /// Comment cette surface parle au domaine.
    pub fn transport(self) -> &'static str {
        match self {
            Surface::Desktop | Surface::Mobile => "Tauri invoke",
            Surface::Web => "HTTP /api",
        }
    }

    /// Une coquille native (fenêtre, app installée) plutôt qu'un onglet.
    pub fn is_native(self) -> bool {
        !matches!(self, Surface::Web)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transports_differ_by_surface() {
        assert_eq!(Surface::Desktop.transport(), "Tauri invoke");
        assert_eq!(Surface::Mobile.transport(), "Tauri invoke");
        assert_eq!(Surface::Web.transport(), "HTTP /api");
    }

    #[test]
    fn web_is_not_native() {
        assert!(!Surface::Web.is_native());
        assert!(Surface::Desktop.is_native());
        assert!(Surface::Mobile.is_native());
    }
}
