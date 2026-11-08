//! Parity between the harness map and the system-state whitelist.
//!
//! `this_agent::surface::surface()` names, for each « état système » capability, a command of
//! the form `system_state (méthode)`. `this_device_tauri::SYSTEM_METHODS` is the whitelist of
//! native selectors the bridge serves without consent. Nothing ties the two at compile time:
//! a typo would announce a capability `Live` and answer it `not_supported` at runtime.

use this_agent::surface::surface;
use this_device_tauri::SYSTEM_METHODS;

/// Les six capacités d'état système, avec la méthode native que chacune nomme.
const ETAT_SYSTEME: [(&str, &str); 6] = [
    ("device_info", "deviceInfo"),
    ("battery", "batteryLevel"),
    ("network_state", "networkState"),
    ("locale", "locale"),
    ("screen_state", "screenState"),
    ("volume", "volume"),
];

#[test]
fn every_system_state_capability_names_a_whitelisted_method() {
    let carte = surface();
    for (id, attendue) in ETAT_SYSTEME {
        let entry = carte
            .capability(id)
            .unwrap_or_else(|| panic!("{id} absent de la carte"));
        let commande = entry.command.clone().unwrap_or_default();
        let methode = commande
            .split_once('(')
            .and_then(|(_, reste)| reste.strip_suffix(')'))
            .map(str::trim)
            .unwrap_or_default();
        assert_eq!(
            methode, attendue,
            "{id} : la commande « {commande} » nomme « {methode} », attendu « {attendue} »"
        );
        assert!(
            SYSTEM_METHODS.contains(&methode),
            "{id} : « {methode} » absent de SYSTEM_METHODS"
        );
    }
}
