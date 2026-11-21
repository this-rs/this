//! Transitions d'écran — **partie du framework**, pas un style recollé par écran.
//!
//! Un changement de vue (navigation) devient une transition logique : l'appelant
//! enveloppe son changement d'état dans [`naviguer`] (fondu doux) ou
//! [`naviguer_sens`] (glissement directionnel, façon navigation iOS). Le navigateur
//! anime via la View Transitions API ; le visuel est fourni par les règles
//! `::view-transition-*` de [`crate::views::style`] — partagé par toutes les apps.

/// Sens d'une transition directionnelle : on entre dans un écran, ou on en revient.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sens {
    /// On avance (vers un sous-écran) : la nouvelle vue glisse depuis la droite.
    Avant,
    /// On revient : la vue glisse depuis la gauche.
    Arriere,
}

impl Sens {
    /// N'est appelée que dans le chemin wasm ; en natif elle serait « unused ».
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn attribut(self) -> &'static str {
        match self {
            Sens::Avant => "avant",
            Sens::Arriere => "arriere",
        }
    }
}

/// Transition neutre (fondu doux) autour d'une navigation.
pub fn naviguer(redessiner: impl FnOnce() + 'static) {
    appliquer(redessiner, None);
}

/// Transition **directionnelle** : le sens donne le glissement (avancer / revenir).
pub fn naviguer_sens(redessiner: impl FnOnce() + 'static, sens: Sens) {
    appliquer(redessiner, Some(sens));
}

#[allow(unused_variables)] // `sens` n'est lu que dans le chemin wasm
fn appliquer(redessiner: impl FnOnce() + 'static, sens: Option<Sens>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(racine) = doc.document_element() {
                let _ =
                    racine.set_attribute("data-sens", sens.map(Sens::attribut).unwrap_or("neutre"));
            }
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
