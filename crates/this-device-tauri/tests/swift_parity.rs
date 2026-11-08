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
use this_device_tauri::SYSTEM_METHODS;
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
        if SYSTEM_METHODS.contains(&method.as_str()) {
            continue;
        }
        let args = if matches!(
            method.as_str(),
            "status" | "captureStatus" | "captureRequest" | "locationRequest"
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
