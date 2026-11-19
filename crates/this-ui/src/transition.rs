//! Transitions d'écran — **partie du framework**, pas un style recollé par écran.
//!
//! Un changement de vue (navigation) devient une transition logique : l'appelant
//! enveloppe son changement d'état dans [`naviguer`], le navigateur anime la sortie
//! et l'entrée via la View Transitions API. Le visuel est fourni par les règles
//! `::view-transition-*` de [`crate::views::style`] — partagé par toutes les apps.

/// Exécute `redessiner` dans une transition de vue si le navigateur la supporte
/// (`document.startViewTransition`), sinon exécute simplement l'action (repli).
///
/// ```ignore
/// naviguer(move || open.set(Some("espace".to_string())));
/// ```
pub fn naviguer(redessiner: impl FnOnce() + 'static) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Ok(vt) = js_sys::Reflect::get(
                &doc,
                &wasm_bindgen::JsValue::from_str("startViewTransition"),
            ) {
                if let Some(func) = vt.dyn_ref::<js_sys::Function>() {
                    let cb = wasm_bindgen::closure::Closure::once_into_js(move || redessiner());
                    let _ = func.call1(&doc, &cb);
                    return;
                }
            }
        }
    }
    redessiner();
}
