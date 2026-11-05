//! Application des réglages au document.
//!
//! Un seul endroit écrit `data-theme` et `data-motion` sur `<html>` : le CSS fait le reste.
//! `Auto` **retire** l'attribut, ce qui laisse `prefers-color-scheme` décider.

use this_app::{Settings, Theme};

pub use crate::storage::now_ms;

fn root() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.document_element()
}

pub fn apply_settings(settings: &Settings) {
    let Some(root) = root() else { return };

    match settings.theme {
        Theme::Auto => {
            let _ = root.remove_attribute("data-theme");
        }
        Theme::Light => {
            let _ = root.set_attribute("data-theme", "light");
        }
        Theme::Dark => {
            let _ = root.set_attribute("data-theme", "dark");
        }
    }

    if settings.reduce_motion {
        let _ = root.set_attribute("data-motion", "off");
    } else {
        let _ = root.remove_attribute("data-motion");
    }
}
