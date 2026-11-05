//! La coquille d'un panneau latéral : voile cliquable, en-tête, corps, fermeture.
//!
//! Le contenu (champs, faits, pied d'actions) est fourni par l'app : `ui-shell` ne
//! connaît aucune entité.

use leptos::prelude::*;

const CLOSE: &str = r#"<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg>"#;

#[component]
pub fn Panel(
    #[prop(into)] eyebrow: String,
    #[prop(into)] title: String,
    on_close: Callback<()>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="overlay" on:click=move |_| on_close.run(())>
            <aside
                class="panel"
                role="dialog"
                aria-modal="true"
                aria-label=title.clone()
                on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()
            >
                <header class="panel-head">
                    <div>
                        <p class="eyebrow">{eyebrow}</p>
                        <h2>{title.clone()}</h2>
                    </div>
                    <button class="btn icon ghost" aria-label="Fermer"
                        on:click=move |_| on_close.run(())>
                        <span inner_html=CLOSE></span>
                    </button>
                </header>
                <div class="panel-body">{children()}</div>
            </aside>
        </div>
    }
}
