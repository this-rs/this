//! Le panneau de réglages : apparence (réglages de l'app) et accès (faits fournis).
//!
//! Les deux contrôles sont réels et persistés ; les faits d'accès sont en lecture seule,
//! parce qu'ils décrivent la surface, ils ne la changent pas.

use leptos::prelude::*;
use this_app::{Settings, Theme};

use crate::panel::Panel;

#[component]
pub fn SettingsPanel(
    settings: RwSignal<Settings>,
    /// Faits d'accès, dans l'ordre d'affichage : (libellé, valeur).
    facts: Vec<(String, String)>,
    on_close: Callback<()>,
) -> impl IntoView {
    let options = [Theme::Auto, Theme::Light, Theme::Dark];

    view! {
        <Panel eyebrow="Réglages" title="Apparence et accès" on_close=on_close>
            <section class="settings-group">
                <h3 class="settings-heading">"Thème"</h3>
                <div class="segmented" role="group" aria-label="Thème">
                    {options.into_iter().map(|option| view! {
                        <button
                            class="segment"
                            aria-pressed=move || (settings.get().theme == option).to_string()
                            on:click=move |_| settings.update(|s| s.theme = option)
                        >
                            {option.label()}
                        </button>
                    }).collect_view()}
                </div>
                <p class="settings-hint">
                    "« Automatique » suit le réglage du système ; les deux autres le forcent."
                </p>
            </section>

            <section class="settings-group">
                <h3 class="settings-heading">"Mouvement"</h3>
                <label class="switch-row">
                    <span>
                        <span class="switch-label">"Réduire les animations"</span>
                        <span class="settings-hint">"Coupe transitions et animations de l'interface."</span>
                    </span>
                    <input
                        type="checkbox"
                        prop:checked=move || settings.get().reduce_motion
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            settings.update(|s| s.reduce_motion = on);
                        }
                    />
                </label>
            </section>

            <section class="settings-group">
                <h3 class="settings-heading">"Accès"</h3>
                <dl class="facts">
                    {facts.into_iter().map(|(label, value)| view! {
                        <div class="fact">
                            <dt>{label}</dt>
                            <dd>{value}</dd>
                        </div>
                    }).collect_view()}
                </dl>
                <p class="settings-hint">
                    "Ces valeurs décrivent la surface courante : elles ne se modifient pas depuis l'interface."
                </p>
            </section>

            <div class="panel-foot">
                <button class="btn ghost" on:click=move |_| on_close.run(())>"Fermer"</button>
            </div>
        </Panel>
    }
}
