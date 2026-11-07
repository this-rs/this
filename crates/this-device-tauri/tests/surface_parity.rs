//! Parity between the harness map and the device bridge.
//!
//! `this_agent::surface::surface()` est ce que l'agent lit pour répondre « peux-tu … ? », par
//! l'outil `capacites`. `this_platform::device_bridge` est ce que la coquille sait faire. Rien
//! ne les relie à la compilation, et ils ont divergé : la carte ne nommait AUCUNE capacité
//! d'appareil, donc l'agent répondait « pas encore » pour les contacts, le calendrier ou le
//! Bluetooth, livrés depuis des semaines. La personne en a conclu que rien ne marchait.
//!
//! Trois règles, vérifiées contre les sources réelles et non contre des copies des listes :
//!
//! 1. chaque capacité du catalogue est dans la carte, sous `device.<nom de fil>`, `Live`, et
//!    nomme toutes les méthodes que le pont lui donne (`device_bridge::capability_of`) ;
//! 2. une capacité peut être plus profonde sur un appareil que sur l'autre : les entrées
//!    `device.<nom>.<aspect>` font partie de la même capacité, et c'est le préfixe qui les
//!    rattache (le Bluetooth se connecte sur iPhone, il ne fait que scanner sur Mac) ;
//! 3. ce que la carte annonce sur macOS existe dans `macos/Sources/dispatch.swift`, et une
//!    capacité qu'elle n'y annonce pas du tout est refusée par ce fichier.
//!
//! Les tests lisent les `.swift` du dépôt : sources absentes, ils ne prétendent pas passer.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;
use this_agent::surface::{Platform, Status, Surface, Tier, surface};
use this_platform::capability::Capability;
use this_platform::device_bridge::{METHODS, capability_of};

/// Le nom de fil d'une capacité (`serde(rename_all = "snake_case")`).
fn wire(capability: Capability) -> String {
    serde_json::to_value(capability)
        .expect("une capacité se sérialise")
        .as_str()
        .expect("un nom de fil est une chaîne")
        .to_string()
}

/// La capacité d'une entrée `device.<nom>` ou `device.<nom>.<aspect>`, si elle en désigne une.
fn entry_capability(id: &str) -> Option<Capability> {
    let rest = id.strip_prefix("device.")?;
    let name = rest.split('.').next()?;
    Capability::ALL
        .into_iter()
        .find(|capability| wire(*capability) == name)
}

/// Les entrées de la carte qui décrivent une capacité de l'appareil.
fn entries_of(carte: &Surface, capability: Capability) -> Vec<&this_agent::surface::Capability> {
    let base = format!("device.{}", wire(capability));
    let prefix = format!("{base}.");
    carte
        .capabilities
        .iter()
        .filter(|entry| entry.id == base || entry.id.starts_with(&prefix))
        .collect()
}

/// Toutes les méthodes que la carte nomme pour une capacité, entrées sœurs comprises.
fn methods_of(carte: &Surface, capability: Capability) -> BTreeSet<String> {
    entries_of(carte, capability)
        .into_iter()
        .filter_map(|entry| entry.command.clone())
        .flat_map(|command| {
            command
                .split(" / ")
                .map(str::trim)
                .filter(|method| !method.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn every_capability_of_the_catalogue_is_in_the_map() {
    let carte = surface();
    for capability in Capability::ALL {
        let id = format!("device.{}", wire(capability));
        let entry = carte.capability(&id).unwrap_or_else(|| {
            panic!("{id} absent de la carte : l'agent ne peut pas savoir que {capability:?} existe")
        });
        assert_eq!(entry.status, Status::Live, "{id}");
        assert_eq!(entry.tier, Tier::Shell, "{id}");
        assert!(entry.platforms.contains(&Platform::Ios), "{id}");
        assert!(entry.command.is_some(), "{id} doit nommer ses méthodes");
    }
}

#[test]
fn every_method_of_the_bridge_is_named_by_its_capability() {
    let carte = surface();
    // Quatre méthodes lisent leur capacité dans leurs arguments — `status`, `captureStatus`,
    // `captureRequest`, `locationRequest` : `capability_of` rend `None` sans arguments. On
    // vérifie alors seulement qu'elles sont nommées quelque part.
    let mut nommees: BTreeSet<String> = BTreeSet::new();
    for capability in Capability::ALL {
        nommees.extend(methods_of(&carte, capability));
    }
    for method in METHODS {
        match capability_of(method, &Value::Null) {
            Some(capability) => {
                let methodes = methods_of(&carte, capability);
                assert!(
                    methodes.contains(*method),
                    "{method} sert {capability:?} et n'est nommé par aucune entrée de la carte ({methodes:?})"
                );
            }
            None => assert!(
                nommees.contains(*method),
                "{method} n'est nommé par aucune entrée de la carte"
            ),
        }
    }
    // Et l'inverse : la carte n'invente aucune méthode.
    for method in &nommees {
        assert!(
            METHODS.contains(&method.as_str()),
            "{method} est nommé par la carte mais absent du pont"
        );
    }
}

/// Entrées `device.*` qui ne parlent PAS du pont d'appareil : `device.events` est l'étage
/// coquille des événements du système (clavier, gestes, batterie, réseau, orientation). Le
/// préfixe se ressemble, le sujet n'a rien à voir avec `Capability::ALL`.
const HORS_PONT: [&str; 1] = ["device.events"];

#[test]
fn every_device_entry_belongs_to_a_capability() {
    // Une entrée `device.<autre chose>` qui ne désigne aucune capacité du catalogue serait une
    // capacité inventée : elle apparaîtrait dans la carte sans que rien ne la serve.
    let carte = surface();
    for entry in carte
        .capabilities
        .iter()
        .filter(|entry| entry.id.starts_with("device."))
        .filter(|entry| !HORS_PONT.contains(&entry.id.as_str()))
    {
        assert!(
            entry_capability(&entry.id).is_some(),
            "{} n'est la capacité d'aucun nom de fil",
            entry.id
        );
    }
}

/// Le dispatch macOS, ou `None` si les sources ne sont pas là.
fn macos_dispatch() -> Option<String> {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("macos/Sources/dispatch.swift"),
    )
    .ok()
}

/// Ce qui n'est pas annoncé sur macOS, et que le fichier ne refuse pas pour autant.
///
/// `location_always` : macOS répond `currentState()` pour cette capacité, ce qui est le même
/// état que `location` — pas une fonction en plus. Il n'y a pas de localisation en arrière-plan
/// sur un Mac, donc la carte ne l'annonce pas, et l'absence d'un refus explicite est normale.
const PAS_ANNONCE_SANS_REFUS_EXPLICITE: [&str; 1] = ["location_always"];

#[test]
fn what_the_map_claims_on_macos_exists_there() {
    let Some(dispatch) = macos_dispatch() else {
        return;
    };
    let carte = surface();
    for capability in Capability::ALL {
        for entry in entries_of(&carte, capability) {
            if !entry.platforms.contains(&Platform::Macos) {
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
                // `status` est servi par `macos_device_status`, pas par la table des méthodes :
                // c'est le nom de la CAPACITÉ qui l'atteint.
                let needle = if method == "status" {
                    format!("\"{}\":", wire(capability))
                } else {
                    format!("\"{method}\"")
                };
                assert!(
                    dispatch.contains(&needle),
                    "{} promet {method} sur macOS, absent de dispatch.swift (cherché {needle})",
                    entry.id
                );
            }
        }
    }
}

#[test]
fn what_the_map_skips_on_macos_is_refused_there() {
    let Some(dispatch) = macos_dispatch() else {
        return;
    };
    // L'arm « Not available on macOS » du fichier : ce que le pont macOS refuse lui-même.
    let refus = dispatch
        .split("Not available on macOS")
        .nth(1)
        .expect("dispatch.swift nomme ce qu'il ne sait pas faire sur macOS");
    let carte = surface();
    for capability in Capability::ALL {
        let entries = entries_of(&carte, capability);
        assert!(!entries.is_empty(), "{capability:?} absent de la carte");
        if entries
            .iter()
            .any(|entry| entry.platforms.contains(&Platform::Macos))
        {
            continue;
        }
        let nom = wire(capability);
        if PAS_ANNONCE_SANS_REFUS_EXPLICITE.contains(&nom.as_str()) {
            continue;
        }
        assert!(
            refus.contains(&format!("\"{nom}\"")),
            "device.{nom} n'est annoncé sur aucune entrée macOS, mais dispatch.swift ne le refuse pas explicitement"
        );
    }
}
