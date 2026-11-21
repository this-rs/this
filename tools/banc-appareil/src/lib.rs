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
                // Le rapport est écrit par le pont lui-même ; on le lit du retour pour le
                // journal de l'app, et rien de plus.
                let rapport = tauri::async_runtime::block_on(tauri_device_banc(&handle));
                let defauts = rapport
                    .get("defauts")
                    .and_then(|v| v.as_array())
                    .map(|d| d.len())
                    .unwrap_or(0);
                eprintln!("banc : {defauts} defaut(s)");
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
/// (bureau sans app, CI) ne tire pas le module `banc`, qui n'y a pas de sens.
#[cfg(any(target_os = "ios", target_os = "macos"))]
async fn tauri_device_banc<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> serde_json::Value {
    // `mesurer_et_ecrire` écrit `Documents/banc.json` et `banc.log` à côté du pont, et rend le
    // rapport. L'app n'a plus à connaître ni la sérialisation, ni les noms de fichiers : c'est
    // ce qui permet à n'importe quel hôte d'appeler la même chose en une ligne.
    this_device_tauri::banc::mesurer_et_ecrire(app).await
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
async fn tauri_device_banc<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> serde_json::Value {
    serde_json::json!({ "kind": "banc", "erreur": "aucun appareil sur cette plateforme" })
}
