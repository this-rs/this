//! Le backend Linux du pont matériel.
//!
//! Même contrat que macOS et iOS (les noms de méthodes de `this_platform::device_bridge`, les
//! formes de réponse de [`crate::families`]), mais sans code natif propre : un bureau Linux
//! expose ses capteurs par `sysfs`, ses services par D-Bus.
//!
//! | Famille        | Source                                                         |
//! |----------------|----------------------------------------------------------------|
//! | Bluetooth      | BlueZ (`org.bluez`, ObjectManager) sur le bus **système**      |
//! | Caméra / micro | `/dev/video*`, sockets PipeWire/Pulse, portail XDG (bus session)|
//! | Localisation   | GeoClue2 sur le bus système                                    |
//! | Gyroscope      | `/sys/bus/iio/devices/iio:deviceN`                             |
//!
//! ## Règles tenues partout
//!
//! - **Jamais de panique, jamais de blocage.** Chaque échange D-Bus a un délai ; chaque lecture
//!   de fichier de périphérique passe par `spawn_blocking`. Un bus qui ne répond pas est un
//!   `timeout`, un bus absent est `unsupported` / `not_supported`.
//! - **On ne ment pas sur la permission.** Linux n'a pas de dialogue système à ouvrir depuis ce
//!   processus : une demande rend l'état réel, jamais un `granted` inventé.
//! - Le vocabulaire d'erreur est celui du contrat ([`DeviceError`]) : rien n'est ajouté.
//!
//! La logique pure de chaque module (classification d'un nœud, lecture d'un attribut IIO,
//! filtrage d'une liste d'appareils, mapping d'un nom d'erreur D-Bus) est testée sans matériel
//! et sans bus.

pub mod bluetooth;
pub mod camera_mic;
pub mod location;
pub mod motion;

use std::future::Future;
use std::time::Duration;

use serde_json::{Value, json};
use this_platform::capability::Capability;
use this_platform::device_bridge::{DeviceError, PermissionState, Reply};

use crate::families::camera_mic::{CaptureArgs, CaptureError, CaptureReply};

/// Le délai d'une ouverture de bus ou d'un appel D-Bus isolé. Un démon muet ne retient pas le
/// tour : au-delà, c'est `timeout`.
pub const BUS_STEP_TIMEOUT: Duration = Duration::from_secs(3);

/// Le nom d'une erreur D-Bus, en refus typé du contrat.
///
/// Les noms `org.freedesktop.DBus.Error.*`, `org.freedesktop.GeoClue2.Error.*` et
/// `org.bluez.Error.*` sont ceux que le bus renvoie. Un service absent est `NotSupported` ; un
/// refus d'accès `NotAuthorized` ; un délai `Timeout` ; tout autre nom (le service répond,
/// mais pas maintenant : radio coupée, client occupé) `Unavailable`.
pub fn error_for_dbus_name(name: &str) -> DeviceError {
    match name {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.UnknownObject"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.NotSupported"
        | "org.bluez.Error.NotSupported"
        | "org.bluez.Error.NotReady" => DeviceError::NotSupported,
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.GeoClue2.Error.AccessDenied"
        | "org.freedesktop.GeoClue2.Error.NotAuthorized"
        | "org.bluez.Error.NotAuthorized"
        | "org.bluez.Error.AuthenticationRejected" => DeviceError::NotAuthorized,
        "org.freedesktop.DBus.Error.NoReply"
        | "org.freedesktop.DBus.Error.Timeout"
        | "org.freedesktop.DBus.Error.TimedOut" => DeviceError::Timeout,
        _ => DeviceError::Unavailable,
    }
}

/// La raison d'un échec de zbus, en refus typé. Pas d'adresse de bus ou socket injoignable :
/// le service n'existe pas sur cette machine (`NotSupported`), ce n'est pas une panne.
pub fn error_of(err: &zbus::Error) -> DeviceError {
    match err {
        zbus::Error::MethodError(name, _, _) => error_for_dbus_name(name.as_str()),
        zbus::Error::Address(_) | zbus::Error::InputOutput(_) => DeviceError::NotSupported,
        _ => DeviceError::Unavailable,
    }
}

/// Borne un futur : à l'échéance, `Timeout`. Aucun appel D-Bus de ce module n'y échappe.
pub async fn bounded<T, F>(limit: Duration, fut: F) -> Result<T, DeviceError>
where
    F: Future<Output = Result<T, DeviceError>>,
{
    match tokio::time::timeout(limit, fut).await {
        Ok(result) => result,
        Err(_) => Err(DeviceError::Timeout),
    }
}

/// Ouvre le bus système, borné.
pub async fn system_bus() -> Result<zbus::Connection, DeviceError> {
    bounded(BUS_STEP_TIMEOUT, async {
        zbus::Connection::system().await.map_err(|e| error_of(&e))
    })
    .await
}

/// `{"kind":"error","code":…}` pour un refus du contrat.
pub fn error_json(code: DeviceError) -> Value {
    json!({ "kind": "error", "code": code })
}

/// `{"kind":"status","state":…}`.
pub fn status_json(state: PermissionState) -> Value {
    json!({ "kind": "status", "state": state })
}

/// **Ce que ce backend porte réellement du contrat**, et rien de plus.
///
/// C'est la seule déclaration de la surface Linux ; `tests/linux_surface.rs` la tient contre
/// deux choses à la fois :
///
/// 1. le **dispatch lui-même** — pour chaque méthode de `METHODS`, `call` doit répondre
///    `not_supported` si et seulement si elle n'est pas ici (mesuré en l'appelant, pas en
///    relisant le `match`) ;
/// 2. la **carte du framework** — une entrée `device.*` qui nomme `Linux` ne peut citer que
///    des méthodes de cette liste.
///
/// Écrite à la main, donc elle peut mentir — d'où les deux contrôles. Ce qui n'y est pas
/// (connexion Bluetooth, capture photo/vidéo, contacts, calendrier, santé, NFC, Face ID,
/// parole, notifications, Live Activity, presse-papiers, partage, réseau local) répond
/// `not_supported`, et la carte ne l'annonce pas sur Linux.
pub const SUPPORTED: &[&str] = &[
    // BlueZ (bus système) : l'état de la permission et le scan. La connexion, la lecture d'une
    // caractéristique et l'écriture ne sont pas branchées.
    "status",
    "bluetoothState",
    "bluetoothScan",
    // Le portail XDG : l'ÉTAT de la caméra et du micro, et la demande — qui, sous Linux,
    // n'ouvre aucun dialogue : elle rend l'état réel.
    "captureStatus",
    "captureRequest",
    // GeoClue2 : la demande d'autorisation et la position ponctuelle. Pas de flux.
    "locationRequest",
    "locationSample",
    // `sysfs` / IIO : un échantillon de gyroscope.
    "gyroscopeSample",
];

/// Les capacités dont ce backend sait lire l'**état** (`Request::Status`). Les autres
/// répondent `unsupported` — ce n'est pas la même chose qu'une méthode non portée.
pub const STATUS_CAPABILITIES: &[Capability] = &[
    Capability::Gyroscope,
    Capability::Bluetooth,
    Capability::Camera,
    Capability::Microphone,
];

/// L'état de permission d'une capacité, pour `Request::Status` (gyroscope, Bluetooth, et la
/// caméra / micro qu'un appelant demanderait par ce chemin). Tout le reste : `unsupported`.
pub async fn status(capability: Capability) -> Reply {
    let state = match capability {
        Capability::Gyroscope => motion::gyroscope_status().await,
        Capability::Bluetooth => bluetooth::permission_state().await,
        Capability::Camera => {
            camera_mic::state_of(crate::families::camera_mic::CaptureCapability::Camera).await
        }
        Capability::Microphone => {
            camera_mic::state_of(crate::families::camera_mic::CaptureCapability::Microphone).await
        }
        _ => PermissionState::Unsupported,
    };
    Reply::Status { state }
}

/// Une réponse de capture refusée `unsupported`, dans la forme de la famille.
fn capture_unsupported() -> Value {
    serde_json::to_value(CaptureReply::Error {
        code: CaptureError::Unsupported,
    })
    .unwrap_or_else(|_| error_json(DeviceError::Internal))
}

/// Un appel de famille (`Request::Call`) : la méthode dit où aller. Une méthode que ce backend
/// ne porte pas est `not_supported` — et jamais un état inventé.
pub async fn call(method: &str, args: &Value) -> Value {
    match method {
        "captureStatus" | "captureRequest" => {
            // Linux n'ouvre aucun dialogue : demander rend l'état réel, comme lire.
            match serde_json::from_value::<CaptureArgs>(args.clone()) {
                Ok(args) => status_json(camera_mic::state_of(args.capability).await),
                Err(_) => error_json(DeviceError::MissingArgument),
            }
        }
        "cameraCapturePhoto" | "cameraRecordVideo" | "microphoneStart" | "microphoneStop" => {
            capture_unsupported()
        }
        "bluetoothState" => bluetooth::state_json().await,
        "bluetoothScan" => {
            let duration = args
                .get("duration_ms")
                .and_then(Value::as_u64)
                .map(|ms| ms.min(u64::from(u32::MAX)) as u32)
                .unwrap_or(crate::wire::SCAN_MS_MIN);
            let services: Vec<String> = args
                .get("services")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            match crate::wire::scan_args(duration, &services) {
                Ok((duration, services)) => {
                    serde_json::to_value(bluetooth::scan(duration, &services).await)
                        .unwrap_or_else(|_| error_json(DeviceError::Internal))
                }
                Err(refusal) => serde_json::to_value(refusal)
                    .unwrap_or_else(|_| error_json(DeviceError::Internal)),
            }
        }
        m if m.starts_with("bluetooth") => {
            serde_json::to_value(crate::families::bluetooth::BluetoothReply::Error {
                code: crate::families::bluetooth::BluetoothError::NotSupported,
            })
            .unwrap_or_else(|_| error_json(DeviceError::Internal))
        }
        "locationRequest" => location::request_json().await,
        "locationSample" => location::sample_json().await,
        "gyroscopeSample" => serde_json::to_value(motion::gyroscope_sample().await)
            .unwrap_or_else(|_| error_json(DeviceError::Internal)),
        // Pas de coprocesseur de mouvement (podomètre) sur un bureau : l'état le dit.
        "motionActivityStatus" | "motionActivityRequest" => {
            status_json(PermissionState::Unsupported)
        }
        _ => error_json(DeviceError::NotSupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un exécuteur minimal : les tests ne dépendent d'aucune macro de tokio.
    pub(crate) fn block_on<T>(fut: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime de test")
            .block_on(fut)
    }

    #[test]
    fn a_missing_service_is_not_supported() {
        for name in [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.bluez.Error.NotReady",
        ] {
            assert_eq!(
                error_for_dbus_name(name),
                DeviceError::NotSupported,
                "{name}"
            );
        }
    }

    #[test]
    fn a_refusal_of_access_is_not_authorized() {
        for name in [
            "org.freedesktop.DBus.Error.AccessDenied",
            "org.freedesktop.GeoClue2.Error.AccessDenied",
            "org.bluez.Error.NotAuthorized",
        ] {
            assert_eq!(
                error_for_dbus_name(name),
                DeviceError::NotAuthorized,
                "{name}"
            );
        }
    }

    #[test]
    fn a_silent_daemon_is_a_timeout_and_the_rest_is_unavailable() {
        assert_eq!(
            error_for_dbus_name("org.freedesktop.DBus.Error.NoReply"),
            DeviceError::Timeout
        );
        // Radio coupée, opération en cours : le service répond, mais pas maintenant.
        assert_eq!(
            error_for_dbus_name("org.bluez.Error.InProgress"),
            DeviceError::Unavailable
        );
        assert_eq!(error_for_dbus_name(""), DeviceError::Unavailable);
    }

    #[test]
    fn every_mapped_code_is_a_wire_code_of_the_contract() {
        // Le vocabulaire existe déjà : ce module n'en ajoute pas.
        for code in [
            DeviceError::NotSupported,
            DeviceError::NotAuthorized,
            DeviceError::Timeout,
            DeviceError::Unavailable,
        ] {
            let reply: Reply = serde_json::from_value(error_json(code)).unwrap();
            assert_eq!(reply, Reply::Error { code });
        }
    }

    #[test]
    fn a_future_that_never_answers_is_a_timeout() {
        let out: Result<(), DeviceError> = block_on(bounded(Duration::from_millis(20), async {
            std::future::pending::<Result<(), DeviceError>>().await
        }));
        assert_eq!(out, Err(DeviceError::Timeout));
    }

    #[test]
    fn an_unknown_method_is_not_supported_without_touching_anything() {
        let out = block_on(call("contactsList", &json!({})));
        assert_eq!(out, json!({"kind":"error","code":"not_supported"}));
        let out = block_on(call("captureStatus", &json!({})));
        assert_eq!(out, json!({"kind":"error","code":"missing_argument"}));
    }

    #[test]
    fn capture_and_bluetooth_calls_this_backend_lacks_are_typed_refusals() {
        assert_eq!(
            block_on(call("cameraCapturePhoto", &json!({}))),
            json!({"kind":"error","code":"unsupported"})
        );
        assert_eq!(
            block_on(call("bluetoothConnect", &json!({}))),
            json!({"kind":"error","code":"not_supported"})
        );
        assert_eq!(
            block_on(call("motionActivityStatus", &json!({}))),
            json!({"kind":"status","state":"unsupported"})
        );
    }
}
