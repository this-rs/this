//! # La coquille web : ce qu'un navigateur ou un webview sait faire
//!
//! Implémentation de [`this_app::Device`] au-dessus des API du navigateur. Elle est
//! volontairement **honnête sur ses limites** : seules les notifications sont branchées
//! aujourd'hui ; caméra, micro, localisation et partage répondent « non supporté » plutôt
//! que de prétendre le contraire.
//!
//! Les implémentations natives (APNs, permissions iOS, feuille de partage) viendront
//! derrière le même trait, sans toucher au kit ni à l'application.

use this_app::{Capability, Device, Notice, PermissionState};
use wasm_bindgen::prelude::*;

/// L'API `Notification` du navigateur, si elle existe.
fn notification_api() -> Option<JsValue> {
    let win = web_sys::window()?;
    let api = js_sys::Reflect::get(&win, &JsValue::from_str("Notification")).ok()?;
    (!api.is_undefined() && !api.is_null()).then_some(api)
}

fn permission_string(api: &JsValue) -> String {
    js_sys::Reflect::get(api, &JsValue::from_str("permission"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "default".into())
}

fn state_of(permission: &str) -> PermissionState {
    match permission {
        "granted" => PermissionState::Granted,
        "denied" => PermissionState::Denied,
        "unsupported" => PermissionState::Unsupported,
        _ => PermissionState::Prompt,
    }
}

/// La coquille du navigateur.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrowserDevice;

impl Device for BrowserDevice {
    fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::Notifications => notification_api().is_some(),
            // pas encore branché : le dire vaut mieux que de proposer un bouton inerte
            _ => false,
        }
    }

    fn state(&self, capability: Capability) -> PermissionState {
        if !self.supports(capability) {
            return PermissionState::Unsupported;
        }
        match notification_api() {
            Some(api) => state_of(&permission_string(&api)),
            None => PermissionState::Unsupported,
        }
    }

    fn request(
        &mut self,
        capability: Capability,
        on_result: Box<dyn FnOnce(PermissionState) + 'static>,
    ) {
        if !self.supports(capability) {
            on_result(PermissionState::Unsupported);
            return;
        }
        let Some(api) = notification_api() else {
            on_result(PermissionState::Unsupported);
            return;
        };

        // déjà décidé par l'utilisateur (ou par le navigateur) : on ne redemande pas
        let current = state_of(&permission_string(&api));
        if current != PermissionState::Prompt {
            on_result(current);
            return;
        }

        let Ok(request) = js_sys::Reflect::get(&api, &JsValue::from_str("requestPermission"))
            .and_then(|value| value.dyn_into::<js_sys::Function>())
        else {
            on_result(PermissionState::Unsupported);
            return;
        };
        let Ok(promise) = request.call0(&api) else {
            on_result(PermissionState::Unsupported);
            return;
        };
        let Some(promise) = promise.dyn_ref::<js_sys::Promise>() else {
            on_result(PermissionState::Unsupported);
            return;
        };

        // la réponse arrive quand l'utilisateur a cliqué
        let handler = Closure::once(move |value: JsValue| {
            let permission = value.as_string().unwrap_or_else(|| "default".into());
            on_result(state_of(&permission));
        });
        let _ = promise.then(&handler);
        handler.forget();
    }

    fn notify(&self, notice: &Notice) {
        if !self.state(Capability::Notifications).is_granted() {
            // pas de permission : la coquille dégrade vers son propre affichage
            return;
        }
        let Some(api) = notification_api() else {
            return;
        };
        let Ok(constructor) = api.clone().dyn_into::<js_sys::Function>() else {
            return;
        };
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &options,
            &JsValue::from_str("body"),
            &JsValue::from_str(&notice.body),
        );
        let _ = constructor.call2(&api, &JsValue::from_str(&notice.title), options.as_ref());
    }
}
