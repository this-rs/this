//! # `Node::Html` : la page que le module livre, dans une iframe sandboxée
//!
//! C'est la porte illimitée de l'interface. Les briques disent ce qu'elles montrent ; une
//! page HTML écrit tout le reste — et sort du contrat : plus de thème imposé, plus de
//! contraste vérifié, plus de cible de 44 px. L'hôte ne garde que deux choses : l'**isolement**
//! et le **pont**.
//!
//! ## L'isolement
//!
//! `<iframe sandbox="allow-scripts" src="blob:…">`, et JAMAIS `allow-same-origin` : une
//! iframe qui garde l'origine de l'hôte lirait `window.parent.__TAURI__` et appellerait les
//! commandes natives. Sans elle, l'origine est opaque. [`SANDBOX`] est l'unique source de
//! l'attribut ; un test refuse tout fichier qui écrirait ce mot ailleurs que dans ses tests.
//!
//! ## Le pont (explicite, dans les deux sens)
//!
//! - hôte → page : `{type:"this:data", rows:{entité:[…]}, theme, locale}` — uniquement les
//!   entités que la vue déclare (celles que ses autres nœuds lisent) ;
//! - page → hôte : `{type:"this:intent", kind, action, id?, values?}`, validé par
//!   [`intent_from_message`] : le genre est l'un de create/update/delete/invoke/navigate,
//!   l'action est DÉCLARÉE par le module et de ce genre. Le reste est refusé : l'appelant
//!   journalise et n'exécute rien ;
//! - page → hôte : `{type:"this:height", px}` règle la hauteur (bornée à [`MAX_HEIGHT`]).

use std::collections::BTreeSet;

use leptos::prelude::*;
use serde_json::{Map, Value, json};
use this_descriptor::ModuleDescriptor;
use this_descriptor::views::{Node, html_resolve};

use crate::views::data::Snapshot;
use crate::views::intent::{FieldValue, Intent, Values};
use crate::views::render::{self, Ctx};

/// Les droits de l'iframe : des scripts, rien d'autre. Surtout pas l'origine de l'hôte.
pub const SANDBOX: &str = "allow-scripts";

/// La hauteur maximale d'une iframe, en px.
pub const MAX_HEIGHT: u32 = this_descriptor::views::HTML_MAX_HEIGHT;

/// La hauteur avant que la page n'ait dit la sienne (`this:height`).
pub const DEFAULT_HEIGHT: u32 = 320;

/// Au-delà, le fichier n'est pas monté : un Blob de cette taille vit en mémoire le temps de
/// la vue.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// La lecture d'un fichier : les octets, ou rien s'il n'existe pas.
pub type ReadFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<u8>>>>>;

/// Ce que l'hôte fournit (par le contexte Leptos) pour monter une page : lire un fichier du
/// VFS, dire le thème et la langue. Sans lui, le nœud `html` dit pourquoi il est vide.
#[derive(Clone, Copy)]
pub struct HtmlHost {
    /// Lit les octets d'un fichier du VFS (chemin complet `/modules/<nom>/<version>/…`).
    /// Asynchrone : le VFS d'une app l'est.
    pub read: Callback<String, ReadFuture>,
    /// Le thème courant, tel que l'hôte le nomme (`light`, `dark`).
    pub theme: Signal<String>,
    /// La langue courante (`fr`, `en`).
    pub locale: Signal<String>,
}

/// La hauteur d'une iframe : celle déclarée, bornée à `1..=MAX_HEIGHT`, sinon la hauteur par
/// défaut.
pub fn height_px(declared: Option<u32>) -> u32 {
    declared.unwrap_or(DEFAULT_HEIGHT).clamp(1, MAX_HEIGHT)
}

/// Le type de contenu d'un fichier servi, d'après son extension.
pub fn content_type(path: &str) -> &'static str {
    if path.to_ascii_lowercase().ends_with(".svg") {
        "image/svg+xml"
    } else {
        "text/html;charset=utf-8"
    }
}

/// Les entités que l'arbre lit : ce que `this:data` peut porter.
pub fn declared_entities(node: &Node) -> BTreeSet<String> {
    fn walk(node: &Node, out: &mut BTreeSet<String>) {
        match node {
            Node::Leaf { brick } => {
                if let Some(e) = brick.entity() {
                    out.insert(e.to_owned());
                }
            }
            Node::Stack { children }
            | Node::Row { children }
            | Node::Drawer { children, .. }
            | Node::Modal { children, .. } => children.iter().for_each(|c| walk(c, out)),
            Node::Tabs { tabs } => tabs
                .iter()
                .flat_map(|t| t.children.iter())
                .for_each(|c| walk(c, out)),
            Node::When { entity, child, .. } => {
                out.insert(entity.clone());
                walk(child, out);
            }
            Node::Html { .. } => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(node, &mut out);
    out
}

/// Le message hôte → page : les lignes des entités déclarées, le thème, la langue.
pub fn data_message(
    snapshot: &Snapshot,
    entities: &BTreeSet<String>,
    theme: &str,
    locale: &str,
) -> Value {
    let mut rows = Map::new();
    for entity in entities {
        if let Some(r) = snapshot.rows(entity) {
            rows.insert(entity.clone(), Value::Array(r.to_vec()));
        }
    }
    json!({"type": "this:data", "rows": rows, "theme": theme, "locale": locale})
}

/// Les genres d'intention qu'une page a le droit d'émettre.
pub const INTENT_KINDS: [&str; 5] = ["create", "update", "delete", "invoke", "navigate"];

/// Valide un message page → hôte et en tire l'intention, ou dit pourquoi non.
///
/// Rien n'est exécuté ici : `Ok` rend une [`Intent`] que l'hôte traite comme n'importe
/// quelle autre ; `Err` est une ligne de journal. Les secrets, capacités d'appareil, surfaces
/// de l'hôte et outils d'agent ne sont pas atteignables : ce ne sont pas des genres admis.
pub fn intent_from_message(module: &ModuleDescriptor, message: &Value) -> Result<Intent, String> {
    let kind = message
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("intention sans `kind`")?;
    if !INTENT_KINDS.contains(&kind) {
        return Err(format!("genre d'intention inconnu `{kind}`"));
    }
    let name = message
        .get("action")
        .and_then(Value::as_str)
        .ok_or("intention sans `action`")?;
    let declared = module
        .actions
        .iter()
        .find(|a| a.name() == name)
        .cloned()
        .ok_or_else(|| format!("action `{name}` non déclarée par le module"))?;
    let id = message.get("id").and_then(Value::as_str).map(str::to_owned);
    let mut values: Values = Vec::new();
    if let Some(Value::Object(map)) = message.get("values") {
        for (field, value) in map {
            let v = match value {
                Value::String(s) => FieldValue::Text(s.clone()),
                Value::Bool(b) => FieldValue::Flag(*b),
                Value::Null => FieldValue::Empty,
                other => FieldValue::Text(other.to_string()),
            };
            values.push((field.clone(), v));
        }
    }
    let intent = render::intent_of(Some(declared), name, None, id, values);
    if intent.kind() != kind {
        return Err(format!(
            "`{name}` est une action `{}`, la page l'a émise comme `{kind}`",
            intent.kind()
        ));
    }
    Ok(intent)
}

/// Le nœud `html` : monte la page, ou dit pourquoi il ne le fait pas.
pub fn render_html(ctx: Ctx, path: &str, height: Option<u32>) -> AnyView {
    let full = ctx
        .module
        .try_with_value(|m| html_resolve(&m.name, &m.version, path))
        .unwrap_or_else(|| Err(("html_path_empty", "module indisponible".into())));
    let full = match full {
        Ok(full) => full,
        Err((reason, why)) => return render::fallback_public(&format!("html {reason} : {why}")),
    };
    let Some(host) = use_context::<HtmlHost>() else {
        return render::fallback_public("html : l'hôte ne livre pas les fichiers du module");
    };
    mount(ctx, host, full, height_px(height))
}

#[cfg(not(target_arch = "wasm32"))]
fn mount(_ctx: Ctx, _host: HtmlHost, full: String, _height: u32) -> AnyView {
    render::fallback_public(&format!("html : {full} ne se monte que dans le navigateur"))
}

#[cfg(target_arch = "wasm32")]
fn mount(ctx: Ctx, host: HtmlHost, full: String, height: u32) -> AnyView {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    // L'état du montage : rien (lecture en cours), l'URL du Blob, ou la raison d'un refus.
    let state = RwSignal::new(None::<Result<String, String>>);
    let created = StoredValue::new_local(None::<String>);
    {
        let full = full.clone();
        let read = host.read.run(full.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let verdict = match read.await {
                None => Err(format!("fichier introuvable ({full})")),
                Some(bytes) if bytes.len() > MAX_BYTES => Err(format!(
                    "{full} fait {} octets, la limite est {MAX_BYTES}",
                    bytes.len()
                )),
                Some(bytes) => {
                    let parts = js_sys::Array::new();
                    parts.push(&js_sys::Uint8Array::from(bytes.as_slice()));
                    let opts = web_sys::BlobPropertyBag::new();
                    opts.set_type(content_type(&full));
                    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
                        .and_then(|b| web_sys::Url::create_object_url_with_blob(&b))
                        .map_err(|_| "impossible de créer le Blob".to_owned())
                }
            };
            if let Ok(url) = &verdict {
                // `try_set` rend `Some(valeur)` quand le signal est déjà démonté : personne
                // ne révoquera cette URL, on le fait ici.
                match state.try_set(Some(verdict.clone())) {
                    Some(_) => {
                        let _ = web_sys::Url::revoke_object_url(url);
                    }
                    None => created.set_value(Some(url.clone())),
                }
            } else {
                let _ = state.try_set(Some(verdict));
            }
        });
    }
    // Révoqué au démontage : pas de fuite.
    on_cleanup(move || {
        if let Some(Some(url)) = created.try_get_value() {
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    });

    let frame = NodeRef::<leptos::html::Iframe>::new();
    let entities = ctx
        .view
        .try_with_value(|view| {
            ctx.module.try_with_value(|m| {
                m.views
                    .iter()
                    .find(|v| &v.name == view)
                    .map(|v| declared_entities(&v.root))
                    .unwrap_or_default()
            })
        })
        .flatten()
        .unwrap_or_default();
    let entities = StoredValue::new(entities);
    let tall = RwSignal::new(height);

    let post = move || {
        let Some(el) = frame.get() else { return };
        let Some(win) = el.content_window() else {
            return;
        };
        let msg = ctx.data.with(|s| {
            entities.with_value(|e| data_message(s, e, &host.theme.get(), &host.locale.get()))
        });
        if let Ok(js) = js_sys::JSON::parse(&msg.to_string()) {
            let _ = win.post_message(&js, "*");
        }
    };
    // Rejoue à chaque changement de données, de thème ou de langue.
    let post_again = post.clone();
    Effect::new(move |_| {
        ctx.data.track();
        host.theme.track();
        host.locale.track();
        post_again();
    });

    // Le pont retour. On ne croit que la fenêtre de CETTE iframe.
    let listener =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            let Some(el) = frame.get_untracked() else {
                return;
            };
            let Some(win) = el.content_window() else {
                return;
            };
            let from_us = ev
                .source()
                .map(|s| js_sys::Object::is(&s, win.as_ref()))
                .unwrap_or(false);
            if !from_us {
                return;
            }
            let Some(text) = js_sys::JSON::stringify(&ev.data())
                .ok()
                .and_then(|s| s.as_string())
            else {
                return;
            };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else {
                return;
            };
            match msg.get("type").and_then(Value::as_str) {
                Some("this:intent") => {
                    match ctx.module.try_with_value(|m| intent_from_message(m, &msg)) {
                        Some(Ok(intent)) => ctx.on_intent.run(intent),
                        Some(Err(why)) => {
                            render::log_refusal(&format!("html : intention ignorée ({why})"))
                        }
                        None => {}
                    }
                }
                Some("this:height") => {
                    if let Some(px) = msg.get("px").and_then(Value::as_u64) {
                        tall.set(height_px(Some(px.min(u64::from(MAX_HEIGHT)) as u32)));
                    }
                }
                Some("this:ready") => post(),
                other => render::log_refusal(&format!("html : message ignoré (type {other:?})")),
            }
        });
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("message", listener.as_ref().unchecked_ref());
        let target = window.clone();
        let cb: js_sys::Function = listener
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone();
        on_cleanup(move || {
            let _ = target.remove_event_listener_with_callback("message", &cb);
        });
    }
    listener.forget();

    view! {
        {move || match state.get() {
            None => view! { <div class="vw-html-loading" role="status">"Chargement…"</div> }.into_any(),
            Some(Err(why)) => render::fallback_public(&format!("html : {why}")),
            Some(Ok(url)) => view! {
                <iframe
                    node_ref=frame
                    class="vw-html"
                    sandbox=SANDBOX
                    src=url
                    title=full.clone()
                    on:load=move |_| post()
                    style=move || format!("width:100%;border:0;height:{}px", tall.get())
                ></iframe>
            }
            .into_any(),
        }}
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::data::Feed;
    use this_descriptor::views::Action;

    fn module() -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("compteur", "1.0.0");
        m.actions.push(
            serde_json::from_value::<Action>(
                json!({"kind":"invoke","name":"incrementer","operation":"inc"}),
            )
            .expect("action"),
        );
        m.actions.push(
            serde_json::from_value::<Action>(
                json!({"kind":"request_secret","name":"cle","secret":"k"}),
            )
            .expect("action"),
        );
        m
    }

    #[test]
    fn the_sandbox_never_carries_the_host_origin() {
        assert_eq!(SANDBOX, "allow-scripts");
        assert!(!SANDBOX.contains("allow-same-origin"));
        // Le mot ne doit figurer, dans le code de montage, nulle part hors des tests et des
        // commentaires : on lit ce fichier-ci avant son module de tests.
        let source = include_str!("html.rs");
        let code = source.split("#[cfg(test)]").next().expect("code");
        for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
            assert!(
                !line.contains("allow-same-origin"),
                "allow-same-origin dans le code de montage : {line}"
            );
        }
    }

    #[test]
    fn height_is_bounded() {
        assert_eq!(height_px(None), DEFAULT_HEIGHT);
        assert_eq!(height_px(Some(0)), 1);
        assert_eq!(height_px(Some(99_999)), 2000);
        assert_eq!(height_px(Some(480)), 480);
    }

    #[test]
    fn a_declared_invoke_intent_is_accepted() {
        let i = intent_from_message(
            &module(),
            &json!({"type":"this:intent","kind":"invoke","action":"incrementer","values":{"n":"1"}}),
        )
        .expect("accepté");
        assert_eq!(i.kind(), "invoke");
        assert_eq!(i.action(), Some("incrementer"));
    }

    #[test]
    fn an_unknown_or_unreachable_intent_is_refused_with_its_reason() {
        let m = module();
        for (msg, why) in [
            (json!({"kind":"invoke","action":"inconnue"}), "non déclarée"),
            (json!({"kind":"rm_rf","action":"incrementer"}), "genre"),
            (json!({"kind":"invoke"}), "sans `action`"),
            (json!({"action":"incrementer"}), "sans `kind`"),
            // une action existante, mais pas du genre annoncé
            (json!({"kind":"delete","action":"incrementer"}), "invoke"),
            // un secret n'est pas atteignable par une page
            (json!({"kind":"invoke","action":"cle"}), "`secret`"),
            (json!({"kind":"secret","action":"cle"}), "genre"),
        ] {
            let err = intent_from_message(&m, &msg).expect_err(&msg.to_string());
            assert!(err.contains(why), "{msg} → {err}");
        }
    }

    #[test]
    fn data_carries_only_the_declared_entities() {
        let snap = Snapshot::new()
            .with("compte", Feed::Ready(vec![json!({"id":"1"})]))
            .with("secret", Feed::Ready(vec![json!({"id":"x"})]));
        let mut only = BTreeSet::new();
        only.insert("compte".to_owned());
        let msg = data_message(&snap, &only, "dark", "fr");
        assert_eq!(msg["type"], "this:data");
        assert_eq!(msg["theme"], "dark");
        assert_eq!(msg["locale"], "fr");
        assert!(msg["rows"].get("compte").is_some());
        assert!(msg["rows"].get("secret").is_none());
    }

    #[test]
    fn content_type_follows_the_extension() {
        assert_eq!(content_type("a/b.svg"), "image/svg+xml");
        assert!(content_type("a/b.html").starts_with("text/html"));
    }
}
