//! # Les primitives du design system
//!
//! Chaque primitive correspond à une section de `style.css` **du même nom** : la structure
//! du code et celle de la feuille se répondent, et un composant se cherche comme il se lit.
//!
//! Les tons sont un [`Tone`] typé, pas une chaîne : `Tone::Dangr` ne compile pas, alors
//! qu'une classe `"dangre"` produisait silencieusement un badge sans couleur.

use leptos::prelude::*;

/// L'intention d'un élément coloré. La couleur ne porte jamais l'information seule : elle
/// double toujours un point et un texte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Danger,
    Info,
    Muted,
}

impl Tone {
    /// Classe CSS (`badge ok`, `dot danger`, …).
    pub fn class(self) -> &'static str {
        match self {
            Tone::Ok => "ok",
            Tone::Warn => "warn",
            Tone::Danger => "danger",
            Tone::Info => "info",
            Tone::Muted => "muted",
        }
    }
}

/// Une pastille d'état : point + texte, jamais la couleur seule.
pub fn badge(text: impl Into<String>, tone: Tone) -> AnyView {
    view! {
        <span class=format!("badge {}", tone.class())>
            <span class="dot"></span>
            {text.into()}
        </span>
    }
    .into_any()
}

/// Une jauge de 0 à `max`, dont le ton suit le niveau.
///
/// Le seuil « bas » est un paramètre : un stock de 5 n'a pas la même urgence selon l'entité.
pub fn meter(value: f64, max: f64, low: f64) -> AnyView {
    let pct = if max > 0.0 {
        (value / max * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let tone = if value <= 0.0 {
        Tone::Danger
    } else if value <= low {
        Tone::Warn
    } else {
        Tone::Ok
    };
    view! {
        <span class=format!("meter-row {}", tone.class())>
            <span class="meter">
                <span class="meter-fill" style=format!("width:{pct:.1}%")></span>
            </span>
            <span class="meter-value">{crate::model::count(value)}</span>
        </span>
    }
    .into_any()
}

/// Une tuile d'indicateur : libellé, valeur, précision.
pub fn kpi(label: &str, value: String, hint: String) -> AnyView {
    view! {
        <div class="kpi">
            <span class="kpi-label">{label.to_string()}</span>
            <strong class="kpi-value">{value}</strong>
            <span class="kpi-hint">{hint}</span>
        </div>
    }
    .into_any()
}

/// Une section vide : on dit ce qui manque, pas seulement qu'il n'y a rien.
pub fn empty(message: impl Into<String>) -> AnyView {
    view! { <p class="empty">{message.into()}</p> }.into_any()
}
