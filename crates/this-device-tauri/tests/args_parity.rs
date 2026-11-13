//! Parity between the arguments the native decoder REQUIRES and what the bridge PROVIDES.
//!
//! Le décodeur Swift est strict : une clé requise absente fait échouer `parseArgs`, et le pont
//! traduit cet échec en `internal`. C'est un fourre-tout qui accuse le pont alors qu'un argument
//! manquait — et l'appelant, qui ne connaît pas la signature, n'a aucun moyen de le deviner.
//!
//! Mesuré sur un iPhone le 13/11/2026 : `faceIdConfirm` répondait
//! `DecodingError.keyNotFound: Key 'reason' not found`, et l'outil de l'agent rendait `echec=true`.
//! Aucun test ne reliait les deux côtés, donc le défaut vivait dans `main`.
//!
//! Deux règles, vérifiées contre les sources Swift réelles :
//!
//! 1. **complet** — pour chaque méthode du contrat, tout argument requis par son `Decodable`
//!    est présent après `complete_args` (le pont le remplit d'un défaut honnête) ;
//! 2. **nommé** — sinon `missing_args` doit nommer CET argument, pour que l'appelant sache quoi
//!    fournir au lieu de lire `internal`.
//!
//! Un `let reason: String` non optionnel n'a pas de défaut honnête : le système l'affiche à la
//! personne. Ce cas relève de la règle 2, pas de la 1.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use this_device_tauri::{STREAM_METHODS, SYSTEM_METHODS};
use this_platform::device_bridge::{METHODS, complete_args, missing_args};

/// Le moment où l'on calcule les défauts : figé, pour que le test soit déterministe.
const NOW_MS: f64 = 1_760_000_000_000.0;

/// Une propriété d'un `Decodable` Swift.
struct Property {
    /// Le nom dans le JSON : le nom de la propriété, sauf si `CodingKeys` le renomme.
    wire: String,
    /// Optionnelle (`String?`) ou pourvue d'une valeur par défaut (`let x = 1`).
    optional: bool,
}

/// Retire les commentaires `//` d'une ligne, en laissant les chaînes intactes.
fn strip_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_string = false;
    for (index, ch) in line.char_indices() {
        if ch == '"' && line.as_bytes().get(index) != Some(&b'\\') {
            in_string = !in_string;
        }
        if !in_string && ch == '/' && line.as_bytes().get(index + 1) == Some(&b'/') {
            break;
        }
        out.push(ch);
    }
    out
}

/// Le corps d'un bloc `{ … }` à partir de l'accolade ouvrante, accolades imbriquées comprises.
fn block_body(text: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (index, ch) in text[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..open + index];
                }
            }
            _ => {}
        }
    }
    &text[open + 1..]
}

/// Les `Decodable` déclarés dans un fichier : nom → propriétés (nom de fil, optionnelle).
fn decodables(text: &str, into: &mut BTreeMap<String, Vec<Property>>) {
    let mut cursor = 0usize;
    while let Some(found) = text[cursor..].find(": Decodable") {
        let at = cursor + found;
        let Some(open) = text[at..].find('{').map(|index| at + index) else {
            break;
        };
        // Le nom du type est le dernier identifiant avant `: Decodable`.
        let name: String = text[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let body = block_body(text, open);

        // `CodingKeys` renomme : c'est la seule chose qui décide du nom de fil.
        let mut renames: BTreeMap<String, String> = BTreeMap::new();
        if let Some(start) = body.find("CodingKeys") {
            let body_block = body[start..]
                .find('{')
                .map(|index| block_body(&body[start..], index))
                .unwrap_or("");
            for line in body_block.lines() {
                let line = strip_comment(line);
                let line = line.trim();
                let Some(rest) = line.strip_prefix("case ") else {
                    continue;
                };
                for part in rest.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    match part.split_once('=') {
                        Some((ident, wire)) => renames.insert(
                            ident.trim().to_string(),
                            wire.trim().trim_matches('"').to_string(),
                        ),
                        None => renames.insert(part.to_string(), part.to_string()),
                    };
                }
            }
        }

        let mut properties = Vec::new();
        for line in body.lines() {
            let line = strip_comment(line);
            let line = line.trim();
            let Some(rest) = line
                .strip_prefix("let ")
                .or_else(|| line.strip_prefix("var "))
            else {
                continue;
            };
            let Some((ident, tail)) = rest.split_once(':') else {
                continue;
            };
            let ident = ident.trim();
            if ident.is_empty() || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let ty = tail.split('=').next().unwrap_or("").trim();
            let optional = ty.ends_with('?') || ty.starts_with("Optional<") || tail.contains('=');
            let wire = renames
                .get(ident)
                .cloned()
                .unwrap_or_else(|| ident.to_string());
            properties.push(Property { wire, optional });
        }
        if !name.is_empty() {
            into.insert(name, properties);
        }
        cursor = open;
    }
}

/// Chaque méthode native et le `Decodable` qu'elle décode.
fn native_signatures(sources: &[String]) -> BTreeMap<String, String> {
    let mut methods = BTreeMap::new();
    for text in sources {
        let mut cursor = 0usize;
        while let Some(found) = text[cursor..].find("@objc public func ") {
            let start = cursor + found + "@objc public func ".len();
            let name: String = text[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let next = text[start..]
                .find("@objc public func ")
                .map(|index| start + index)
                .unwrap_or(text.len());
            let body = &text[start..next];
            if let Some(at) = body.find("parseArgs(") {
                let rest = &body[at + "parseArgs(".len()..];
                let ty: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !ty.is_empty() {
                    methods.insert(name, ty);
                }
            }
            cursor = start;
        }
    }
    methods
}

fn sources() -> Option<Vec<String>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ios/Sources");
    let mut texts = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("swift") {
            texts.push(std::fs::read_to_string(entry.path()).ok()?);
        }
    }
    Some(texts)
}

/// Les arguments requis par la méthode : ce que le décodeur natif refuse de voir manquer.
fn required_arguments(
    signatures: &BTreeMap<String, String>,
    decodable: &BTreeMap<String, Vec<Property>>,
    method: &str,
) -> Vec<String> {
    let Some(ty) = signatures.get(method) else {
        return Vec::new();
    };
    decodable
        .get(ty)
        .map(|properties| {
            properties
                .iter()
                .filter(|property| !property.optional)
                .map(|property| property.wire.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn every_required_argument_is_filled_or_named() {
    let Some(texts) = sources() else {
        eprintln!("ios/Sources illisible : parité non vérifiée");
        return;
    };
    let mut decodable = BTreeMap::new();
    for text in &texts {
        decodables(text, &mut decodable);
    }
    let signatures = native_signatures(&texts);
    assert!(
        signatures.len() >= 40,
        "only {} native signatures found: the extraction is broken, not the sources",
        signatures.len()
    );

    let mut problems = Vec::new();
    for method in METHODS
        .iter()
        .copied()
        .chain(SYSTEM_METHODS.iter().copied())
    {
        // `status` ne passe pas par `complete_args` : `perform` construit ses arguments lui-même
        // (`device_bridge::status_request`), parce que la capacité y EST la requête. Les autres
        // méthodes à `capability` (`captureStatus`, `captureRequest`, `locationRequest`) passent,
        // elles, par `call_native`, et leur défaut est vérifié comme les autres.
        if method == "status" {
            continue;
        }
        let required = required_arguments(&signatures, &decodable, method);
        if required.is_empty() {
            continue;
        }
        let completed = complete_args(method, &json!({}), NOW_MS);
        let named = missing_args(method, &completed);
        for key in required {
            if completed.get(&key).is_some() || named.contains(&key.as_str()) {
                continue;
            }
            problems.push(format!(
                "{method} : le décodeur natif exige `{key}`, le pont ne le remplit pas et \
                 `missing_args` ne le nomme pas (il dit {named:?}) — l'appelant lira `internal`"
            ));
        }
    }
    assert!(problems.is_empty(), "\n  {}\n", problems.join("\n  "));
}

/// Le contrôle inverse : un défaut que le natif ne lit pas ne sert à rien, et un défaut inventé
/// pour une clé que la méthode n'a pas est du bruit qui masque le vrai contrat.
#[test]
fn every_default_belongs_to_the_method_it_is_written_for() {
    let Some(texts) = sources() else {
        return;
    };
    let mut decodable = BTreeMap::new();
    for text in &texts {
        decodables(text, &mut decodable);
    }
    let signatures = native_signatures(&texts);
    let mut extra = Vec::new();
    for method in METHODS.iter().copied() {
        let Some(ty) = signatures.get(method) else {
            continue;
        };
        let Some(properties) = decodable.get(ty) else {
            continue;
        };
        let known: BTreeSet<&str> = properties.iter().map(|p| p.wire.as_str()).collect();
        let completed = complete_args(method, &json!({}), NOW_MS);
        // `capability` et les bornes que le pont ajoute lui-même (durée d'un scan, identifiant
        // d'un flux) sont fournis pour la dérivation de capacité : elles ne sont pas du bruit.
        let defaults: Vec<String> = completed
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        for key in defaults {
            if known.contains(key.as_str()) || key == "capability" {
                continue;
            }
            extra.push(format!(
                "{method} : défaut `{key}` que {ty} ne lit pas ({known:?})"
            ));
        }
    }
    assert!(extra.is_empty(), "\n  {}\n", extra.join("\n  "));
}

/// Aucune méthode native ne décode d'arguments sans que le contrat la connaisse : sinon le pont
/// refuse une méthode qui existe, et rien ne le dit.
#[test]
fn every_method_with_arguments_is_in_the_contract() {
    let Some(texts) = sources() else {
        return;
    };
    let signatures = native_signatures(&texts);
    let known: BTreeSet<&str> = METHODS
        .iter()
        .copied()
        .chain(SYSTEM_METHODS.iter().copied())
        .chain(STREAM_METHODS.iter().copied())
        .collect();
    let orphans: Vec<&String> = signatures
        .keys()
        .filter(|method| !known.contains(method.as_str()))
        .collect();
    assert!(
        orphans.is_empty(),
        "méthodes natives hors contrat : {orphans:?}"
    );
}
