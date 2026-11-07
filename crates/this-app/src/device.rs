//! # Ce que la coquille peut faire de spécifique à l'appareil
//!
//! Le kit ne sait pas si l'app tourne dans un navigateur, une app iOS ou un binaire
//! desktop. Il sait **demander** : « peux-tu notifier ? », « où en est la permission
//! caméra ? », « demande-la ». Chaque coquille répond.
//!
//! Tout est pur ici : la logique testable vit dans ce crate, l'implémentation réelle
//! (API Notification du navigateur, APNs, permissions iOS) vit dans la coquille.

use serde::{Deserialize, Serialize};

/// Une capacité liée à l'appareil, pas au module. Une seule définition, dans
/// `this_platform` : deux enums identiques auraient divergé au premier ajout.
pub use this_platform::Capability;

/// Où en est une autorisation **de l'appareil**.
///
/// À ne pas confondre avec `this_descriptor::Capability` (ce que la *session* a le droit
/// de faire sur les données). Ici c'est le système qui décide, pas le module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionState {
    Granted,
    Denied,
    /// Jamais demandée : on peut demander.
    #[default]
    Prompt,
    /// La coquille ne sait pas faire : demander n'a aucun sens.
    Unsupported,
}

impl PermissionState {
    pub fn is_granted(self) -> bool {
        matches!(self, PermissionState::Granted)
    }

    /// Peut-on tenter une demande ?
    pub fn can_ask(self) -> bool {
        matches!(self, PermissionState::Prompt | PermissionState::Denied)
    }

    pub fn label(self) -> &'static str {
        match self {
            PermissionState::Granted => "accordée",
            PermissionState::Denied => "refusée",
            PermissionState::Prompt => "à demander",
            PermissionState::Unsupported => "non supportée",
        }
    }
}

/// L'intention d'un message présenté à l'utilisateur.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    Info,
    Ok,
    Warn,
    Danger,
}

/// Un message à présenter : notification système si c'est possible, toast sinon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    /// Clé de regroupement : deux avis de même clé se remplacent au lieu de s'empiler.
    pub key: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tone: Tone,
}

impl Notice {
    pub fn new(key: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            body: body.into(),
            tone: Tone::Info,
        }
    }

    pub fn with_tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

/// Ce qu'une coquille expose au kit.
///
/// Les méthodes sont synchrones : une coquille qui doit interroger l'OS le fait de son
/// côté et tient son état à jour ; le kit ne bloque jamais sur une permission.
pub trait Device {
    /// La coquille sait-elle faire ça ?
    fn supports(&self, capability: Capability) -> bool;

    fn state(&self, capability: Capability) -> PermissionState;

    /// Demande l'autorisation et **rend la réponse plus tard**.
    ///
    /// Une permission système passe par une boîte de dialogue : elle ne peut pas être
    /// synchrone. La coquille appelle `on_result` quand l'utilisateur a répondu — ou tout
    /// de suite si elle sait déjà (non supportée, déjà accordée).
    fn request(
        &mut self,
        capability: Capability,
        on_result: Box<dyn FnOnce(PermissionState) + 'static>,
    );

    /// Présente un message. Sans permission de notification, une coquille dégrade vers son
    /// propre affichage (toast) plutôt que de perdre le message.
    fn notify(&self, notice: &Notice);
}

/// La coquille par défaut : elle ne sait rien faire et le dit.
///
/// Implémenter ce trait en « tout non supporté » est le bon défaut pour un test ou un
/// rendu serveur : l'interface s'affiche sans bouton de permission qui ne mènerait nulle part.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoDevice;

impl Device for NoDevice {
    fn supports(&self, _: Capability) -> bool {
        false
    }

    fn state(&self, _: Capability) -> PermissionState {
        PermissionState::Unsupported
    }

    fn request(&mut self, _: Capability, on_result: Box<dyn FnOnce(PermissionState) + 'static>) {
        // rien à demander : on répond tout de suite que ce n'est pas supporté
        on_result(PermissionState::Unsupported);
    }

    fn notify(&self, _: &Notice) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_capability_cannot_be_asked() {
        let mut device = NoDevice;
        assert!(!device.supports(Capability::Notifications));

        // la réponse peut arriver plus tard : on la partage, on ne l'emprunte pas
        let answer = std::rc::Rc::new(std::cell::Cell::new(None));
        let slot = answer.clone();
        device.request(
            Capability::Notifications,
            Box::new(move |state| slot.set(Some(state))),
        );
        let state = answer
            .get()
            .expect("la coquille répond toujours, même pour refuser");
        assert_eq!(state, PermissionState::Unsupported);
        assert!(!state.can_ask(), "demander l'impossible n'a pas de sens");
        assert!(!state.is_granted());
    }

    #[test]
    fn only_undecided_or_denied_states_can_be_asked() {
        assert!(PermissionState::Prompt.can_ask());
        assert!(PermissionState::Denied.can_ask());
        assert!(!PermissionState::Granted.can_ask());
        assert!(!PermissionState::Unsupported.can_ask());
    }

    #[test]
    fn a_notice_defaults_to_info_and_keeps_its_tone() {
        let notice = Notice::new("save", "Enregistré", "3 lignes");
        assert_eq!(notice.tone, Tone::Info);
        assert_eq!(notice.key, "save");
        let warning = notice.clone().with_tone(Tone::Warn);
        assert_eq!(warning.tone, Tone::Warn);
    }

    #[test]
    fn the_wire_shape_is_the_contract() {
        let raw = serde_json::to_value(Notice::new("realtime", "Stock", "mis à jour")).unwrap();
        assert_eq!(raw["key"], "realtime");
        assert_eq!(raw["tone"], "info");
        assert_eq!(
            serde_json::to_value(Capability::Clipboard).unwrap(),
            serde_json::json!("clipboard")
        );
        assert_eq!(
            serde_json::to_value(PermissionState::Prompt).unwrap(),
            serde_json::json!("prompt")
        );
    }
}
