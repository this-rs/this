//! La surface Linux, tenue contre le dispatch et contre la carte.
//!
//! `linux::SUPPORTED` est écrite à la main, donc elle peut mentir de deux façons : annoncer une
//! méthode que `call` ne sert pas, ou taire une méthode qu'il sert. Les deux sont vérifiées ici,
//! et la seconde **en appelant** `call` — relire le `match` des yeux est exactement ce qu'on
//! fait quand on se trompe.

#![cfg(target_os = "linux")]

use serde_json::json;
use this_agent::surface::{Platform, surface};
use this_device_tauri::linux::{self, STATUS_CAPABILITIES, SUPPORTED};
use this_platform::capability::Capability;
use this_platform::device_bridge::{METHODS, complete_args};

const MAINTENANT: f64 = 1_700_000_000_000.0;

/// Ce que `call` répond pour une méthode : `not_supported` ou autre chose.
///
/// On ne regarde pas la valeur de retour, seulement le **refus** : le reste dépend des services
/// du bureau (BlueZ absent, GeoClue2 sans bus…), et un test qui exigerait une réponse utile
/// serait un test d'intégration, pas de surface.
fn refuse(method: &str) -> bool {
    let args = complete_args(method, &json!({}), MAINTENANT);
    let reply = tauri::async_runtime::block_on(linux::call(method, &args));
    reply.get("code").and_then(|code| code.as_str()) == Some("not_supported")
}

/// **Le dispatch et la déclaration disent la même chose.** Une méthode servie mais absente de
/// `SUPPORTED` est une capacité que la carte taira ; une méthode annoncée mais refusée est une
/// promesse en l'air.
#[test]
fn the_declaration_says_exactly_what_the_dispatch_serves() {
    let mut ecarts = Vec::new();
    for method in METHODS {
        // `status` n'est pas une méthode de famille : il passe par `Request::Status` et la
        // liste `STATUS_CAPABILITIES`, vérifiée juste après.
        if *method == "status" {
            continue;
        }
        let declaree = SUPPORTED.contains(method);
        let refusee = refuse(method);
        match (declaree, refusee) {
            (true, true) => {
                ecarts.push(format!("{method} est déclarée servie et `call` la refuse"))
            }
            (false, false) => ecarts.push(format!(
                "{method} est servie par `call` et absente de SUPPORTED — la carte se taira"
            )),
            _ => {}
        }
    }
    assert!(ecarts.is_empty(), "\n  {}\n", ecarts.join("\n  "));
}

/// Et `status` : lu pour les capacités déclarées, `unsupported` pour les autres. Une capacité
/// que ce backend sait interroger mais qui n'est pas listée serait tue par le pont.
#[test]
fn the_status_capabilities_are_exactly_the_ones_this_backend_can_read() {
    for capability in Capability::ALL {
        let reply = tauri::async_runtime::block_on(linux::status(capability));
        let etat = serde_json::to_value(reply).ok().and_then(|value| {
            value
                .get("state")
                .and_then(|s| s.as_str())
                .map(str::to_owned)
        });
        let lu = etat.as_deref() != Some("unsupported");
        assert_eq!(
            lu,
            STATUS_CAPABILITIES.contains(&capability),
            "{capability:?} : lue = {lu}, déclarée = {}",
            STATUS_CAPABILITIES.contains(&capability)
        );
    }
}

/// Le nom de fil d'une capacité (`serde(rename_all = "snake_case")`).
fn wire(capability: Capability) -> String {
    serde_json::to_value(capability)
        .expect("une capacité se sérialise")
        .as_str()
        .expect("un nom de fil est une chaîne")
        .to_string()
}

/// **La carte ne promet rien que ce backend ne porte.** Une entrée `device.*` qui nomme `Linux`
/// ne peut citer que des méthodes de `SUPPORTED` (ou `status`, servi par capacité).
#[test]
fn the_map_claims_nothing_on_linux_that_this_backend_does_not_serve() {
    let carte = surface();
    let mut fautes = Vec::new();
    for capability in Capability::ALL {
        let base = format!("device.{}", wire(capability));
        let prefix = format!("{base}.");
        for entry in carte
            .capabilities
            .iter()
            .filter(|entry| entry.id == base || entry.id.starts_with(&prefix))
        {
            if !entry.platforms.contains(&Platform::Linux) {
                continue;
            }
            let Some(command) = entry.command.clone() else {
                continue;
            };
            for method in command
                .split(" / ")
                .map(str::trim)
                .filter(|m| !m.is_empty())
            {
                if method == "status" {
                    if !STATUS_CAPABILITIES.contains(&capability) {
                        fautes.push(format!(
                            "{} promet `status` sur Linux pour {capability:?}, que ce backend ne lit pas",
                            entry.id
                        ));
                    }
                    continue;
                }
                if !SUPPORTED.contains(&method) {
                    fautes.push(format!(
                        "{} promet {method} sur Linux, absent de SUPPORTED",
                        entry.id
                    ));
                }
            }
        }
    }
    assert!(fautes.is_empty(), "\n  {}\n", fautes.join("\n  "));
}
