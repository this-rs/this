//! L'écran de démarrage.
//!
//! Présentation pure : il ne décide pas quand disparaître (c'est `this_app::Boot`, piloté
//! par l'app). Il raconte : les étapes franchies, celle en cours, et l'échec éventuel
//! avec un bouton pour réessayer.

use leptos::prelude::*;

/// Un rond plein : une marque neutre, que l'app peut remplacer.
const MARK: &str = r#"<svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 2 4 6.5v11L12 22l8-4.5v-11L12 2Z"/><path d="M8 12h8"/></svg>"#;

#[component]
pub fn Splash(
    #[prop(into)] title: String,
    #[prop(into)] subtitle: String,
    /// Étapes franchies, dans l'ordre. La dernière est celle en cours.
    steps: Vec<String>,
    /// Message d'échec : le splash reste alors affiché avec le bouton « Réessayer ».
    error: Option<String>,
    on_retry: Callback<()>,
) -> impl IntoView {
    let done = steps.len().saturating_sub(1);
    let current = steps.last().cloned().unwrap_or_default();
    let finished: Vec<String> = steps.iter().take(done).cloned().collect();
    let failed = error.is_some();

    view! {
        <div class="splash" role="status" aria-live="polite">
            <div class="splash-card">
                <span class="splash-mark" inner_html=MARK></span>
                <h1 class="splash-title">{title}</h1>
                <p class="splash-sub">{subtitle}</p>

                {if failed {
                    let message = error.unwrap_or_default();
                    view! {
                        <div class="splash-error">
                            <p class="splash-error-title">"Le démarrage a échoué"</p>
                            <p class="splash-error-msg">{message}</p>
                            <button class="btn primary" on:click=move |_| on_retry.run(())>
                                "Réessayer"
                            </button>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <ol class="splash-steps">
                            {finished.into_iter().map(|s| view! {
                                <li class="splash-step done"><span class="splash-dot"></span>{s}</li>
                            }).collect_view()}
                            <li class="splash-step active">
                                <span class="splash-dot pulse"></span>{current}
                            </li>
                        </ol>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
}
