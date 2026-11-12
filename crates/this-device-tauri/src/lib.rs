//! # `tauri-plugin-this-device` — raw device primitives for iOS
//!
//! Three native primitives, answering with the wire contract of
//! [`this_platform::device_bridge`]:
//!
//! - `status(capability)` — the permission state (`granted`, `denied`, `prompt`,
//!   `unsupported`), read without prompting the person;
//! - `gyroscope_sample()` — one gyroscope reading, or a refusal after about 2 s;
//! - `bluetooth_scan(duration_ms, services)` — a one-shot scan, the devices seen, once each;
//! - [`answer`] with a `Request::Call` — any of the thirteen device families (contacts,
//!   photos, files, calendar, health, Face ID, speech, NFC, local network, motion, location,
//!   notifications, live activity, clipboard, share), each with its own typed wire shape in
//!   [`families`].
//!
//! ## Security: these are raw primitives, not an API for modules
//!
//! The commands are **not** a permission and must never be reachable by a module directly.
//! The access decision belongs to the device broker, [`this_platform::device_bridge::gate`]:
//! it asks the person's consent before a request is forwarded to [`perform`]. That wiring is
//! not done yet, so until it is:
//!
//! - this plugin declares **no default permission** (no `default.toml`): a capability file
//!   must grant `this-device:allow-*` explicitly to expose a command to a webview. Do not add
//!   such a grant, nor `default`, without the broker in front of it;
//! - a Rust caller must go through `gate` first; [`perform`] does not check consent itself.
//!
//! ## Use
//!
//! ```ignore
//! tauri::Builder::default().plugin(tauri_plugin_this_device::init())
//! ```
//!
//! Only iOS talks to the device. On every other target, each request is answered with
//! `Unsupported` (`status`) or `not_supported` (the others), without touching anything.

use std::marker::PhantomData;

use serde_json::{Value, json};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime};
use this_platform::capability::Capability;
use this_platform::device_bridge::{DeviceError, PermissionState, Reply, Request};

pub mod consent;
pub mod families;
pub mod wire;

/// Le banc d'essai : ce que l'appareil devant soi sait vraiment faire. Compilé seulement avec
/// la feature `banc`, parce que ce n'est pas une surface de production — c'est un diagnostic
/// qu'un hôte demande, pas un module. Voir `src/banc.rs`.
#[cfg(feature = "banc")]
pub mod banc;

#[cfg(target_os = "ios")]
use tauri::plugin::PluginHandle;

#[cfg(target_os = "macos")]
mod macos;

/// Combien de temps on attend le natif avant de répondre `timeout`. Un capteur peut être
/// lent, mais une promesse qui ne se résout jamais bloque le tour de l'agent entier.
#[cfg(target_os = "ios")]
const NATIVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

// The Swift entry point, defined in `ios/Sources/ThisDevicePlugin.swift`.
#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_this_device);

/// The plugin's state: on iOS, the handle to the Swift side.
pub struct ThisDevice<R: Runtime> {
    #[cfg(target_os = "ios")]
    handle: PluginHandle<R>,
    _runtime: PhantomData<fn() -> R>,
}

/// Build the plugin. Add it once, before the app runs. It declares no permission: see the
/// module documentation.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("this-device")
        .invoke_handler(tauri::generate_handler![
            status,
            gyroscope_sample,
            bluetooth_scan
        ])
        .setup(|app, api| {
            #[cfg(not(target_os = "ios"))]
            let _ = &api;
            #[cfg(target_os = "ios")]
            let handle = api.register_ios_plugin(init_plugin_this_device)?;
            app.manage(ThisDevice::<R> {
                #[cfg(target_os = "ios")]
                handle,
                _runtime: PhantomData,
            });
            Ok(())
        })
        .build()
}

/// Answer one request, as JSON.
///
/// This is the single entry point of the bridge, and the only one a shell should call. It is
/// **not** gated: the caller must check consent first with
/// `this_platform::device_bridge::gate` and only call this when the gate returned `Ok`.
///
/// A family answers in its own shape (`{"kind":"contacts",...}`), so the answer is a
/// [`Value`]; the caller decodes it with the family's own type. The three pilot primitives
/// are still checked against [`Reply`] on the way back, because their shape is fixed.
pub async fn answer<R: Runtime>(app: &AppHandle<R>, request: &Request) -> Value {
    match request {
        Request::Call { method, args } => call_native(app, method, args).await,
        other => match serde_json::to_value(perform(app, other).await) {
            Ok(value) => value,
            // A `Reply` always serializes: this arm keeps the signature honest without a panic.
            Err(_) => json!({ "kind": "error", "code": "internal" }),
        },
    }
}

/// Les méthodes « état système » que le pont expose SANS consentement : des lectures passives
/// de l'appareil (modèle, batterie, réseau, langue, écran, volume). Ni permission, ni
/// entitlement, ni donnée personnelle — c'est ce qui les distingue des familles `device.*`,
/// qui passent par le consentement. Tout nom hors de cette liste est refusé `not_supported`.
pub const SYSTEM_METHODS: &[&str] = &[
    "deviceInfo",
    "batteryLevel",
    "networkState",
    "locale",
    "screenState",
    "volume",
];

/// Une lecture d'état système, sans consentement. `method` doit être dans [`SYSTEM_METHODS`] ;
/// tout autre nom est refusé. Aucune donnée personnelle n'est lue ni écrite.
///
/// Ce chemin est volontairement séparé d'[`answer`] : `answer` suppose un `Request` du
/// contrat (consenti par la personne via `gate`) ; ici il n'y a rien à consentir, donc pas de
/// `Request`. Hors iOS il n'y a pas d'appareil : `not_supported`.
pub async fn system_state<R: Runtime>(app: &AppHandle<R>, method: &str) -> Value {
    #[cfg(target_os = "ios")]
    {
        if !SYSTEM_METHODS.contains(&method) {
            return json!({ "kind": "error", "code": "not_supported" });
        }
        // `try_state`, jamais `state` : un plugin absent doit être une réponse, pas une panique.
        let Some(plugin) = app.try_state::<ThisDevice<R>>() else {
            return json!({ "kind": "error", "code": "internal" });
        };
        match tokio::time::timeout(
            NATIVE_TIMEOUT,
            plugin
                .handle
                .run_mobile_plugin_async::<Value>(method, json!({})),
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                note(&format!("system_state {method} a échoué : {error:?}"));
                json!({ "kind": "error", "code": "internal" })
            }
            Err(_) => json!({ "kind": "error", "code": "timeout" }),
        }
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, method);
        json!({ "kind": "error", "code": "not_supported" })
    }
}

/// Les flux d'état système que le pont pousse SANS consentement : mêmes lectures passives
/// que [`SYSTEM_METHODS`] (orientation de l'écran, batterie), envoyées à chaque changement au
/// lieu d'être demandées. Tout autre nom est refusé `not_supported`, ici et côté natif.
pub const STREAMS: &[&str] = &["orientation", "battery"];

/// Les sélecteurs natifs des flux : départ (avec un canal) et arrêt.
pub const STREAM_METHODS: &[&str] = &["streamStart", "streamStop"];

/// Les sélecteurs de **diagnostic** : ils ne lisent ni n'écrivent aucune donnée de la personne,
/// et ils existent pour interroger le pont lui-même. `listCommands` dit quelles familles sont
/// réellement liées dans le binaire — ce que le contrat ne peut pas promettre, puisque c'est
/// l'éditeur de liens qui décide.
pub const DIAGNOSTIC_METHODS: &[&str] = &["listCommands"];

/// Démarre un flux de [`STREAMS`]. `on_sample` reçoit chaque échantillon natif, tel quel
/// (`{"kind":"orientation","angle":…}`, `{"kind":"battery","level":…,"state":…}`) : un
/// échantillon initial tout de suite, puis un par notification du système. L'appelant le
/// convertit et l'horodate ; le pont ne fait que transporter.
///
/// Le canal est un `tauri::ipc::Channel` créé côté Rust : aucune webview n'est traversée,
/// aucune permission de webview n'est ouverte. Hors iOS : `not_supported`.
pub async fn stream_start<R: Runtime>(
    app: &AppHandle<R>,
    stream: &str,
    on_sample: impl Fn(Value) + Send + Sync + 'static,
) -> Value {
    if !STREAMS.contains(&stream) {
        return json!({ "kind": "error", "code": "not_supported" });
    }
    #[cfg(target_os = "ios")]
    {
        let Some(plugin) = app.try_state::<ThisDevice<R>>() else {
            return json!({ "kind": "error", "code": "internal" });
        };
        let channel = tauri::ipc::Channel::<Value>::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(text) = body
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                on_sample(value);
            }
            Ok(())
        });
        let args = json!({ "stream": stream, "channel": channel });
        match tokio::time::timeout(
            NATIVE_TIMEOUT,
            plugin
                .handle
                .run_mobile_plugin_async::<Value>("streamStart", args),
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                note(&format!("stream_start {stream} a échoué : {error:?}"));
                json!({ "kind": "error", "code": "internal" })
            }
            Err(_) => json!({ "kind": "error", "code": "timeout" }),
        }
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, on_sample);
        json!({ "kind": "error", "code": "not_supported" })
    }
}

/// Arrête un flux de [`STREAMS`]. Arrêter un flux qui ne tourne pas n'est pas une erreur.
pub async fn stream_stop<R: Runtime>(app: &AppHandle<R>, stream: &str) -> Value {
    if !STREAMS.contains(&stream) {
        return json!({ "kind": "error", "code": "not_supported" });
    }
    #[cfg(target_os = "ios")]
    {
        let Some(plugin) = app.try_state::<ThisDevice<R>>() else {
            return json!({ "kind": "error", "code": "internal" });
        };
        match tokio::time::timeout(
            NATIVE_TIMEOUT,
            plugin
                .handle
                .run_mobile_plugin_async::<Value>("streamStop", json!({ "stream": stream })),
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => json!({ "kind": "error", "code": "internal" }),
            Err(_) => json!({ "kind": "error", "code": "timeout" }),
        }
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        json!({ "kind": "error", "code": "not_supported" })
    }
}

/// One call of a device family. The method is checked against the contract's table before it
/// is sent, so a typo is a refusal here rather than an unknown selector on the native side.
#[cfg(target_os = "ios")]
async fn call_native<R: Runtime>(app: &AppHandle<R>, method: &str, args: &Value) -> Value {
    // Les arguments que l'appelant peut omettre sont complétés ICI, dans le pont : c'est le
    // seul endroit traversé par tous les chemins (outil de l'agent, coquille, diagnostic).
    // Sans eux, le décodage natif répond `internal` en nommant une clé que l'appelant ne
    // pouvait pas deviner (« Key 'duration_ms' not found »).
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    let completed = this_platform::device_bridge::complete_args(method, args, now_ms);
    let manquants = this_platform::device_bridge::missing_args(method, &completed);
    if !manquants.is_empty() {
        note(&format!(
            "call {method} : argument(s) requis et absent(s) : {}",
            manquants.join(", ")
        ));
        // Code dédié : `internal` laissait l'agent conclure à une panne du pont alors qu'il
        // manquait un argument qu'il peut fournir lui-même. La liste est dans la RÉPONSE, pas
        // seulement dans le journal : c'est le seul endroit que l'appelant lit, et un message
        // qu'il faut aller chercher dans un fichier ne sert à personne.
        return json!({
            "kind": "error",
            "code": "missing_argument",
            "missing": manquants,
        });
    }
    let args = &completed;
    if this_platform::device_bridge::capability_of(method, args).is_none() {
        return json!({ "kind": "error", "code": "not_supported" });
    }
    // Les bornes d'une famille se vérifient ICI, avant que quoi que ce soit n'atteigne le
    // natif : une écriture Bluetooth de 10 Ko ne doit pas partir chez le périphérique en
    // morceaux silencieux, et un identifiant inventé ne doit pas devenir un `unavailable`
    // qui accuse la radio.
    // Les deux familles rendent des types DIFFÉRENTS (`BluetoothReply`,
    // `ClipboardShareReply`) : les mettre dans un même tableau ne compilait pas (E0308,
    // « expected Option<BluetoothReply> »). On aplatit chacun en JSON dans son propre
    // élément, si bien que le tableau est homogène.
    fn refus_en_json<T: serde::Serialize>(refus: Option<T>) -> Option<serde_json::Value> {
        refus.map(|refus| {
            serde_json::to_value(refus)
                .unwrap_or_else(|_| json!({ "kind": "error", "code": "internal" }))
        })
    }
    for refus in [
        refus_en_json(families::bluetooth::check(method, args)),
        refus_en_json(families::clipboard_share::check(method, args)),
    ]
    .into_iter()
    .flatten()
    {
        note(&format!(
            "call {method} : refusé avant le natif ({refus:?})"
        ));
        return refus;
    }
    // `try_state`, jamais `state` : un plugin absent doit être une réponse, pas une panique.
    // Une panique dans une commande async laisse la promesse d'invocation non résolue, donc
    // l'appelant attend indéfiniment — mesuré : l'outil de l'agent restait bloqué sans
    // jamais rendre la main ni poser la moindre question.
    let Some(plugin) = app.try_state::<ThisDevice<R>>() else {
        return json!({ "kind": "error", "code": "internal" });
    };
    match tokio::time::timeout(
        NATIVE_TIMEOUT,
        plugin
            .handle
            .run_mobile_plugin_async::<Value>(method, args.clone()),
    )
    .await
    {
        Ok(Ok(value)) => value,
        // Un appel qui échoue n'est pas une réponse de l'appareil : c'est une panne du pont.
        // On trace le message natif : sans lui, `internal` est un fourre-tout qui ne dit pas
        // si c'est un argument manquant, une clé absente ou un refus du système.
        Ok(Err(error)) => {
            note(&format!("call {method} args={args} a échoué : {error:?}"));
            json!({ "kind": "error", "code": "internal" })
        }
        // Le natif ne répond pas : on le dit, au lieu d'attendre pour toujours.
        Err(_) => json!({ "kind": "error", "code": "timeout" }),
    }
}

/// On macOS the Swift bridge is called synchronously via C FFI, so it runs on a blocking
/// thread to avoid stalling the Tokio runtime.
#[cfg(target_os = "macos")]
async fn call_native<R: Runtime>(_app: &AppHandle<R>, method: &str, args: &Value) -> Value {
    let method = method.to_string();
    let args = args.clone();
    match tokio::task::spawn_blocking(move || macos::call(&method, &args)).await {
        Ok(value) => value,
        Err(_) => json!({ "kind": "error", "code": "internal" }),
    }
}

/// Off iOS and macOS there is no device: a family call is refused as unsupported.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
async fn call_native<R: Runtime>(_app: &AppHandle<R>, _method: &str, _args: &Value) -> Value {
    json!({ "kind": "error", "code": "not_supported" })
}

/// Answer one request, natively, in the shape of the pilot's [`Reply`]. Not gated: see
/// [`answer`].
#[cfg(target_os = "ios")]
pub async fn perform<R: Runtime>(app: &AppHandle<R>, request: &Request) -> Reply {
    // Voir `call_native` : un plugin absent est une réponse, pas une panique.
    let Some(plugin) = app.try_state::<ThisDevice<R>>() else {
        return Reply::Error {
            code: DeviceError::Internal,
        };
    };
    let native = match request {
        // Une seule table decide quelle méthode lit quel état (`device_bridge::status_request`).
        // Le plugin avait sa propre copie du routage et des arguments : elles ont divergé, et
        // camera et santé ont répondu `internal` une après-midi entière.
        Request::Status { capability } => {
            match this_platform::device_bridge::status_request(*capability) {
                // Rien à demander : un sélecteur système n'a pas d'état.
                None => Ok(Reply::Status {
                    state: PermissionState::Unsupported,
                }),
                Some(Request::Status { capability }) => {
                    plugin
                        .handle
                        .run_mobile_plugin_async::<Reply>(
                            "status",
                            json!({ "capability": capability }),
                        )
                        .await
                }
                Some(Request::Call { method, args }) => match plugin
                    .handle
                    .run_mobile_plugin_async::<Value>(&method, args)
                    .await
                {
                    // Les familles répondent `{"kind":"status","state":"…"}`. `limited`
                    // (photos, accès partiel iOS 14+) est un état, pas un refus.
                    Ok(reply) => Ok(Reply::Status {
                        state: reply
                            .get("state")
                            .and_then(|state| serde_json::from_value(state.clone()).ok())
                            .unwrap_or(PermissionState::Unsupported),
                    }),
                    Err(error) => {
                        note(&format!("status {method} a échoué : {error:?}"));
                        Ok(Reply::Error {
                            code: DeviceError::Internal,
                        })
                    }
                },
                // `status_request` ne rend qu'un statut ou un appel. Toute autre forme serait
                // un bug du contrat, pas une requête de module : on ne devine pas.
                Some(_) => Ok(Reply::Status {
                    state: PermissionState::Unsupported,
                }),
            }
        }
        Request::GyroscopeSample => {
            plugin
                .handle
                .run_mobile_plugin_async::<Reply>("gyroscopeSample", json!({}))
                .await
        }
        Request::BluetoothScan {
            duration_ms,
            services,
        } => {
            let (duration_ms, services) = match wire::scan_args(*duration_ms, services) {
                Ok(args) => args,
                Err(refusal) => return refusal,
            };
            plugin
                .handle
                .run_mobile_plugin_async::<Reply>(
                    "bluetoothScan",
                    json!({ "duration_ms": duration_ms, "services": services }),
                )
                .await
        }
        // Reached only if a caller bypasses `answer`. A family answer is not a `Reply`, so
        // answering it here would be a lie: it is refused instead.
        Request::Call { .. } => {
            return Reply::Error {
                code: DeviceError::NotSupported,
            };
        }
    };
    // A failed call into the plugin is not a device answer: it is an internal error.
    native.unwrap_or(Reply::Error {
        code: DeviceError::Internal,
    })
}

/// Append one line to the app's journal (`$HOME/Documents/atelier.log`), the same file the
/// shell's own `trace` writes. A native failure has no place in the device contract — the
/// contract carries states, not Tauri messages — so it is recorded where it can be read.
#[cfg(target_os = "ios")]
fn note(line: &str) {
    use std::io::Write;
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{home}/Documents/atelier.log"))
    else {
        return;
    };
    let _ = writeln!(file, "device {line}");
}

/// Ask the native runtime which selectors it really exposes. A diagnostic, not a bridge
/// call: it goes around the contract on purpose, because the contract is what is in doubt.
#[cfg(target_os = "ios")]
pub async fn commands<R: Runtime>(app: &AppHandle<R>) -> Value {
    let Some(plugin) = app.try_state::<ThisDevice<R>>() else {
        return json!({ "kind": "error", "code": "internal" });
    };
    plugin
        .handle
        .run_mobile_plugin_async::<Value>("listCommands", json!({}))
        .await
        .unwrap_or_else(|error| json!({ "kind": "error", "message": format!("{error:?}") }))
}

/// On macOS, the Swift bridge is present: list the supported methods.
#[cfg(target_os = "macos")]
pub async fn commands<R: Runtime>(_app: &AppHandle<R>) -> Value {
    json!({
        "kind": "commands",
        "platform": "macos",
        "methods": this_platform::device_bridge::METHODS
    })
}

/// Hors iOS et macOS il n'y a pas de runtime natif à interroger. Sans cette version,
/// l'Atelier (qui appelle `commands` dans son auto-test) ne compilait plus sur l'hôte :
/// `cargo test --lib` échouait en E0425 — mesuré le 06/11.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub async fn commands<R: Runtime>(_app: &AppHandle<R>) -> Value {
    json!({ "kind": "error", "code": "not_supported" })
}

/// On macOS the Swift bridge answers permission states and Bluetooth scans. The gyroscope
/// does not exist on macOS, and family calls go through [`call_native`] (via [`answer`]).
#[cfg(target_os = "macos")]
pub async fn perform<R: Runtime>(_app: &AppHandle<R>, request: &Request) -> Reply {
    match request {
        Request::Status { capability } => {
            let cap_json = serde_json::to_value(capability).unwrap_or_default();
            let cap_str = cap_json.as_str().unwrap_or("").to_string();
            match tokio::task::spawn_blocking(move || macos::status(&cap_str)).await {
                Ok(value) => serde_json::from_value(value).unwrap_or(Reply::Status {
                    state: PermissionState::Unsupported,
                }),
                Err(_) => Reply::Status {
                    state: PermissionState::Unsupported,
                },
            }
        }
        // Gyroscope: CoreMotion is not available on macOS.
        Request::GyroscopeSample => Reply::Error {
            code: DeviceError::NotSupported,
        },
        // Bluetooth: CoreBluetooth IS available on macOS.
        Request::BluetoothScan {
            duration_ms,
            services,
        } => {
            let (duration_ms, services) = match wire::scan_args(*duration_ms, services) {
                Ok(args) => args,
                Err(refusal) => return refusal,
            };
            let args = json!({ "duration_ms": duration_ms, "services": services });
            match tokio::task::spawn_blocking(move || macos::call("bluetoothScan", &args)).await {
                Ok(value) => serde_json::from_value(value).unwrap_or(Reply::Error {
                    code: DeviceError::Internal,
                }),
                Err(_) => Reply::Error {
                    code: DeviceError::Internal,
                },
            }
        }
        Request::Call { .. } => Reply::Error {
            code: DeviceError::NotSupported,
        },
    }
}

/// Off iOS and macOS there is no device: every request is refused as unsupported.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub async fn perform<R: Runtime>(_app: &AppHandle<R>, request: &Request) -> Reply {
    match request {
        Request::Status { .. } => Reply::Status {
            state: PermissionState::Unsupported,
        },
        Request::GyroscopeSample | Request::BluetoothScan { .. } | Request::Call { .. } => {
            Reply::Error {
                code: DeviceError::NotSupported,
            }
        }
    }
}

/// The permission state of a capability. Raw: see the module documentation.
#[tauri::command]
async fn status<R: Runtime>(app: AppHandle<R>, capability: Capability) -> Reply {
    perform(&app, &Request::Status { capability }).await
}

/// One gyroscope sample. Raw: see the module documentation.
#[tauri::command]
async fn gyroscope_sample<R: Runtime>(app: AppHandle<R>) -> Reply {
    perform(&app, &Request::GyroscopeSample).await
}

/// A one-shot Bluetooth scan. Raw: see the module documentation.
#[tauri::command]
async fn bluetooth_scan<R: Runtime>(
    app: AppHandle<R>,
    duration_ms: u32,
    services: Vec<String>,
) -> Reply {
    perform(
        &app,
        &Request::BluetoothScan {
            duration_ms,
            services,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::SYSTEM_METHODS;

    /// Les six méthodes d'état système sont nommées, exactement comme les sélecteurs Swift
    /// (`ios/Sources/system_state.swift`) — un nom qui diverge devient `not_supported` sans
    /// que rien ne le dise à la compilation.
    #[test]
    fn the_six_system_methods_are_the_whitelist() {
        for m in [
            "deviceInfo",
            "batteryLevel",
            "networkState",
            "locale",
            "screenState",
            "volume",
        ] {
            assert!(SYSTEM_METHODS.contains(&m), "{m} absent de la liste");
        }
        assert_eq!(SYSTEM_METHODS.len(), 6);
    }

    /// Tout nom hors liste est refusé : `system_state` ne laisse passer que des lectures
    /// passives connues, jamais un sélecteur inventé.
    #[test]
    fn an_unknown_system_method_is_not_whitelisted() {
        assert!(!SYSTEM_METHODS.contains(&"status"));
        assert!(!SYSTEM_METHODS.contains(&"cameraCapturePhoto"));
        assert!(!SYSTEM_METHODS.contains(&"deviceInfoExtra"));
    }
}
