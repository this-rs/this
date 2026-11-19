//! # L'habillage appliqué au document
//!
//! [`install_chrome`] réunit ce que la coquille sait (plateforme, barre de titre, plein écran,
//! modèle d'appareil — par `plugin:this-shell|env` sous Tauri) et ce que seule la webview sait (marges
//! de sécurité, taille du viewport), demande la décision à `this_app::chrome::layout`, puis
//! écrit le résultat sur `<html>` :
//!
//! - variables `--chrome-top/right/bottom/left`, `--chrome-header-min`, `--chrome-lead`,
//!   `--chrome-trail`, et `--chrome-cutout-*` quand une découpe existe ;
//! - attributs `data-platform`, `data-titlebar`, `data-fullscreen`, `data-focused`,
//!   `data-drag`, `data-cutout`, `data-orientation`.
//!
//! Et la poignée de fenêtre : [`drag_region`] rend l'attribut `data-tauri-drag-region`
//! **seulement** quand l'en-tête sert de poignée (barre intégrée, hors plein écran). L'app
//! le pose sur ses zones vides d'en-tête ; elle n'écrit aucun cas de plateforme.
//!
//! Recalculé à chaque redimensionnement, rotation, entrée ou sortie du plein écran, et à
//! chaque `chrome://changed` de la coquille.

use leptos::prelude::*;
use this_app::chrome::{ChromeEnv, ChromeLayout, Insets, Platform, Size, WindowState, layout};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

const PROBE_ID: &str = "chrome-safe-area-probe";
const CHANGED: &str = "shell://event";
/// Commande de la coquille `this-shell` : l'habillage de la fenêtre appelante.
const ENV_COMMAND: &str = "plugin:this-shell|env";

fn get(target: &JsValue, key: &str) -> Option<JsValue> {
    js_sys::Reflect::get(target, &JsValue::from_str(key))
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
}

fn tauri() -> Option<JsValue> {
    get(web_sys::window()?.as_ref(), "__TAURI__")
}

/// Sans coquille native : la plateforme se lit dans l'agent utilisateur. Sur iPhone, c'est
/// ce qui permet à une page installée de déduire sa découpe depuis la marge du haut.
fn web_env() -> ChromeEnv {
    let ua = web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .unwrap_or_default()
        .to_lowercase();
    let platform = if ua.contains("iphone") || ua.contains("ipad") {
        Platform::Ios
    } else if ua.contains("android") {
        Platform::Android
    } else {
        Platform::Web
    };
    ChromeEnv {
        platform,
        window: WindowState {
            fullscreen: false,
            maximized: false,
            focused: true,
        },
        ..Default::default()
    }
}

/// Une sonde invisible dont le padding vaut `env(safe-area-inset-*)` : c'est la seule
/// façon de lire ces valeurs depuis le code, et elle reste dans le kit, pas dans l'app.
fn probe(doc: &web_sys::Document) -> Option<web_sys::Element> {
    if let Some(el) = doc.get_element_by_id(PROBE_ID) {
        return Some(el);
    }
    let el = doc.create_element("div").ok()?;
    el.set_id(PROBE_ID);
    let _ = el.set_attribute(
        "style",
        "position:fixed;inset:0 auto auto 0;width:0;height:0;visibility:hidden;\
         pointer-events:none;padding:env(safe-area-inset-top,0px) \
         env(safe-area-inset-right,0px) env(safe-area-inset-bottom,0px) \
         env(safe-area-inset-left,0px)",
    );
    let _ = el.set_attribute("aria-hidden", "true");
    doc.body()?.append_child(&el).ok()?;
    Some(el)
}

fn measured_safe_area() -> Insets {
    let Some(win) = web_sys::window() else {
        return Insets::ZERO;
    };
    let Some(el) = win.document().and_then(|d| probe(&d)) else {
        return Insets::ZERO;
    };
    let Ok(Some(style)) = win.get_computed_style(&el) else {
        return Insets::ZERO;
    };
    let px = |prop: &str| {
        style
            .get_property_value(prop)
            .ok()
            .and_then(|v| v.trim_end_matches("px").parse::<f32>().ok())
            .unwrap_or(0.0)
    };
    Insets::new(
        px("padding-top"),
        px("padding-right"),
        px("padding-bottom"),
        px("padding-left"),
    )
}

fn viewport() -> Size {
    let Some(win) = web_sys::window() else {
        return Size::default();
    };
    let dim = |v: Result<JsValue, JsValue>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    Size {
        width: dim(win.inner_width()),
        height: dim(win.inner_height()),
    }
}

/// Le plein écran **du navigateur** (Fullscreen API ou `display-mode: fullscreen`).
/// Sous Tauri c'est la coquille qui le dit.
fn web_fullscreen() -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };
    let element = win
        .document()
        .and_then(|d| get(d.as_ref(), "fullscreenElement"))
        .is_some();
    let mode = win
        .match_media("(display-mode: fullscreen)")
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(false);
    element || mode
}

fn apply(l: &ChromeLayout) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(root) = doc
        .document_element()
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let style = root.style();
    for name in this_app::chrome::CUTOUT_VARS {
        let _ = style.remove_property(name);
    }
    for (name, value) in l.css_vars() {
        let _ = style.set_property(name, &value);
    }
    for (name, value) in l.attributes() {
        let _ = root.set_attribute(name, value);
    }

    // Les contrôles viennent d'être montrés ou cachés par l'attribut ci-dessus :
    // c'est le seul moment où leur boîte est celle qui sera vue.
    publish_controls();
}

/// La valeur de `data-tauri-drag-region` pour une zone d'en-tête : présente quand l'en-tête
/// est la poignée de la fenêtre, absente sinon (barre native, plein écran, mobile, web).
///
/// ```ignore
/// <header class="topbar" data-tauri-drag-region=ui_shell::chrome::drag_region(chrome)>
/// ```
/// Tauri ne déplace la fenêtre que si la cible du clic porte l'attribut : le poser sur les
/// zones vides (l'en-tête, le fil d'Ariane), jamais sur un bouton.
pub fn drag_region(chrome: RwSignal<ChromeLayout>) -> impl Fn() -> Option<&'static str> + Copy {
    move || chrome.get().drag_region.then_some("")
}

/// Recalcule depuis la dernière base connue et l'état mesuré à l'instant.
fn refresh(base: RwSignal<ChromeEnv>, out: RwSignal<ChromeLayout>, native: bool) {
    let mut env = base.get_untracked();
    env.safe = measured_safe_area();
    env.viewport = viewport();
    if !native {
        env.window.fullscreen = web_fullscreen();
    }
    let l = layout(&env);
    apply(&l);
    out.set(l);
}

fn on(target: &web_sys::EventTarget, event: &str, f: impl Fn() + 'static) {
    let cb = Closure::<dyn Fn()>::new(f);
    let _ = target.add_event_listener_with_callback(event, cb.as_ref().unchecked_ref());
    cb.forget();
}

fn decode(value: &JsValue) -> Option<ChromeEnv> {
    let text = js_sys::JSON::stringify(value).ok()?.as_string()?;
    serde_json::from_str(&text).ok()
}

/// Le premier clic qui active la fenêtre n'agit pas — sauf sur les trois points.
///
/// `acceptFirstMouse` est une propriété de la **fenêtre entière** : l'activer
/// fait agir le premier clic partout, y compris sur une ligne de tableau qu'on
/// voulait seulement ramener au premier plan. macOS réserve ce privilège à ses
/// contrôles ; on le restreint ici à la boîte des feux.
///
/// Le mécanisme : à l'activation on marque le document, et le premier clic venu
/// d'ailleurs est avalé — sans être annulé pour les boutons de fenêtre, qui
/// doivent agir dans ce clic-là.
fn guard_first_click(root: web_sys::Element, win: web_sys::Window) {
    use wasm_bindgen::{JsCast, closure::Closure};

    const MARK: &str = "data-first-click";

    // À l'activation : la fenêtre vient de prendre le focus, le prochain clic
    // est donc « le premier ». La marque s'efface d'elle-même après 600 ms pour
    // ne pas avaler un clic tardif.
    {
        let root = root.clone();
        let win_for_timer = win.clone();
        let on_focus = Closure::<dyn FnMut()>::new(move || {
            let _ = root.set_attribute(MARK, "1");
            let root = root.clone();
            let clear = Closure::<dyn FnMut()>::new(move || {
                let _ = root.remove_attribute(MARK);
            });
            let _ = win_for_timer.set_timeout_with_callback_and_timeout_and_arguments_0(
                clear.as_ref().unchecked_ref(),
                600,
            );
            clear.forget();
        });
        let _ = win.add_event_listener_with_callback("focus", on_focus.as_ref().unchecked_ref());
        on_focus.forget();
    }

    // Au clic, en phase de capture : on décide avant que l'app ne voie l'événement.
    let handler = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        if root.get_attribute(MARK).is_none() {
            return;
        }
        let _ = root.remove_attribute(MARK);

        let inside_controls = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|element| element.closest(".win-controls").ok().flatten())
            .is_some();

        if !inside_controls {
            event.prevent_default();
            event.stop_propagation();
        }
    });

    let _ = win.document().and_then(|doc| {
        doc.add_event_listener_with_callback_and_bool(
            "click",
            handler.as_ref().unchecked_ref(),
            true, // capture
        )
        .ok()
    });
    handler.forget();
}

/// Dit à la coquille où sont les contrôles, tels qu'ils sont dessinés.
///
/// La géométrie vivait à trois endroits : la constante du modèle, les variables
/// CSS et une boîte recopiée à la main dans le Rust de la coquille. Changer l'un
/// faisait diverger les autres en silence. Le DOM sait ce qui est réellement
/// dessiné — c'est donc lui qui fait autorité, et la coquille reçoit la boîte au
/// lieu de la deviner.
fn publish_controls() {
    use wasm_bindgen::{JsCast, JsValue};

    let Some(win) = web_sys::window() else {
        return;
    };
    let Some(document) = win.document() else {
        return;
    };
    let Ok(Some(element)) = document.query_selector(".win-controls") else {
        return;
    };
    let Ok(tauri) = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("__TAURI__")) else {
        return;
    };
    let Ok(api) = js_sys::Reflect::get(&tauri, &JsValue::from_str("event")) else {
        return;
    };
    let Ok(emit) = js_sys::Reflect::get(&api, &JsValue::from_str("emit")) else {
        return;
    };
    let Ok(emit) = emit.dyn_into::<js_sys::Function>() else {
        return;
    };

    let rect = element.get_bounding_client_rect();
    let payload = js_sys::Object::new();
    for (key, value) in [
        ("x", rect.x()),
        ("y", rect.y()),
        ("w", rect.width()),
        ("h", rect.height()),
    ] {
        let _ = js_sys::Reflect::set(&payload, &JsValue::from_str(key), &JsValue::from_f64(value));
    }

    let name = JsValue::from_str("shell://controls");
    let _ = emit.call2(&api, &name, &payload);
}

/// Le survol des feux, rapporté par la coquille.
///
/// WebKit n'installe ses zones de suivi que pour la fenêtre active : une fenêtre
/// en arrière-plan ne produit aucun `:hover`. La coquille relève donc la position
/// de la souris et nous dit quand elle entre dans la boîte des feux — c'est la
/// seule façon d'avoir le comportement de macOS, où les feux d'une fenêtre
/// inactive se rallument au survol.
fn listen_pointer(root: web_sys::Element) {
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};

    let Ok(tauri) = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("__TAURI__")) else {
        return;
    };
    let Ok(api) = js_sys::Reflect::get(&tauri, &JsValue::from_str("event")) else {
        return;
    };
    let Ok(listen) = js_sys::Reflect::get(&api, &JsValue::from_str("listen")) else {
        return;
    };
    let Ok(listen) = listen.dyn_into::<js_sys::Function>() else {
        return;
    };

    let mark = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let payload = js_sys::Reflect::get(&event, &JsValue::from_str("payload"))
            .unwrap_or_else(|_| event.clone());
        let inside = payload.as_bool().unwrap_or(false);
        let _ = root.set_attribute("data-pointer", if inside { "in" } else { "out" });
    });

    let name = JsValue::from_str("shell://pointer");
    let _ = listen.call2(&api, &name, mark.as_ref().unchecked_ref());
    mark.forget();
}

/// L'activation de la fenêtre telle que la coquille la connaît.
///
/// `onFocusChanged` de Tauri vient du système (AppKit sous macOS) : c'est la
/// seule source qui sache qu'une fenêtre a perdu le focus *sans* clic. Sans
/// elle, les feux restent colorés alors que l'app est en arrière-plan, et le
/// survol ne rend rien puisqu'il n'y a rien à rendre.
fn watch_activation(root: web_sys::Element) {
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};

    let Ok(tauri) = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("__TAURI__")) else {
        return;
    };
    let Ok(api) = js_sys::Reflect::get(&tauri, &JsValue::from_str("window")) else {
        return;
    };
    let Ok(getter) = js_sys::Reflect::get(&api, &JsValue::from_str("getCurrentWindow")) else {
        return;
    };
    let Ok(getter) = getter.dyn_into::<js_sys::Function>() else {
        return;
    };
    let Ok(current) = getter.call0(&api) else {
        return;
    };
    let Ok(listen) = js_sys::Reflect::get(&current, &JsValue::from_str("onFocusChanged")) else {
        return;
    };
    let Ok(listen) = listen.dyn_into::<js_sys::Function>() else {
        return;
    };

    let mark = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        // Tauri 2 enveloppe la charge utile : { event, id, payload }.
        let payload = js_sys::Reflect::get(&event, &JsValue::from_str("payload"))
            .unwrap_or_else(|_| event.clone());
        let active = payload.as_bool().unwrap_or(true);
        let _ = root.set_attribute("data-focused", if active { "true" } else { "false" });
    });

    let _ = listen.call1(&current, mark.as_ref().unchecked_ref());
    // Le rappel vit aussi longtemps que la page : c'est le prix d'une écoute
    // qu'on ne peut pas désabonner sans garder la poignée.
    mark.forget();
}

/// Installe l'habillage et rend un signal qu'un composant peut lire (placer un élément
/// à côté de l'île, masquer un bouton en plein écran…).
///
/// À appeler une fois, au démarrage. Le signal est aussi fourni en contexte
/// ([`use_chrome`]) pour les composants qui en ont besoin sans le recevoir en paramètre.
pub fn install_chrome() -> RwSignal<ChromeLayout> {
    let native = tauri().is_some();
    let base = RwSignal::new(web_env());
    let out = RwSignal::new(layout(&base.get_untracked()));
    provide_context(out);
    refresh(base, out, native);

    if let Some(win) = web_sys::window() {
        let w: &web_sys::EventTarget = win.as_ref();
        on(w, "resize", move || {
            refresh(base, out, native);
            publish_controls();
        });
        on(w, "orientationchange", move || refresh(base, out, native));

        // L'activation de la fenêtre : macOS grise les feux quand elle n'est plus
        // active. Le DOM ne le dit pas de façon fiable dans une webview — `blur`
        // ne part pas toujours quand la fenêtre perd le focus — donc on écoute
        // les deux : `focus`/`blur` pour le web, et `onFocusChanged` de Tauri,
        // qui vient du système, pour la coquille native. Le second gagne.
        if let Some(root) = win.document().and_then(|doc| doc.document_element()) {
            let mark = {
                let root = root.clone();
                move |active: bool| {
                    let _ =
                        root.set_attribute("data-focused", if active { "true" } else { "false" });
                }
            };
            mark(true);
            let on_focus = mark.clone();
            on(w, "focus", move || on_focus(true));
            on(w, "blur", move || mark(false));
            publish_controls();
            watch_activation(root.clone());
            listen_pointer(root.clone());
            guard_first_click(root, win.clone());
        }
        if let Some(doc) = win.document() {
            on(doc.as_ref(), "fullscreenchange", move || {
                refresh(base, out, native)
            });
        }
    }

    if let Some(t) = tauri() {
        // L'état initial, puis chaque changement annoncé par la coquille.
        if let Some(invoke) = get(&t, "core")
            .and_then(|c| get(&c, "invoke").map(|f| (c, f)))
            .and_then(|(c, f)| f.dyn_into::<js_sys::Function>().ok().map(|f| (c, f)))
        {
            let (core, invoke) = invoke;
            if let Ok(promise) = invoke.call1(&core, &JsValue::from_str(ENV_COMMAND)) {
                if let Ok(promise) = promise.dyn_into::<js_sys::Promise>() {
                    leptos::task::spawn_local(async move {
                        if let Ok(value) = wasm_bindgen_futures::JsFuture::from(promise).await {
                            if let Some(env) = decode(&value) {
                                base.set(env);
                                refresh(base, out, native);
                            }
                        }
                    });
                }
            }
        }
        if let Some((event, listen)) = get(&t, "event").and_then(|e| {
            get(&e, "listen")
                .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
                .map(|f| (e, f))
        }) {
            let handler = Closure::<dyn Fn(JsValue)>::new(move |evt: JsValue| {
                // `shell://event` porte des `ShellEvent` : on ne garde que l'habillage.
                let env = get(&evt, "payload")
                    .and_then(|p| get(&p, "env"))
                    .and_then(|e| decode(&e));
                if let Some(env) = env {
                    base.set(env);
                    refresh(base, out, native);
                }
            });
            let _ = listen.call2(&event, &JsValue::from_str(CHANGED), handler.as_ref());
            handler.forget();
        }
    }

    out
}

/// L'habillage installé par [`install_chrome`], depuis n'importe quel composant.
pub fn use_chrome() -> Option<RwSignal<ChromeLayout>> {
    use_context::<RwSignal<ChromeLayout>>()
}
