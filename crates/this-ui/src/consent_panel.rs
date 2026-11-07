//! Le panneau de consentement : les demandes en attente et les accords existants.
//!
//! Le composant est **pur d'entrée** : il reçoit des données ([`PendingItem`], [`GrantItem`])
//! et rend des callbacks. Il n'appelle aucun natif et ne touche à aucun stockage. La décision
//! appartient au noyau : la page affiche la demande et transmet la réponse de la personne,
//! rien de plus. Le parent (l'app) porte l'état et l'envoie au noyau.
//!
//! Les capacités sont des noms de fil (`snake_case`, comme dans `this_platform::device_bridge`)
//! et sont traduites en libellé par [`Capability::label`].
//!
//! Textes : pas de mécanisme i18n dans `this-ui` ; toutes les chaînes sont isolées dans le
//! module `text`, pour être faciles à traduire.

use leptos::prelude::*;
use serde_json::Value;
use this_platform::Capability;

use crate::panel::Panel;

const CHECK: &str = r#"<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>"#;

/// Une demande en attente : un module demande une capacité, la personne n'a pas encore répondu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingItem {
    /// Nom du module demandeur, tel que le noyau le connaît.
    pub module: String,
    /// Nom de fil de la capacité (ex. `"contacts"`, `"live_activity"`).
    pub capability: String,
    /// Finalité, lue dans le descripteur du module. Passée telle quelle, jamais inventée ici.
    pub purpose: String,
}

/// L'état d'un accord déjà enregistré.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantState {
    Granted,
    Denied,
}

/// Un accord existant, accordé ou refusé, pour un module et une capacité.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantItem {
    pub module: String,
    /// Nom de fil de la capacité.
    pub capability: String,
    pub state: GrantState,
}

/// Le couple sur lequel une réponse porte : un module et une capacité.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentTarget {
    pub module: String,
    pub capability: String,
}

/// Le libellé d'une capacité, à partir de son nom de fil. Un nom inconnu est affiché tel quel.
pub fn capability_label(wire: &str) -> String {
    serde_json::from_value::<Capability>(Value::String(wire.to_owned()))
        .map(|capability| capability.label().to_owned())
        .unwrap_or_else(|_| wire.to_owned())
}

/// Les demandes restantes une fois la réponse donnée à `answered` : la demande correspondante
/// disparaît, les autres restent dans l'ordre d'origine.
pub fn without_answered(pending: &[PendingItem], answered: &ConsentTarget) -> Vec<PendingItem> {
    pending
        .iter()
        .filter(|item| !(item.module == answered.module && item.capability == answered.capability))
        .cloned()
        .collect()
}

/// Les chaînes du panneau. Un seul endroit à traduire.
mod text {
    pub fn eyebrow() -> &'static str {
        "Consentement"
    }
    pub fn title() -> &'static str {
        "Accès demandés et accordés"
    }
    pub fn pending_heading() -> &'static str {
        "Demandes en attente"
    }
    pub fn no_pending() -> &'static str {
        "Aucune demande en attente."
    }
    pub fn grants_heading() -> &'static str {
        "Accords enregistrés"
    }
    pub fn no_grants() -> &'static str {
        "Aucun accord enregistré."
    }
    pub fn purpose_prefix() -> &'static str {
        "Finalité : "
    }
    pub fn granted() -> &'static str {
        "Accordé"
    }
    pub fn denied() -> &'static str {
        "Refusé"
    }
    pub fn allow(capability: &str, module: &str) -> String {
        format!("Autoriser l'accès : {capability} pour {module}")
    }
    pub fn deny(capability: &str, module: &str) -> String {
        format!("Refuser l'accès : {capability} pour {module}")
    }
    pub fn revoke(capability: &str, module: &str) -> String {
        format!("Révoquer l'accès : {capability} pour {module}")
    }
    pub fn allow_button() -> &'static str {
        "Autoriser"
    }
    pub fn deny_button() -> &'static str {
        "Refuser"
    }
    pub fn revoke_button() -> &'static str {
        "Révoquer"
    }
    pub fn close() -> &'static str {
        "Fermer"
    }
    pub fn hint() -> &'static str {
        "Votre réponse est enregistrée par l'appareil. Révoquer un accord le retire."
    }
}

#[component]
pub fn ConsentPanel(
    pending: Vec<PendingItem>,
    grants: Vec<GrantItem>,
    on_allow: Callback<ConsentTarget>,
    on_deny: Callback<ConsentTarget>,
    on_revoke: Callback<ConsentTarget>,
    on_close: Callback<()>,
) -> impl IntoView {
    let pending_list = if pending.is_empty() {
        view! { <p class="settings-hint">{text::no_pending()}</p> }.into_any()
    } else {
        view! {
            <ul class="consent-list">
                {pending.into_iter().map(|item| {
                    let label = capability_label(&item.capability);
                    let target = ConsentTarget {
                        module: item.module.clone(),
                        capability: item.capability.clone(),
                    };
                    let (for_allow, for_deny) = (target.clone(), target.clone());
                    let allow_label = text::allow(&label, &item.module);
                    let deny_label = text::deny(&label, &item.module);
                    view! {
                        <li class="consent-item">
                            <div class="consent-head">
                                <strong>{item.module.clone()}</strong>
                                <span class="badge">{label.clone()}</span>
                            </div>
                            <p class="settings-hint">{format!("{}{}", text::purpose_prefix(), item.purpose)}</p>
                            <div class="consent-actions">
                                <button class="btn" aria-label=allow_label
                                    on:click=move |_| on_allow.run(for_allow.clone())>
                                    {text::allow_button()}
                                </button>
                                <button class="btn ghost" aria-label=deny_label
                                    on:click=move |_| on_deny.run(for_deny.clone())>
                                    {text::deny_button()}
                                </button>
                            </div>
                        </li>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any()
    };

    let grants_list = if grants.is_empty() {
        view! { <p class="settings-hint">{text::no_grants()}</p> }.into_any()
    } else {
        view! {
            <ul class="consent-list">
                {grants.into_iter().map(|item| {
                    let label = capability_label(&item.capability);
                    let target = ConsentTarget {
                        module: item.module.clone(),
                        capability: item.capability.clone(),
                    };
                    let revoke_label = text::revoke(&label, &item.module);
                    let state_text = match item.state {
                        GrantState::Granted => text::granted(),
                        GrantState::Denied => text::denied(),
                    };
                    view! {
                        <li class="consent-item">
                            <div class="consent-head">
                                <strong>{item.module.clone()}</strong>
                                <span class="badge">{label.clone()}</span>
                                <span class="small">{state_text}</span>
                            </div>
                            <div class="consent-actions">
                                <button class="btn ghost" aria-label=revoke_label
                                    on:click=move |_| on_revoke.run(target.clone())>
                                    <span inner_html=CHECK aria-hidden="true"></span>
                                    {text::revoke_button()}
                                </button>
                            </div>
                        </li>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any()
    };

    view! {
        <Panel eyebrow=text::eyebrow() title=text::title() on_close=on_close>
            <section class="settings-group">
                <h3 class="settings-heading">{text::pending_heading()}</h3>
                {pending_list}
            </section>

            <section class="settings-group">
                <h3 class="settings-heading">{text::grants_heading()}</h3>
                {grants_list}
                <p class="settings-hint">{text::hint()}</p>
            </section>

            <div class="panel-foot">
                <button class="btn ghost" on:click=move |_| on_close.run(())>{text::close()}</button>
            </div>
        </Panel>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(module: &str, capability: &str) -> PendingItem {
        PendingItem {
            module: module.into(),
            capability: capability.into(),
            purpose: "Trouver un contact".into(),
        }
    }

    fn target(module: &str, capability: &str) -> ConsentTarget {
        ConsentTarget {
            module: module.into(),
            capability: capability.into(),
        }
    }

    #[test]
    fn a_known_wire_name_gets_the_catalogue_label() {
        assert_eq!(capability_label("contacts"), "Contacts");
        assert_eq!(capability_label("live_activity"), "Activité en direct");
        assert_eq!(capability_label("gyroscope"), "Gyroscope et accéléromètre");
    }

    #[test]
    fn an_unknown_wire_name_is_shown_as_is() {
        assert_eq!(capability_label("teleport"), "teleport");
    }

    #[test]
    fn a_request_disappears_once_answered() {
        let all = vec![
            pending("atelier", "contacts"),
            pending("atelier", "bluetooth"),
            pending("carnet", "contacts"),
        ];
        let rest = without_answered(&all, &target("atelier", "contacts"));
        assert_eq!(
            rest,
            vec![
                pending("atelier", "bluetooth"),
                pending("carnet", "contacts")
            ]
        );
    }

    #[test]
    fn answering_an_unknown_request_leaves_the_list_unchanged() {
        let all = vec![pending("atelier", "contacts")];
        assert_eq!(without_answered(&all, &target("autre", "contacts")), all);
    }

    #[test]
    fn labels_name_the_capability_and_the_module() {
        assert_eq!(
            text::allow("Contacts", "atelier"),
            "Autoriser l'accès : Contacts pour atelier"
        );
    }
}
