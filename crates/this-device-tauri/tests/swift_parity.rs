//! Parity between the contract's method table and the native selectors.
//!
//! The bridge decides what a module may ask for from `this_platform::device_bridge::METHODS`.
//! The native side dispatches by selector name. Nothing at compile time ties the two together,
//! so a method added on one side and not the other fails silently: the Rust side refuses a
//! capability that exists, or the Swift side answers a method nobody can reach.
//!
//! This test reads the real `.swift` sources and compares. It needs the sources to be present,
//! which they are in the repository; it is skipped otherwise rather than pretending to pass.

use std::collections::BTreeSet;

use serde_json::json;
use this_device_tauri::{DIAGNOSTIC_METHODS, STREAM_METHODS, SYSTEM_METHODS};
use this_platform::device_bridge::{METHODS, capability_of};

/// Every `@objc public func <name>` declared by the plugin's Swift sources.
fn swift_methods() -> Option<BTreeSet<String>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ios/Sources");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut names = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("swift") {
            continue;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("@objc public func ") else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                names.insert(name);
            }
        }
    }
    Some(names)
}

#[test]
fn every_native_selector_is_reachable_through_the_contract() {
    let Some(swift) = swift_methods() else {
        eprintln!("ios/Sources not readable: parity not checked");
        return;
    };
    assert!(
        swift.len() >= 35,
        "only {} selectors found: the extraction is broken, not the sources",
        swift.len()
    );
    // Deux chemins : `METHODS` (familles consenties) et `SYSTEM_METHODS` (lectures passives
    // d'état système, sans consentement, servies par `system_state`). Un sélecteur qui n'est
    // dans AUCUNE des deux listes n'est joignable par rien.
    let known: BTreeSet<&str> = METHODS
        .iter()
        .copied()
        .chain(SYSTEM_METHODS.iter().copied())
        .chain(STREAM_METHODS.iter().copied())
        .chain(DIAGNOSTIC_METHODS.iter().copied())
        .collect();
    let unreachable: Vec<&String> = swift
        .iter()
        .filter(|m| !known.contains(m.as_str()))
        .collect();
    assert!(
        unreachable.is_empty(),
        "native selectors no module can reach: {unreachable:?}"
    );
}

#[test]
fn every_method_of_the_contract_exists_on_the_native_side() {
    let Some(swift) = swift_methods() else {
        eprintln!("ios/Sources not readable: parity not checked");
        return;
    };
    let missing: Vec<&&str> = METHODS
        .iter()
        .filter(|m| !swift.contains(**m as &str))
        .collect();
    assert!(
        missing.is_empty(),
        "the contract lists methods the native side does not declare: {missing:?}"
    );
}

#[test]
fn every_native_selector_derives_a_capability() {
    let Some(swift) = swift_methods() else {
        eprintln!("ios/Sources not readable: parity not checked");
        return;
    };
    for method in swift {
        // État système : lectures passives, aucune capacité consentie — le chemin est
        // `system_state`, pas `capability_of`. Rien à dériver ici.
        if SYSTEM_METHODS.contains(&method.as_str())
            || STREAM_METHODS.contains(&method.as_str())
            || DIAGNOSTIC_METHODS.contains(&method.as_str())
        {
            continue;
        }
        let args = if matches!(
            method.as_str(),
            "status" | "captureStatus" | "captureRequest" | "locationRequest" | "locationStatus"
        ) {
            json!({ "capability": "camera" })
        } else {
            json!({})
        };
        assert!(
            capability_of(&method, &args).is_some(),
            "{method} is declared natively but maps to no capability"
        );
    }
}

#[test]
fn every_system_method_exists_on_the_native_side() {
    let Some(swift) = swift_methods() else {
        eprintln!("ios/Sources not readable: parity not checked");
        return;
    };
    let missing: Vec<&&str> = SYSTEM_METHODS
        .iter()
        .filter(|m| !swift.contains(**m as &str))
        .collect();
    assert!(
        missing.is_empty(),
        "SYSTEM_METHODS lists selectors the native side does not declare: {missing:?}"
    );
}

#[test]
fn every_stream_method_exists_on_the_native_side() {
    let Some(swift) = swift_methods() else {
        eprintln!("ios/Sources not readable: parity not checked");
        return;
    };
    let missing: Vec<&&str> = STREAM_METHODS
        .iter()
        .filter(|m| !swift.contains(**m as &str))
        .collect();
    assert!(
        missing.is_empty(),
        "STREAM_METHODS lists selectors the native side does not declare: {missing:?}"
    );
}

/// Chaque flux que le pont accepte est traité par `streamStart` côté natif : un nom de flux
/// sans `case` répondrait `not_supported` alors que Rust l'a laissé passer.
#[test]
fn every_bridged_stream_has_a_native_case() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ios/Sources/streams.swift");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("streams.swift not readable: parity not checked");
        return;
    };
    for stream in this_device_tauri::STREAMS {
        assert!(
            text.contains(&format!("case \"{stream}\":")),
            "{stream} : aucun `case` dans streamStart"
        );
    }
}

/// **Le défaut mesuré le 13/11/2026, et le contrôle qui l'empêche de revenir.**
///
/// Un `#selector(...)` ne garde pas une méthode : il produit une chaîne. Le dead-strip retirait
/// donc l'objet de chaque fichier de famille, et dix-huit méthodes répondaient « No command … found
/// for plugin this-device » sur un iPhone alors que leur code compilait et que la parité des noms
/// était verte. Seule une référence de CLASSE garde l'objet.
///
/// Trois règles, vérifiées sur les sources réelles :
///
/// 1. chaque fichier qui déclare un sélecteur `@objc` (hors le fichier du plugin) porte une
///    `final class Famille<Nom>` — l'ancre ;
/// 2. cette ancre est listée dans `ThisDevicePlugin.famillesAncrees` ;
/// 3. la liste ne nomme aucune ancre qui n'existe pas.
#[test]
fn every_family_file_is_anchored_even_if_nothing_references_its_methods() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ios/Sources");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("ios/Sources illisible : ancres non vérifiées");
        return;
    };
    let Ok(plugin) = std::fs::read_to_string(dir.join("ThisDevicePlugin.swift")) else {
        return;
    };

    let mut manquantes = Vec::new();
    let mut attendues = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("swift") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem == "ThisDevicePlugin" || stem == "Reply" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !text.contains("@objc public func") {
            continue;
        }
        let ancre: String = format!(
            "Famille{}",
            stem.split('_')
                .map(|mot| {
                    let mut chars = mot.chars();
                    match chars.next() {
                        Some(premier) => {
                            premier.to_uppercase().collect::<String>() + chars.as_str()
                        }
                        None => String::new(),
                    }
                })
                .collect::<String>()
        );
        if !text.contains(&format!("final class {ancre}:")) {
            manquantes.push(format!("{stem}.swift : aucune `final class {ancre}`"));
        }
        if !plugin.contains(&format!("{ancre}.self")) {
            manquantes.push(format!("{ancre} absente de famillesAncrees"));
        }
        attendues.push(ancre);
    }

    assert!(
        manquantes.is_empty(),
        "des fichiers de famille seraient retirés par l'éditeur de liens :\n  {}",
        manquantes.join("\n  ")
    );
    assert!(
        attendues.len() >= 15,
        "seulement {} fichier(s) de famille trouvé(s) : l'extraction est cassée, pas les sources",
        attendues.len()
    );

    // Et l'inverse : une ancre listée qui n'existe nulle part serait une référence morte.
    for ligne in plugin.lines() {
        let ligne = ligne.trim();
        let Some(nom) = ligne.strip_suffix(".self,") else {
            continue;
        };
        if !nom.starts_with("Famille") {
            continue;
        }
        let fichier = nom
            .trim_start_matches("Famille")
            .chars()
            .enumerate()
            .flat_map(|(index, c)| {
                if index > 0 && c.is_uppercase() {
                    vec!['_', c.to_lowercase().next().unwrap_or(c)]
                } else {
                    vec![c.to_lowercase().next().unwrap_or(c)]
                }
            })
            .collect::<String>();
        let existe = dir.join(format!("{fichier}.swift")).exists()
            || dir.join(format!("{}.swift", nom.to_lowercase())).exists();
        assert!(
            existe,
            "{nom} est listée dans famillesAncrees et ne correspond à aucun fichier"
        );
    }
}

/// **Le pont ne lit qu'un champ d'une réponse d'état : `state`.**
///
/// `perform` (branche `Request::Status`) appelle la méthode d'état de la capacité et lit
/// `reply["state"]`. Un natif qui répond une autre forme — `faceIdStatus` répondait
/// `{"kind":"face_id_biometry","biometry":…}` — n'est pas une erreur : c'est un `unsupported`
/// silencieux. Mesuré le 13/11/2026 sur un iPhone 16 Pro Max, qui a un Face ID et se l'entendait
/// dire « pas sur cet appareil ».
///
/// Trois formes sont acceptées pour une méthode d'état, et trois seulement :
///
/// 1. elle écrit la clé (`"state"`) ;
/// 2. elle construit une réponse typée dont le champ s'appelle `state` — `Reply.status(state:)`,
///    `PhotosReply.status(state:)`… : le nom de la clé est alors décidé par le type, une fois ;
/// 3. elle délègue à un lecteur qui rend un `Reply` (le seul cas : `notificationsStatus`).
///
/// `status` lui-même est hors règle : il répond `Reply`, dont le champ est déjà `state`.
#[test]
fn every_status_method_answers_the_state_the_bridge_reads() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ios/Sources");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("ios/Sources illisible : états non vérifiés");
        return;
    };
    let mut sources = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("swift") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            sources.push(text);
        }
    }

    let mut fautives = Vec::new();
    let mut verifiees = 0usize;
    for capability in this_platform::capability::Capability::ALL {
        let Some(method) = this_platform::device_bridge::status_method(capability) else {
            continue;
        };
        if method == "status" {
            continue;
        }
        let Some((body, file)) = method_body(&sources, method) else {
            fautives.push(format!("{method} : corps introuvable dans ios/Sources"));
            continue;
        };
        verifiees += 1;
        let ecrit_la_cle = body.contains("state");
        let delegue_a_un_reply =
            body.contains("invoke.resolve($0)") && file.contains("Reply.status(state:");
        if !(ecrit_la_cle || delegue_a_un_reply) {
            fautives.push(format!(
                "{method} ne rend pas `state` : le pont lira `unsupported` pour {capability:?}"
            ));
        }
    }
    assert!(
        verifiees >= 10,
        "seulement {verifiees} méthode(s) d'état trouvée(s) : l'extraction est cassée"
    );
    assert!(fautives.is_empty(), "\n  {}\n", fautives.join("\n  "));
}

/// Le corps d'un `@objc public func <nom>(…)` — jusqu'au `@objc public func` suivant — et le
/// fichier qui le contient (une méthode peut déléguer à un lecteur défini plus haut dans le
/// même fichier : c'est là qu'on voit si ce lecteur rend un `state`).
fn method_body<'a>(sources: &'a [String], name: &str) -> Option<(String, &'a String)> {
    let needle = format!("@objc public func {name}(");
    for text in sources {
        let Some(start) = text.find(&needle) else {
            continue;
        };
        let rest = &text[start..];
        let end = rest[1..]
            .find("@objc public func ")
            .map(|index| index + 1)
            .unwrap_or(rest.len());
        return Some((rest[..end].to_string(), text));
    }
    None
}
