//! # Le retour : quand l'app revient, qu'elle soit une page ou une coquille native
//!
//! Trois sources signalent qu'il faut reconsidérer les données :
//!
//! - **la coquille Tauri** : `shell://event` avec `type: "foreground"` (iOS et Android, quand
//!   le système ramène l'app au premier plan) ;
//! - **la page web** : `visibilitychange` quand l'onglet redevient visible ;
//! - **le réseau** : l'événement `online`, quand la connexion revient.
//!
//! Toutes trois appellent le même `on_resume`. La décision (resynchroniser ? rouvrir le flux ?)
//! appartient à `this_platform::resume_plan` : ce module ne fait que dire « c'est un retour ».

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

fn get(target: &JsValue, key: &str) -> Option<JsValue> {
    js_sys::Reflect::get(target, &JsValue::from_str(key))
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
}

/// Le réseau est-il joignable ? Sans information, on suppose oui : la décision de reprise
/// ne doit pas se bloquer sur une API absente.
pub fn is_online() -> bool {
    web_sys::window()
        .map(|w| w.navigator().on_line())
        .unwrap_or(true)
}

fn on(target: &web_sys::EventTarget, event: &str, f: impl Fn() + 'static) {
    let cb = Closure::<dyn Fn()>::new(f);
    let _ = target.add_event_listener_with_callback(event, cb.as_ref().unchecked_ref());
    cb.forget();
}

/// Appelle `f` à chaque retour : premier plan de la coquille, onglet redevenu visible, réseau
/// revenu. À installer une fois, au démarrage.
pub fn on_resume(f: impl Fn() + Clone + 'static) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let target: &web_sys::EventTarget = win.as_ref();

    // Web : l'onglet redevient visible, ou la connexion revient.
    if let Some(doc) = win.document() {
        let f_visible = f.clone();
        let doc_for_check = doc.clone();
        on(doc.as_ref(), "visibilitychange", move || {
            // `document.visibilityState` : lu directement, sans fonctionnalité web-sys dédiée.
            let visible = get(doc_for_check.as_ref(), "visibilityState")
                .and_then(|v| v.as_string())
                .as_deref()
                == Some("visible");
            if visible {
                f_visible();
            }
        });
    }
    let f_online = f.clone();
    on(target, "online", f_online);

    // Coquille Tauri : le système ramène l'app au premier plan (plugin `this-shell`).
    let Some(tauri) = get(win.as_ref(), "__TAURI__") else {
        return;
    };
    let Some(event) = get(&tauri, "event") else {
        return;
    };
    let Some(listen) = get(&event, "listen").and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    else {
        return;
    };
    let handler = Closure::<dyn Fn(JsValue)>::new(move |evt: JsValue| {
        let kind = get(&evt, "payload")
            .and_then(|p| get(&p, "type"))
            .and_then(|t| t.as_string());
        if kind.as_deref() == Some("foreground") {
            f();
        }
    });
    let _ = listen.call2(
        &event,
        &JsValue::from_str("shell://event"),
        handler.as_ref(),
    );
    handler.forget();
}
