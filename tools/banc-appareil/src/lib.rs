//! L'hôte minimal du banc d'essai du pont d'appareil.
//!
//! Il ne sert qu'à une chose : lancer le balayage du contrat **sur l'appareil**, et écrire le
//! rapport là où on peut le lire sans câble de débogage (`Documents/banc.json`).
//!
//! Pourquoi une app séparée plutôt que l'Atelier : le pont doit être mesurable **sans** la
//! coquille. Une app de 200 lignes qui ne dépend que de `tauri` et du plugin se construit en
//! quelques minutes, se réinstalle sans toucher aux données de personne, et peut être lancée sur
//! chaque iPhone disponible. Mesurer le pont à travers une app de 15 000 lignes, c'est mesurer
//! les deux à la fois.
//!
//! Le déclenchement est un **environnement**, pas un bouton : `devicectl device process launch
//! -e '{"THIS_DEVICE_BANC":"1"}'` — un balayage ne part jamais tout seul.

/// Où le rapport est écrit, dans le conteneur de l'app : `Documents/banc.json` et un journal
/// ligne à ligne dans `Documents/banc.log`.
const RAPPORT: &str = "Documents/banc.json";
const JOURNAL: &str = "Documents/banc.log";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_device_plugin())
        .setup(|app| {
            if std::env::var_os("THIS_DEVICE_BANC").is_none() {
                return Ok(());
            }
            let handle = app.handle().clone();
            // Hors du fil de démarrage : le balayage prend des secondes (un scan Bluetooth en
            // dure trois), et une fenêtre qui ne s'affiche pas se lit comme un plantage.
            std::thread::spawn(move || {
                let rapport = tauri::async_runtime::block_on(tauri_device_banc(&handle));
                ecrire(&rapport);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("l'app Banc n'a pas pu démarrer");
}

/// Le plugin, avec sa fonction d'initialisation nommée à part : `init` est aussi le nom de la
/// fonction de Tauri, et deux `init` dans le même fichier se confondent à la lecture.
fn tauri_device_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    this_device_tauri::init()
}

/// Le balayage. Séparé pour que `setup` reste lisible, et pour que la version hors appareil
/// (bureau, CI) ne tire pas le module `banc`, qui n'y a pas de sens.
#[cfg(any(target_os = "ios", target_os = "macos"))]
async fn tauri_device_banc<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> serde_json::Value {
    this_device_tauri::banc::balayer(app).await
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
async fn tauri_device_banc<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> serde_json::Value {
    serde_json::json!({ "kind": "banc", "erreur": "aucun appareil sur cette plateforme" })
}

/// Écrit le rapport et son journal. Un échec d'écriture ne doit pas tuer l'app : il est dit.
fn ecrire(rapport: &serde_json::Value) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let racine = std::path::Path::new(&home);
    let _ = std::fs::create_dir_all(racine.join("Documents"));
    let _ = std::fs::write(
        racine.join(RAPPORT),
        serde_json::to_string_pretty(rapport).unwrap_or_default(),
    );

    let mut journal = String::new();
    if let Some(capacites) = rapport.get("capacites").and_then(|v| v.as_array()) {
        for entree in capacites {
            journal.push_str(&format!(
                "capacite {} = {}\n",
                entree["capacite"].as_str().unwrap_or("?"),
                entree["etat"].as_str().unwrap_or("?")
            ));
        }
    }
    if let Some(methodes) = rapport.get("methodes").and_then(|v| v.as_array()) {
        for entree in methodes {
            journal.push_str(&format!(
                "methode {} [{}] = {}{}{}\n",
                entree["methode"].as_str().unwrap_or("?"),
                entree["nature"].as_str().unwrap_or("?"),
                entree["etat"].as_str().unwrap_or("?"),
                entree
                    .get("code")
                    .and_then(|v| v.as_str())
                    .map(|code| format!(" ({code})"))
                    .unwrap_or_default(),
                entree
                    .get("ms")
                    .and_then(|v| v.as_u64())
                    .map(|ms| format!(" {ms}ms"))
                    .unwrap_or_default(),
            ));
        }
    }
    if let Some(ecritures) = rapport.get("ecritures").and_then(|v| v.as_array()) {
        for entree in ecritures {
            journal.push_str(&format!(
                "ecriture {} [{}] = {}{}\n",
                entree["methode"].as_str().unwrap_or("—"),
                entree["groupe"].as_str().unwrap_or("—"),
                entree["etat"].as_str().unwrap_or("?"),
                entree
                    .get("code")
                    .and_then(|v| v.as_str())
                    .map(|code| format!(" ({code})"))
                    .unwrap_or_default(),
            ));
        }
    }
    journal.push_str(&format!(
        "defauts = {}\n",
        rapport
            .get("defauts")
            .and_then(|v| v.as_array())
            .map(|d| d
                .iter()
                .map(|entree| entree["methode"].as_str().unwrap_or("?"))
                .collect::<Vec<_>>()
                .join(","))
            .unwrap_or_default()
    ));
    let _ = std::fs::write(racine.join(JOURNAL), journal);
}
