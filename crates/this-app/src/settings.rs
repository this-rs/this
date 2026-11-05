//! Les réglages de l'app et leur persistance.
//!
//! Le stockage est un trait : `localStorage` dans un webview, un fichier dans une coquille
//! native, une map en mémoire dans les tests. La validation (JSON illisible, valeur
//! inconnue) est ici, une seule fois, au lieu d'être refaite par chaque surface.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Clé de stockage. Stable : la changer perd les réglages des utilisateurs.
pub const KEY: &str = "this.app.settings";

/// Thème de l'interface. `Auto` suit le réglage du système.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    /// Lecture tolérante : une valeur inconnue retombe sur `Auto`.
    pub fn parse(value: &str) -> Theme {
        match value {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::Auto,
        }
    }

    /// Cycle du bouton de thème : auto → clair → sombre → auto.
    pub fn next(self) -> Theme {
        match self {
            Theme::Auto => Theme::Light,
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Auto,
        }
    }

    /// Libellé français, pour le panneau de réglages.
    pub fn label(self) -> &'static str {
        match self {
            Theme::Auto => "Automatique",
            Theme::Light => "Clair",
            Theme::Dark => "Sombre",
        }
    }
}

/// Les réglages, dans l'ordre où ils sont écrits sur disque.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub theme: Theme,
    /// Coupe animations et transitions (accessibilité, ou appareil lent).
    #[serde(default)]
    pub reduce_motion: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Auto,
            reduce_motion: false,
        }
    }
}

/// Un stockage clé/valeur minimal.
pub trait Store {
    fn read(&self, key: &str) -> Option<String>;
    fn write(&mut self, key: &str, value: String);
}

impl Settings {
    /// Lit les réglages ; toute anomalie retombe sur les valeurs par défaut.
    ///
    /// Un JSON corrompu ne doit jamais empêcher l'app de démarrer : on préfère des
    /// réglages par défaut à un écran blanc.
    pub fn load(store: &dyn Store) -> Settings {
        store
            .read(KEY)
            .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
            .unwrap_or_default()
    }

    /// Écrit les réglages. Renvoie l'erreur de sérialisation, si elle arrive un jour.
    pub fn save(&self, store: &mut dyn Store) -> Result<(), serde_json::Error> {
        let raw = serde_json::to_string(self)?;
        store.write(KEY, raw);
        Ok(())
    }
}

/// Stockage en mémoire : suffit aux tests et à un démarrage sans persistance.
#[derive(Default, Debug)]
pub struct MemoryStore(BTreeMap<String, String>);

impl MemoryStore {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Contenu brut, pour vérifier ce qui a réellement été écrit.
    pub fn raw(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

impl Store for MemoryStore {
    fn read(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn write(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_nothing_is_stored() {
        let store = MemoryStore::new();
        assert_eq!(Settings::load(&store), Settings::default());
    }

    #[test]
    fn roundtrip_through_the_store() {
        let mut store = MemoryStore::new();
        let settings = Settings {
            theme: Theme::Dark,
            reduce_motion: true,
        };
        settings.save(&mut store).unwrap();
        assert_eq!(Settings::load(&store), settings);
        assert!(store.raw().contains_key(KEY));
    }

    #[test]
    fn corrupt_json_falls_back_to_defaults_instead_of_panicking() {
        let mut store = MemoryStore::new();
        store.write(KEY, "{ceci n'est pas du json".into());
        assert_eq!(Settings::load(&store), Settings::default());
    }

    #[test]
    fn unknown_theme_value_falls_back_to_auto() {
        let mut store = MemoryStore::new();
        store.write(KEY, r#"{"theme":"neon","reduce_motion":false}"#.into());
        assert_eq!(Settings::load(&store).theme, Theme::Auto);
    }

    #[test]
    fn partial_json_keeps_other_defaults() {
        let mut store = MemoryStore::new();
        store.write(KEY, r#"{"reduce_motion":true}"#.into());
        let s = Settings::load(&store);
        assert_eq!(s.theme, Theme::Auto);
        assert!(s.reduce_motion);
    }

    #[test]
    fn theme_cycles_auto_light_dark() {
        assert_eq!(Theme::Auto.next(), Theme::Light);
        assert_eq!(Theme::Light.next(), Theme::Dark);
        assert_eq!(Theme::Dark.next(), Theme::Auto);
    }

    #[test]
    fn theme_strings_are_stable() {
        for t in [Theme::Auto, Theme::Light, Theme::Dark] {
            assert_eq!(Theme::parse(t.as_str()), t);
        }
        assert_eq!(Theme::parse("n'importe quoi"), Theme::Auto);
    }
}
