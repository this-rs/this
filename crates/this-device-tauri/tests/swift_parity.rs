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
    //
    // La comparaison se fait sur les noms DÉJÀ dérivés des fichiers, pas en reconstruisant un
    // chemin : la première version faisait `dir.join("<nom>.swift")`, ce qui ne marchait que sur
    // un système de fichiers insensible à la casse (macOS). Sur Linux, `bluetooth.swift` ne
    // trouve pas `Bluetooth.swift`, et le test échouait — mesuré au premier passage du job
    // `device_linux`, le 13/11/2026.
    let attendues: std::collections::BTreeSet<&str> =
        attendues.iter().map(String::as_str).collect();
    for ligne in plugin.lines() {
        let ligne = ligne.trim();
        let Some(nom) = ligne.strip_suffix(".self,") else {
            continue;
        };
        if !nom.starts_with("Famille") {
            continue;
        }
        assert!(
            attendues.contains(nom),
            "{nom} est listée dans famillesAncrees et aucun fichier de famille ne la définit"
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

/// **Le vocabulaire d'état est écrit une fois, et c'est le même que celui du contrat.**
///
/// `ios/Sources/Commun/Etat.swift` est compilé par les DEUX arbres : côté iOS il appartient au
/// paquet SPM (`path: "Sources"` inclut les sous-répertoires), côté macOS `build.rs` l'ajoute à
/// sa liste `swiftc`. C'est le premier morceau vraiment commun du pont — et la seule façon d'en
/// avoir un sans toucher à l'empaquetage de l'hôte.
///
/// Sa raison d'être est le défaut du 13/11/2026 : `faceIdStatus` ne rendait pas `state`, et le
/// pont lisait `unsupported` sur un appareil qui a un Face ID. Les cinq mots étaient écrits à la
/// main 195 fois ; une faute de frappe y changeait un sens sans rien signaler.
///
/// Ce contrôle tient les deux bouts : le fichier existe, et ses cas sont EXACTEMENT les noms de
/// fil de `PermissionState`. Renommer une variante en Rust sans toucher au Swift échoue ici, et
/// l'inverse aussi.
#[test]
fn l_etat_swift_dit_les_memes_mots_que_le_contrat() {
    let racine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let Ok(etat) = std::fs::read_to_string(racine.join("ios/Sources/Commun/Etat.swift")) else {
        panic!("ios/Sources/Commun/Etat.swift a disparu : le vocabulaire n'est plus commun");
    };
    let Ok(contrat) = std::fs::read_to_string(racine.join("../this-platform/src/device_bridge.rs"))
    else {
        eprintln!("device_bridge.rs introuvable : vocabulaire non vérifié");
        return;
    };

    // Côté Swift : les `case` du seul enum déclaré.
    let mut mots_swift: Vec<String> = Vec::new();
    for ligne in etat.lines() {
        let ligne = ligne.trim();
        if let Some(reste) = ligne.strip_prefix("case ") {
            // `case granted` — un cas par ligne dans ce fichier, et c'est voulu.
            mots_swift.push(reste.trim().to_string());
        }
    }
    assert!(
        !mots_swift.is_empty(),
        "aucun `case` trouvé dans Etat.swift : la forme du fichier a changé"
    );

    // Côté Rust : les variantes de `PermissionState`, en snake_case (`rename_all`).
    let debut = contrat
        .find("pub enum PermissionState {")
        .expect("PermissionState a disparu du contrat");
    let corps = &contrat[debut..];
    let fin = corps
        .find("\n}")
        .expect("fin de PermissionState introuvable");
    let mut mots_rust: Vec<String> = Vec::new();
    for ligne in corps[..fin].lines().skip(1) {
        let ligne = ligne.trim();
        if ligne.starts_with("//") || ligne.is_empty() {
            continue;
        }
        let Some(variante) = ligne.strip_suffix(',') else {
            continue;
        };
        let snake: String = variante
            .chars()
            .enumerate()
            .flat_map(|(index, c)| {
                if index > 0 && c.is_uppercase() {
                    vec!['_', c.to_lowercase().next().unwrap_or(c)]
                } else {
                    vec![c.to_lowercase().next().unwrap_or(c)]
                }
            })
            .collect();
        mots_rust.push(snake);
    }

    mots_swift.sort();
    mots_rust.sort();
    assert_eq!(
        mots_swift, mots_rust,
        "le vocabulaire Swift et `PermissionState` ne disent pas les mêmes mots"
    );
}

/// Hors d'un bundle `.app`, `UNUserNotificationCenter.current()` lève une exception
/// Objective-C (`bundleProxyForCurrentProcess is nil`) que Rust ne rattrape pas : le processus
/// meurt. Un binaire lancé directement (`cargo run`, un banc d'essai) n'a pas de bundle. Mesuré le
/// 21/11/2026 : l'Atelier plantait à son auto-test.
///
/// Chaque fonction de `MacNotifications` qui touche le centre doit donc COMMENCER par la garde
/// `centreDisponible`.
///
/// **Ce que ce test promet, et ce qu'il ne promet pas.** Il est STATIQUE : il lit le texte et
/// vérifie qu'une garde ouvre chaque fonction qui nomme `UNUserNotificationCenter` (tout
/// modificateur : `static`, `private static`, `static private`…) et qu'aucune référence au
/// centre ne traîne hors d'une fonction. Il ne prouve pas que la garde est juste ni que le
/// processus survit : cela, c'est `les_sept_entrees_de_notification_survivent_hors_bundle`
/// (macOS seulement, exécution réelle). Un fichier illisible FAIT ÉCHOUER le test.
#[test]
fn every_macos_notification_entry_point_is_guarded_against_a_missing_bundle() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("macos/Sources/notifications.swift");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} illisible : {e}", path.display()));
    assert!(
        text.contains("static var centreDisponible"),
        "la garde `centreDisponible` doit exister"
    );
    let lines: Vec<&str> = text.lines().collect();
    // Une déclaration de fonction : `func ` précédé, au plus, de modificateurs.
    let est_func = |l: &str| {
        let t = l.trim_start();
        let sans = [
            "private ",
            "fileprivate ",
            "internal ",
            "public ",
            "static ",
            "final ",
        ]
        .iter()
        .fold(t, |acc, m| {
            let mut cur = acc;
            while let Some(r) = cur.strip_prefix(m) {
                cur = r;
            }
            cur
        });
        sans.starts_with("func ")
    };
    let starts: Vec<usize> = (0..lines.len()).filter(|&i| est_func(lines[i])).collect();
    // Aucune référence au centre avant la première fonction (une propriété, un `let` global).
    let first = starts.first().copied().unwrap_or(lines.len());
    assert!(
        !lines[..first]
            .iter()
            .any(|l| l.contains("UNUserNotificationCenter") && !l.trim_start().starts_with("//")),
        "une référence à UNUserNotificationCenter hors de toute fonction n'est pas gardée"
    );
    let mut checked = 0;
    for (n, &i) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(lines.len());
        let body = &lines[i + 1..end];
        let touche = body
            .iter()
            .any(|l| l.contains("UNUserNotificationCenter") && !l.trim_start().starts_with("//"));
        if !touche {
            continue; // `stateName`, `entier` : sans centre.
        }
        checked += 1;
        let first = body
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with("//"))
            .unwrap_or("");
        assert!(
            first.starts_with("guard centreDisponible"),
            "`{}` touche UNUserNotificationCenter sans commencer par `guard centreDisponible` \
             (première instruction : `{first}`) — hors bundle, le processus mourrait",
            lines[i].trim()
        );
    }
    assert!(
        checked >= 7,
        "les sept points d'entrée de MacNotifications doivent être vérifiés, pas {checked}"
    );
}

/// EXÉCUTION réelle, macOS seulement : compile les sources Swift avec un petit `main` et appelle
/// les sept points d'entrée de `MacNotifications` dans un binaire SANS bundle. Chacun doit dire
/// « indisponible » et le processus doit survivre. Sans la garde, `UNUserNotificationCenter.current()`
/// tue le processus (mesuré : `NSException` non rattrapée).
///
/// Limite : c'est un binaire nu, pas l'app `.app` : le chemin AVEC bundle (où la garde laisse
/// passer) n'est pas exercé ici. `swiftc` absent sur macOS est un échec, pas un saut.
#[cfg(target_os = "macos")]
#[test]
fn les_sept_entrees_de_notification_survivent_hors_bundle() {
    use std::process::Command;
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dossier = std::env::temp_dir().join(format!("this-notif-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dossier).unwrap();
    let binaire = dossier.join("probe");
    // Du code de premier niveau n'est permis que dans un fichier nommé `main.swift`.
    let main_swift = dossier.join("main.swift");
    std::fs::copy(
        manifest.join("tests/fixtures/notifications_probe.swift"),
        &main_swift,
    )
    .unwrap();

    let mut sources: Vec<std::path::PathBuf> = Vec::new();
    for sous in ["macos/Sources", "ios/Sources/Commun"] {
        if let Ok(rd) = std::fs::read_dir(manifest.join(sous)) {
            sources.extend(
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "swift")),
            );
        }
    }
    let sortie = Command::new("swiftc")
        .arg("-module-name")
        .arg("probe")
        .arg("-o")
        .arg(&binaire)
        .arg(&main_swift)
        .args(&sources)
        .output()
        .expect("swiftc doit exister sur macOS");
    assert!(
        sortie.status.success(),
        "compilation Swift : {}",
        String::from_utf8_lossy(&sortie.stderr)
    );
    let run = Command::new(&binaire).output().expect("lancer le binaire");
    let out = String::from_utf8_lossy(&run.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dossier);
    assert!(
        run.status.success(),
        "le processus est mort hors bundle (statut {:?}) :\n{out}\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(out.contains("centreDisponible=false"), "{out}");
    assert!(out.contains("ENTRY currentState -> unsupported"), "{out}");
    for nom in ["status", "request", "post", "cancel", "schedule", "pending"] {
        let attendu = format!(r#"ENTRY {nom} -> {{"kind":"error","code":"unavailable"}}"#);
        assert!(out.contains(&attendu), "{nom} : {out}");
    }
    assert!(out.contains("ALIVE"), "{out}");
}
