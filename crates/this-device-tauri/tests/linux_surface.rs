//! La surface Linux, tenue contre le dispatch et contre la carte.
//!
//! `linux::SUPPORTED` est écrite à la main, donc elle peut mentir. Deux contrôles l'en empêchent,
//! et tous deux lisent **la source** — pas de réponse d'exécution.
//!
//! Pourquoi pas d'exécution : première version de ces tests, `call` était appelé et sa réponse
//! comparée à la déclaration. Mesuré dans le job `device_linux` le 13/11/2026, c'était faux dans
//! les deux sens — sur une machine sans BlueZ, `bluetoothScan` répond `not_supported` alors
//! qu'il est servi, et `cameraCapturePhoto` répond `unsupported` alors qu'il est routé. Une
//! réponse dépend du matériel et des services de la machine ; la surface n'en dépend pas.

#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::path::Path;

use this_agent::surface::{Platform, Status, surface};
use this_device_tauri::linux::{REFUSED, STATUS_CAPABILITIES, SUPPORTED};
use this_platform::capability::Capability;
use this_platform::device_bridge::METHODS;

/// La source du backend. Une déclaration écrite à la main ne peut être confrontée qu'à ce qui
/// la fait vivre, et ce qui la fait vivre est du code.
fn source() -> Option<String> {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/linux/mod.rs")).ok()
}

/// Le corps d'une fonction : de sa signature à la première accolade en colonne 0.
fn corps<'a>(source: &'a str, signature: &str) -> Option<&'a str> {
    let debut = source.find(signature)?;
    let reste = &source[debut..];
    let fin = reste.find("\n}\n")?;
    Some(&reste[..fin])
}

/// Le nom de fil d'une capacité (`serde(rename_all = "snake_case")`).
fn wire(capability: Capability) -> String {
    serde_json::to_value(capability)
        .expect("une capacité se sérialise")
        .as_str()
        .expect("un nom de fil est une chaîne")
        .to_string()
}

/// **Tout ce que le dispatch route est soit servi, soit refusé exprès.**
///
/// Les motifs du `match` de `call`, plus le garde `m.starts_with("bluetooth")` qui tient lieu de
/// famille. Une méthode routée qui n'est ni servie ni refusée échapperait à la carte sans que
/// personne ne le sache ; une méthode servie que `call` ne route pas est une promesse vide.
#[test]
fn everything_the_dispatch_routes_is_either_served_or_refused_on_purpose() {
    let Some(source) = source() else {
        eprintln!("src/linux/mod.rs illisible : surface non vérifiée");
        return;
    };
    let Some(corps) = corps(
        &source,
        "pub async fn call(method: &str, args: &Value) -> Value {",
    ) else {
        panic!("le corps de `call` est introuvable : la forme de la fonction a changé");
    };

    let mut routees: BTreeSet<&str> = BTreeSet::new();
    let mut famille_bluetooth = false;
    for ligne in corps.lines() {
        let ligne = ligne.trim();
        let Some((motif, _)) = ligne.split_once("=>") else {
            continue;
        };
        if motif.contains("starts_with(\"bluetooth\")") {
            famille_bluetooth = true;
            continue;
        }
        for morceau in motif.split('|') {
            let morceau = morceau.trim();
            if let Some(nom) = morceau
                .strip_prefix('"')
                .and_then(|reste| reste.strip_suffix('"'))
            {
                routees.insert(nom);
            }
        }
    }
    assert!(
        famille_bluetooth,
        "le garde `starts_with(\"bluetooth\")` a disparu de `call`"
    );
    for method in METHODS
        .iter()
        .filter(|method| method.starts_with("bluetooth"))
    {
        routees.insert(method);
    }

    // `status` n'est pas une méthode de famille : il est vérifié par le test suivant, contre
    // `STATUS_CAPABILITIES`. Le mêler ici le ferait passer pour « servie et non routée ».
    let servies: BTreeSet<&str> = SUPPORTED.iter().copied().collect();
    let refusees: BTreeSet<&str> = REFUSED.iter().copied().collect();
    assert!(
        servies.is_disjoint(&refusees),
        "une méthode est à la fois servie et refusée : {:?}",
        servies.intersection(&refusees).collect::<Vec<_>>()
    );

    let mut fautes = Vec::new();
    for method in &servies {
        if !METHODS.contains(method) {
            fautes.push(format!(
                "{method} est déclarée servie et absente du contrat"
            ));
        }
        if !routees.contains(method) {
            fautes.push(format!(
                "{method} est déclarée servie et `call` ne la route pas"
            ));
        }
    }
    for method in &routees {
        if !servies.contains(method) && !refusees.contains(method) {
            fautes.push(format!(
                "{method} est routée par `call` sans être déclarée servie ni refusée : la carte \
                 ne saura pas quoi en dire"
            ));
        }
    }
    for method in &refusees {
        if !METHODS.contains(method) {
            fautes.push(format!(
                "{method} est déclarée refusée et absente du contrat"
            ));
        }
        if !routees.contains(method) {
            fautes.push(format!(
                "{method} est déclarée refusée et `call` ne la route pas"
            ));
        }
    }
    assert!(fautes.is_empty(), "\n  {}\n", fautes.join("\n  "));
}

/// **Les capacités dont `status` lit l'état sont exactement celles déclarées.**
///
/// Même raison que ci-dessus : un bureau sans caméra répond `unsupported` pour une capacité que
/// le backend sait interroger. On lit donc les bras du `match`, pas la réponse.
#[test]
fn the_status_capabilities_are_exactly_the_ones_the_match_handles() {
    let Some(source) = source() else {
        return;
    };
    let Some(corps) = corps(
        &source,
        "pub async fn status(capability: Capability) -> Reply {",
    ) else {
        panic!("le corps de `status` est introuvable");
    };
    // Les quatre capacités que ce backend peut interroger ; les autres tombent dans `_`.
    // `Capability` n'implémente pas `Ord` — on compare donc des noms de fil, pas des variantes.
    let connues = [
        (Capability::Gyroscope, "Capability::Gyroscope =>"),
        (Capability::Bluetooth, "Capability::Bluetooth =>"),
        (Capability::Camera, "Capability::Camera =>"),
        (Capability::Microphone, "Capability::Microphone =>"),
    ];
    let lues: BTreeSet<String> = connues
        .iter()
        .filter(|(_, bras)| corps.contains(*bras))
        .map(|(capability, _)| wire(*capability))
        .collect();
    let declarees: BTreeSet<String> = STATUS_CAPABILITIES
        .iter()
        .copied()
        .filter(|capability| connues.iter().any(|(connue, _)| connue == capability))
        .map(wire)
        .collect();
    assert_eq!(
        lues, declarees,
        "les capacités dont `status` lit l'état et celles déclarées diffèrent"
    );
}

/// **La carte ne promet rien que ce backend ne porte.** Une entrée qui nomme `Linux` ne peut
/// citer que des méthodes de `SUPPORTED` — et `status` que pour les capacités de
/// `STATUS_CAPABILITIES`.
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

/// Et la carte nomme bien Linux là où il porte quelque chose : un silence de plus serait aussi
/// faux qu'une promesse. Vérifié sur les entrées, pas sur le texte rendu (la description de
/// `device.camera` cite `device.camera.capture`, qui n'existe pas sur Linux).
#[test]
fn the_map_names_linux_where_this_backend_does_carry_something() {
    let linux = surface().for_platform(Platform::Linux);
    for attendu in [
        "device.bluetooth",
        "device.camera",
        "device.microphone",
        "device.gyroscope",
        "device.location.fix",
    ] {
        assert_eq!(
            linux.capability(attendu).map(|entry| entry.status),
            Some(Status::Live),
            "Linux ne nomme pas {attendu}, que son backend porte"
        );
    }
    for interdit in [
        "device.bluetooth.connect",
        "device.camera.capture",
        "device.camera.video",
        "device.location.stream",
        "device.contacts",
        "device.calendar",
    ] {
        assert_ne!(
            linux.capability(interdit).map(|entry| entry.status),
            Some(Status::Live),
            "Linux annonce {interdit}, que son backend ne porte pas"
        );
    }
}
