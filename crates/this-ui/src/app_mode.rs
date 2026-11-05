//! Comportement de coquille : **une app**, pas une page web.
//!
//! Un webview natif ne doit pas se comporter comme un onglet : pas de zoom (pincement,
//! double-tap, focus sur un champ), pas de sélection accidentelle en touchant un bouton,
//! pas de menu contextuel sur appui long, pas de rebond élastique en haut de page.
//!
//! Le **site** garde ces comportements : ce sont ceux d'une page, et les retirer casserait
//! le zoom d'accessibilité. Une exception : une page **installée** sur l'écran d'accueil
//! (`display: standalone`, ou `navigator.standalone` sur iOS) n'est plus une page — elle
//! s'ouvre en plein écran, sans navigateur, et doit se comporter comme l'app native.

use this_app::Surface;
use wasm_bindgen::{JsCast, JsValue};

/// Largeur du viewport d'une coquille native : zoom verrouillé.
const APP_VIEWPORT: &str =
    "width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, viewport-fit=cover";

/// La page a-t-elle été installée sur un écran d'accueil ?
fn is_standalone() -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };

    // iOS (historique, et c'est ce qui marche encore sur Safari) : `navigator.standalone`.
    let navigator = win.navigator();
    if let Ok(v) = js_sys::Reflect::get(navigator.as_ref(), &JsValue::from_str("standalone")) {
        if v.as_bool() == Some(true) {
            return true;
        }
    }

    // Standard : `matchMedia('(display-mode: standalone)').matches`.
    let Ok(match_media) = js_sys::Reflect::get(&win, &JsValue::from_str("matchMedia")) else {
        return false;
    };
    let Ok(match_media) = match_media.dyn_into::<js_sys::Function>() else {
        return false;
    };
    let Ok(list) = match_media.call1(&win, &JsValue::from_str("(display-mode: standalone)")) else {
        return false;
    };
    js_sys::Reflect::get(&list, &JsValue::from_str("matches"))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Marque le document comme surface et, si c'est une app, le verrouille.
pub fn behave_like_app(surface: Surface) {
    // Une page installée sur l'écran d'accueil est une coquille, pas un onglet.
    let surface = if surface.is_native() || !is_standalone() {
        surface
    } else {
        Surface::Mobile
    };

    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(root) = doc.document_element() else {
        return;
    };
    let _ = root.set_attribute("data-surface", surface.id());

    if !surface.is_native() {
        // Une page reste une page : zoom et sélection d'accessibilité intacts.
        return;
    }

    if let Ok(Some(meta)) = doc.query_selector("meta[name=viewport]") {
        let _ = meta.set_attribute("content", APP_VIEWPORT);
    }
}
