//! # Les avis à l'écran
//!
//! Rendu de la file [`this_app::Notices`] : une pile de toasts, chacun avec sa clé (donc
//! son bouton de fermeture) et son ton. Le composant ne décide **pas** quand un avis
//! disparaît — c'est l'horloge de l'application, comme partout dans le kit.

use leptos::prelude::*;
use this_app::{Notices, Tone};

const CLOSE: &str = r#"<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg>"#;

fn tone_class(tone: Tone) -> &'static str {
    match tone {
        Tone::Info => "info",
        Tone::Ok => "ok",
        Tone::Warn => "warn",
        Tone::Danger => "danger",
    }
}

#[component]
pub fn ToastHost(notices: RwSignal<Notices>, on_dismiss: Callback<String>) -> impl IntoView {
    view! {
        <div class="toasts" role="status" aria-live="polite">
            {move || {
                notices
                    .get()
                    .live()
                    .iter()
                    .map(|live| {
                        let key = live.notice.key.clone();
                        let key_for_click = key.clone();
                        view! {
                            <div class=format!("toast {}", tone_class(live.notice.tone))>
                                <div class="toast-text">
                                    <strong>{live.notice.title.clone()}</strong>
                                    <span class="small">{live.notice.body.clone()}</span>
                                </div>
                                <button class="btn icon ghost small" aria-label="Fermer l'avis"
                                    on:click=move |_| on_dismiss.run(key_for_click.clone())>
                                    <span inner_html=CLOSE></span>
                                </button>
                            </div>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
