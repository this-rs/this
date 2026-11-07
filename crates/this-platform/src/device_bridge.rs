//! The device bridge: the pure contract between a module and the device.
//!
//! A module never touches the device. It sends a [`Request`]; the shell answers with a
//! [`Reply`]. Every request that touches the device or its data goes through [`gate`], which
//! asks the person's consent through a [`Consent`] and refuses unless the answer is
//! [`Decision::Allow`]. The gate is fail-closed: [`Decision::Ask`] and [`Decision::Deny`] are
//! both refusals, and a capability the bridge does not support is refused before consent is
//! even asked.
//!
//! One exception, and it is deliberate: reading the permission *state* (`Request::Status`) is
//! a meta-question, not a data access. It is answered without consent, exactly as the system
//! reports it to any app, because asking permission in order to learn whether permission was
//! granted would turn a diagnostic into a queue of questions.
//!
//! No I/O and no async: the shell implements the device side, this module only decides.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::Capability;

/// The capabilities the iOS pilot bridges for sampling and scanning.
const BRIDGED: [Capability; 2] = [Capability::Gyroscope, Capability::Bluetooth];

/// A permission state, as the system reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    /// Access to a subset the person picked (iOS 14+ on the photo library). Not a refusal:
    /// what the app can read is exactly what was chosen, and saying `denied` would tell a
    /// module to give up on data the person deliberately shared.
    Limited,
    Prompt,
    Unsupported,
}

/// What a module asks the device to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The permission state of a capability. Any capability of [`Capability::ALL`] can be
    /// asked for; the answer is still gated by consent.
    Status { capability: Capability },
    /// One gyroscope sample. A single reading, not a stream.
    GyroscopeSample,
    /// A Bluetooth scan lasting `duration_ms`, optionally filtered by service identifiers.
    BluetoothScan {
        duration_ms: u32,
        services: Vec<String>,
    },
    /// One call of a device family (contacts, photos, calendar, ...), named by its native
    /// method. The arguments and the answer are JSON: each family owns the typed shape
    /// (`crate::device_bridge::family` users decode it), the contract carries it.
    ///
    /// The capability is **derived from `method`** by [`capability_of`], never declared by
    /// the caller: a module cannot ask to be checked against a capability it does not need.
    Call { method: String, args: Value },
}

/// What the shell answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Status { state: PermissionState },
    Gyroscope { x: f64, y: f64, z: f64, ts_ms: u64 },
    Devices { devices: Vec<BtDevice> },
    Error { code: DeviceError },
}

/// A Bluetooth peripheral seen during a scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtDevice {
    pub id: String,
    pub name: Option<String>,
    pub rssi: i32,
}

/// Why a request was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceError {
    /// The person has not granted this module this capability.
    NotAuthorized,
    /// The bridge does not support this capability.
    NotSupported,
    /// The device is present but cannot answer now (Bluetooth off, sensor absent).
    Unavailable,
    /// The device did not answer in time.
    Timeout,
    /// Something went wrong on the shell side.
    /// A required argument is missing and no honest default can replace it.
    MissingArgument,
    Internal,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            DeviceError::NotAuthorized => "not authorized",
            DeviceError::NotSupported => "not supported",
            DeviceError::Unavailable => "unavailable",
            DeviceError::Timeout => "timeout",
            DeviceError::MissingArgument => "missing argument",
            DeviceError::Internal => "internal error",
        };
        f.write_str(text)
    }
}

/// The answer of the person's consent for one module and one capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// The person has not answered yet. Never an implicit agreement.
    Ask,
    Deny,
}

/// Where the person's consent lives. The shell implements it; tests use a fake.
pub trait Consent {
    fn decide(&self, module: &str, capability: Capability) -> Decision;
}

/// The device capability a request needs, or `None` when the request names something the
/// bridge does not know: an unknown method has no capability, and is refused before consent.
pub fn required_capability(request: &Request) -> Option<Capability> {
    match request {
        Request::Status { capability } => Some(*capability),
        Request::GyroscopeSample => Some(Capability::Gyroscope),
        Request::BluetoothScan { .. } => Some(Capability::Bluetooth),
        Request::Call { method, args } => capability_of(method, args),
    }
}

/// Every native method the bridge knows, with the capability it needs.
///
/// A method that is not here is not reachable: [`capability_of`] answers `None`, and [`gate`]
/// refuses it with [`DeviceError::NotSupported`] before consent is even consulted. Adding a
/// native method without adding it here leaves it closed, which is the safe direction.
pub const METHODS: &[&str] = &[
    // The three primitives of the pilot.
    "status",
    "gyroscopeSample",
    "bluetoothScan",
    // Camera and microphone share one native pair; the capability is read from the arguments.
    "captureStatus",
    "captureRequest",
    "notificationsStatus",
    "notificationsRequest",
    // Poster et retirer une notification locale : même capacité que la permission, donc la
    // même décision de la personne — une notification n'est pas un contournement.
    "notificationsPost",
    "notificationsCancel",
    "liveActivityStatus",
    "clipboardRead",
    "clipboardWrite",
    "shareText",
    "contactsStatus",
    "contactsRequest",
    "contactsList",
    "photosStatus",
    "photosRequest",
    "photosPick",
    "filesPick",
    "filesRead",
    "calendarStatus",
    "calendarRequest",
    "calendarEvents",
    "calendarCreateEvent",
    "healthStatus",
    "healthRequest",
    "healthQuantitySample",
    "faceIdStatus",
    "faceIdConfirm",
    "speechStatus",
    "speechRequest",
    "speechTranscribeFile",
    "nfcStatus",
    "nfcReadOnce",
    "localNetworkStatus",
    "localNetworkBrowse",
    "locationRequest",
    "locationSample",
    "motionActivityStatus",
    "motionActivityRequest",
    "motionStepsSample",
];

/// The capability a native method needs, derived from the method and its arguments.
///
/// `None` means the bridge does not know the method. Three methods (`status`, `captureStatus`,
/// `captureRequest`) serve two capabilities each, so the capability is read from the
/// `capability` field of the arguments rather than assumed.
pub fn capability_of(method: &str, args: &Value) -> Option<Capability> {
    let capability = match method {
        // One native method, two capabilities: the argument says which. `locationRequest`
        // belongs here too — it asks for in-use OR always, and gating it on `Location` alone
        // would let a module get the background permission by naming the foreground one.
        "status" | "captureStatus" | "captureRequest" | "locationRequest" => {
            return declared_capability(args);
        }
        "gyroscopeSample" => Capability::Gyroscope,
        "bluetoothScan" => Capability::Bluetooth,
        "notificationsStatus"
        | "notificationsRequest"
        | "notificationsPost"
        | "notificationsCancel" => Capability::Notifications,
        "liveActivityStatus" => Capability::LiveActivity,
        "clipboardRead" | "clipboardWrite" => Capability::Clipboard,
        "shareText" => Capability::Share,
        "contactsStatus" | "contactsRequest" | "contactsList" => Capability::Contacts,
        "photosStatus" | "photosRequest" => Capability::Photos,
        "photosPick" => Capability::PhotosPick,
        "filesPick" | "filesRead" => Capability::FilesPick,
        "calendarStatus" | "calendarRequest" | "calendarEvents" | "calendarCreateEvent" => {
            Capability::Calendar
        }
        "healthStatus" | "healthRequest" | "healthQuantitySample" => Capability::Health,
        "faceIdStatus" | "faceIdConfirm" => Capability::FaceId,
        "speechStatus" | "speechRequest" | "speechTranscribeFile" => Capability::Speech,
        "nfcStatus" | "nfcReadOnce" => Capability::Nfc,
        "localNetworkStatus" | "localNetworkBrowse" => Capability::LocalNetwork,
        "locationSample" => Capability::Location,
        "motionActivityStatus" | "motionActivityRequest" | "motionStepsSample" => {
            Capability::MotionActivity
        }
        _ => return None,
    };
    Some(capability)
}

/// The capability named by the `capability` field of `args`, when it is a catalogue entry.
fn declared_capability(args: &Value) -> Option<Capability> {
    serde_json::from_value(args.get("capability")?.clone()).ok()
}

/// The arguments a family's status method requires.
///
/// Most `…Status` methods take nothing. Three do: `captureStatus` and `status` name which of
/// their capabilities is meant, and `healthStatus` names a quantity — HealthKit has no "the
/// health permission", only per-type answers.
pub fn status_args(method: &str, capability: Capability) -> Value {
    match method {
        "status" | "captureStatus" => serde_json::json!({ "capability": capability }),
        "healthStatus" => serde_json::json!({ "quantity": "step_count" }),
        _ => serde_json::json!({}),
    }
}

/// The request that reports the permission state of `capability`, if the bridge can ask it.
///
/// One place builds it, so the shell and the plugin cannot disagree on which method reads
/// which state — they did, and camera and health answered `internal` for a whole afternoon.
pub fn status_request(capability: Capability) -> Option<Request> {
    let method = status_method(capability)?;
    if method == "status" {
        return Some(Request::Status { capability });
    }
    Some(Request::Call {
        method: method.to_string(),
        args: status_args(method, capability),
    })
}

/// The native method that ASKS the system for a capability, and the arguments it needs.
///
/// Two permissions stack, and they are not the same one. The person allows a module through
/// [`gate`]; iOS separately needs its own answer, and only a `…Request` method shows that
/// prompt. Wiring the first without the second is what made a person who had authorised
/// contacts in the app still get `not_authorized` from the system.
///
/// `None` means the system never asks: pickers (a person chooses what to hand over), and
/// capabilities whose access is decided per use (Face ID, NFC, Bonjour).
pub fn permission_request(capability: Capability) -> Option<(&'static str, Value)> {
    let method = match capability {
        Capability::Contacts => "contactsRequest",
        Capability::Camera => "captureRequest",
        Capability::Microphone => "captureRequest",
        Capability::Photos => "photosRequest",
        Capability::Calendar => "calendarRequest",
        Capability::Health => "healthRequest",
        Capability::Speech => "speechRequest",
        Capability::MotionActivity => "motionActivityRequest",
        Capability::Notifications => "notificationsRequest",
        Capability::Location => "locationRequest",
        Capability::LocationAlways => "locationRequest",
        _ => return None,
    };
    let args = match capability {
        // These two families take one argument each, and it names which of their behaviours
        // is asked for. The catalogue spelling is the one the native side reads.
        Capability::Camera
        | Capability::Microphone
        | Capability::Location
        | Capability::LocationAlways => serde_json::json!({ "capability": capability }),
        Capability::Health => {
            serde_json::json!({ "quantities": ["step_count", "heart_rate", "body_mass"] })
        }
        _ => serde_json::json!({}),
    };
    Some((method, args))
}

/// The arguments a native method requires and an honest default for each.
///
/// Measured on the device: every family whose read failed answered `internal` with a message
/// naming the missing key (`Key 'quantity' not found`, `Key 'start_ms' not found`,
/// `Key 'duration_ms' not found`…). The native decoder is strict — that is correct — but a
/// caller that does not know a signature cannot guess it, and `internal` tells it nothing.
///
/// The defaults below are the ones the contract already documents (`LIMIT_DEFAULT`,
/// `PICK_MAX`, `SCAN_MS_MIN`…). They are filled in HERE, in the bridge, so every path (the
/// agent's tool, the shell, a diagnostic) benefits from the same answer.
pub fn complete_args(method: &str, args: &Value, now_ms: f64) -> Value {
    let jour = 24.0 * 3600.0 * 1000.0;
    let depuis = (now_ms - jour).max(0.0) as i64;
    let maintenant = now_ms as i64;
    let defaults = match method {
        "contactsList" => serde_json::json!({ "limit": 50 }),
        "photosPick" => serde_json::json!({ "limit": 10 }),
        "filesPick" => serde_json::json!({ "max": 5 }),
        "nfcReadOnce" => serde_json::json!({ "timeout_ms": 15_000 }),
        // Health is per type: `step_count` is the most common, and `healthStatus` already
        // takes that default.
        // `window_ms` en plus de `quantity` : mesuré, health le réclame aussi. 24 h est le
        // maximum que la famille accepte (`WINDOW_MS_MAX`).
        "healthStatus" | "healthQuantitySample" => {
            // Trois champs, tous mesurés : `quantity`, `window_ms` (24 h, le maximum de la
            // famille) et `limit` (le plus grand que la famille accepte sans tronquer).
            serde_json::json!({ "quantity": "step_count", "window_ms": 86_400_000, "limit": 10 })
        }
        "healthRequest" => {
            serde_json::json!({ "quantities": ["step_count", "heart_rate", "body_mass"] })
        }
        // A window without bounds means "the last 24 hours": what a caller means by
        // "my events" when it does not say.
        "calendarEvents" => serde_json::json!({ "start_ms": depuis, "end_ms": maintenant }),
        "motionStepsSample" => serde_json::json!({ "from_ms": depuis, "to_ms": maintenant }),
        // Camera and location carry their name in their arguments: without it the capability
        // is not even derivable, and the bridge answers `not_supported`.
        "captureRequest" | "captureStatus" => serde_json::json!({ "capability": "camera" }),
        "locationRequest" | "locationSample" => serde_json::json!({ "capability": "location" }),
        "bluetoothScan" => serde_json::json!({ "duration_ms": 3000, "services": [] }),
        "localNetworkBrowse" => serde_json::json!({ "service_type": "_http._tcp" }),
        _ => serde_json::json!({}),
    };
    let mut completed = defaults;
    if let (Some(base), Some(given)) = (completed.as_object_mut(), args.as_object()) {
        for (key, value) in given {
            base.insert(key.clone(), value.clone());
        }
    }
    completed
}

/// The arguments no default can honestly replace: a text to share, a file to transcribe, a
/// bookmark to open, a row to create. `None` means the call is complete.
pub fn missing_args(method: &str, args: &Value) -> Option<&'static str> {
    let required: &[&'static str] = match method {
        "shareText" => &["text"],
        "speechTranscribeFile" => &["path"],
        "filesRead" => &["bookmark"],
        "calendarCreateEvent" => &["title", "start_ms", "end_ms"],
        _ => &[],
    };
    required
        .iter()
        .find(|key| args.get(**key).is_none())
        .copied()
}

/// The native method that reports the permission state of `capability`, if one exists.
///
/// `Some("status")` is the pilot's own status command, which takes the capability as an
/// argument. `None` means the native side has no way to report that state: the shell must
/// answer [`PermissionState::Unsupported`] rather than guess.
pub fn status_method(capability: Capability) -> Option<&'static str> {
    Some(match capability {
        Capability::Gyroscope | Capability::Bluetooth => "status",
        Capability::Notifications => "notificationsStatus",
        Capability::LiveActivity => "liveActivityStatus",
        Capability::Camera | Capability::Microphone => "captureStatus",
        Capability::Contacts => "contactsStatus",
        Capability::Photos => "photosStatus",
        Capability::Calendar => "calendarStatus",
        Capability::Health => "healthStatus",
        Capability::FaceId => "faceIdStatus",
        Capability::Speech => "speechStatus",
        Capability::Nfc => "nfcStatus",
        Capability::LocalNetwork => "localNetworkStatus",
        Capability::MotionActivity => "motionActivityStatus",
        // No state to read: the person decides at each use, so there is nothing to ask.
        Capability::PhotosPick
        | Capability::FilesPick
        | Capability::Clipboard
        | Capability::Share
        | Capability::Location
        | Capability::LocationAlways => return None,
    })
}

/// Lets a request through only when the person has allowed `module` to use the capability
/// it needs. Returns that capability on success; otherwise a [`Reply::Error`] to send back.
///
/// A capability the bridge does not support is refused with
/// [`DeviceError::NotSupported`] before `consent` is consulted.
pub fn gate(module: &str, request: &Request, consent: &dyn Consent) -> Result<Capability, Reply> {
    // Reading the STATE of a permission is not reading data: it is the meta-information the
    // system already gives any app (iOS and Android both answer it without asking again).
    // Sending it through consent had two measured effects: a diagnostic over the 21
    // capabilities asked the person 21 questions, and the queue filled with requests that
    // grant access to nothing. A refusal is learned from the answer (`denied`), not from a
    // permission one would have asked for in order to observe it.
    //
    // Still gated: anything that touches the sensor or the data (a `Call`, the gyroscope, a
    // Bluetooth scan).
    if let Request::Status { capability } = request {
        return if Capability::ALL.contains(capability) {
            Ok(*capability)
        } else {
            Err(refused(DeviceError::NotSupported))
        };
    }
    gate_within(module, request, consent, bridged_for(request))
}

/// The capabilities a request may name: any catalogue entry for a status or a family call
/// (a family method is in the catalogue by construction, [`capability_of`] proves it), the
/// bridged set for the two primitives that touch the device directly.
fn bridged_for(request: &Request) -> &'static [Capability] {
    match request {
        Request::Status { .. } | Request::Call { .. } => EVERY_CAPABILITY,
        Request::GyroscopeSample | Request::BluetoothScan { .. } => &BRIDGED,
    }
}

const EVERY_CAPABILITY: &[Capability] = &Capability::ALL;

/// [`gate`] with the set of supported capabilities made explicit.
fn gate_within(
    module: &str,
    request: &Request,
    consent: &dyn Consent,
    supported: &[Capability],
) -> Result<Capability, Reply> {
    let Some(capability) = required_capability(request) else {
        return Err(refused(DeviceError::NotSupported));
    };
    if !supported.contains(&capability) {
        return Err(refused(DeviceError::NotSupported));
    }
    match consent.decide(module, capability) {
        Decision::Allow => Ok(capability),
        Decision::Ask | Decision::Deny => Err(refused(DeviceError::NotAuthorized)),
    }
}

fn refused(code: DeviceError) -> Reply {
    Reply::Error { code }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::{Value, json};

    use super::*;

    /// A consent that always answers the same, and counts its calls.
    struct Fixed {
        answer: Decision,
        calls: Cell<u32>,
    }

    impl Fixed {
        fn new(answer: Decision) -> Self {
            Self {
                answer,
                calls: Cell::new(0),
            }
        }
    }

    impl Consent for Fixed {
        fn decide(&self, _module: &str, _capability: Capability) -> Decision {
            self.calls.set(self.calls.get() + 1);
            self.answer
        }
    }

    /// The requests that reach the device or its data: all of them are gated.
    fn every_device_request() -> Vec<Request> {
        vec![
            Request::GyroscopeSample,
            Request::BluetoothScan {
                duration_ms: 3000,
                services: vec!["180d".into()],
            },
            call("contactsList", json!({"limit": 5})),
            call("photosPick", json!({"limit": 1})),
        ]
    }

    #[test]
    fn deny_refuses_every_device_request_as_not_authorized() {
        for request in every_device_request() {
            let consent = Fixed::new(Decision::Deny);
            let result = gate("atelier", &request, &consent);
            assert_eq!(
                result,
                Err(Reply::Error {
                    code: DeviceError::NotAuthorized
                }),
                "{request:?}"
            );
            assert_eq!(consent.calls.get(), 1);
        }
    }

    #[test]
    fn ask_is_never_an_implicit_agreement() {
        for request in every_device_request() {
            let consent = Fixed::new(Decision::Ask);
            assert_eq!(
                gate("atelier", &request, &consent),
                Err(Reply::Error {
                    code: DeviceError::NotAuthorized
                }),
                "{request:?}"
            );
        }
    }

    #[test]
    fn allow_returns_the_capability_the_request_needs() {
        let consent = Fixed::new(Decision::Allow);
        assert_eq!(
            gate("atelier", &every_device_request()[0], &consent),
            Ok(Capability::Gyroscope)
        );
        assert_eq!(
            gate("atelier", &every_device_request()[1], &consent),
            Ok(Capability::Bluetooth)
        );
        assert_eq!(
            gate("atelier", &every_device_request()[2], &consent),
            Ok(Capability::Contacts)
        );
        assert_eq!(
            gate("atelier", &every_device_request()[3], &consent),
            Ok(Capability::PhotosPick)
        );
    }

    // La seule requête qui ne demande rien : sans cela, un diagnostic poserait une question
    // par capacité, et remplirait la file de demandes qui ne donnent accès à rien.
    #[test]
    fn reading_a_permission_state_asks_nobody_and_queues_nothing() {
        for capability in Capability::ALL {
            let request = Request::Status { capability };
            for answer in [Decision::Allow, Decision::Ask, Decision::Deny] {
                let consent = Fixed::new(answer);
                assert_eq!(
                    gate("atelier", &request, &consent),
                    Ok(capability),
                    "{capability:?} with {answer:?}"
                );
                assert_eq!(
                    consent.calls.get(),
                    0,
                    "{capability:?} with {answer:?}: consent must not be consulted"
                );
            }
        }
    }

    #[test]
    fn status_is_allowed_for_every_catalogue_capability() {
        for capability in Capability::ALL {
            let request = Request::Status { capability };
            let consent = Fixed::new(Decision::Allow);
            assert_eq!(gate("atelier", &request, &consent), Ok(capability));
        }
    }

    #[test]
    fn requests_that_reach_the_device_are_still_gated_after_the_status_exception() {
        for request in every_device_request() {
            let deny = Fixed::new(Decision::Deny);
            assert_eq!(
                gate("atelier", &request, &deny),
                Err(Reply::Error {
                    code: DeviceError::NotAuthorized
                }),
                "{request:?}"
            );
            assert_eq!(deny.calls.get(), 1, "{request:?} must consult consent");
        }
    }

    #[test]
    fn unsupported_capability_is_refused_before_consent_is_asked() {
        let request = Request::Status {
            capability: Capability::Camera,
        };
        let consent = Fixed::new(Decision::Allow);
        // The bridge supports only the gyroscope and Bluetooth here.
        let result = gate_within("atelier", &request, &consent, &BRIDGED);
        assert_eq!(
            result,
            Err(Reply::Error {
                code: DeviceError::NotSupported
            })
        );
        assert_eq!(consent.calls.get(), 0, "consent must not be consulted");
    }

    #[test]
    fn required_capability_for_each_variant() {
        assert_eq!(
            required_capability(&Request::Status {
                capability: Capability::Camera
            }),
            Some(Capability::Camera)
        );
        assert_eq!(
            required_capability(&Request::GyroscopeSample),
            Some(Capability::Gyroscope)
        );
        assert_eq!(
            required_capability(&Request::BluetoothScan {
                duration_ms: 1,
                services: vec![]
            }),
            Some(Capability::Bluetooth)
        );
        assert_eq!(
            required_capability(&call("contactsList", json!({"limit": 10}))),
            Some(Capability::Contacts)
        );
        assert_eq!(
            required_capability(&call("photosPick", json!({"limit": 1}))),
            Some(Capability::PhotosPick),
            "the picker is not the library"
        );
        assert_eq!(required_capability(&call("noSuchMethod", json!({}))), None);
    }

    fn call(method: &str, args: Value) -> Request {
        Request::Call {
            method: method.into(),
            args,
        }
    }

    // A method the native side grew but the contract did not learn is closed, not guessed.
    #[test]
    fn an_unknown_method_has_no_capability_and_is_refused_before_consent() {
        let consent = Fixed::new(Decision::Allow);
        let result = gate("atelier", &call("harvestContacts", json!({})), &consent);
        assert_eq!(
            result,
            Err(Reply::Error {
                code: DeviceError::NotSupported
            })
        );
        assert_eq!(consent.calls.get(), 0, "consent must not be consulted");
    }

    // The capability comes from the method, so a module cannot be checked against the wrong one.
    #[test]
    fn a_call_is_gated_on_the_capability_of_its_method() {
        let denied = Fixed::new(Decision::Deny);
        assert_eq!(
            gate(
                "atelier",
                &call("contactsList", json!({"limit": 5})),
                &denied
            ),
            Err(Reply::Error {
                code: DeviceError::NotAuthorized
            })
        );
        let allowed = Fixed::new(Decision::Allow);
        assert_eq!(
            gate(
                "atelier",
                &call("contactsList", json!({"limit": 5})),
                &allowed
            ),
            Ok(Capability::Contacts)
        );
        assert_eq!(allowed.calls.get(), 1);
    }

    #[test]
    fn every_known_method_has_a_capability() {
        for method in METHODS {
            let args = if matches!(
                *method,
                "status" | "captureStatus" | "captureRequest" | "locationRequest"
            ) {
                json!({"capability": "camera"})
            } else {
                json!({})
            };
            assert!(
                capability_of(method, &args).is_some(),
                "{method} is listed but maps to no capability"
            );
        }
    }

    #[test]
    fn posting_and_cancelling_a_notification_need_the_notifications_capability() {
        for method in ["notificationsPost", "notificationsCancel"] {
            assert!(METHODS.contains(&method), "{method} n'est pas dans METHODS");
            assert_eq!(
                capability_of(method, &json!({"id": "a", "title": "t", "body": "b"})),
                Some(Capability::Notifications)
            );
        }
        assert_eq!(
            status_method(Capability::Notifications),
            Some("notificationsStatus")
        );
    }

    #[test]
    fn the_capability_reading_methods_refuse_an_unknown_capability_word() {
        for method in ["status", "captureStatus", "captureRequest"] {
            assert_eq!(
                capability_of(method, &json!({"capability": "telepathy"})),
                None
            );
            assert_eq!(capability_of(method, &json!({})), None);
            assert_eq!(
                capability_of(method, &json!({"capability": "microphone"})),
                Some(Capability::Microphone)
            );
        }
    }

    #[test]
    fn a_status_request_is_answered_by_the_method_that_knows_that_capability() {
        for capability in Capability::ALL {
            match status_request(capability) {
                Some(request) => assert_eq!(
                    required_capability(&request),
                    Some(capability),
                    "{capability:?} reads its state through a request about another capability"
                ),
                None => assert!(status_method(capability).is_none(), "{capability:?}"),
            }
        }
    }

    #[test]
    fn a_method_that_needs_arguments_gets_them_completed() {
        let now = 1_700_000_000_000.0;
        for (method, expected) in [
            ("contactsList", "limit"),
            ("photosPick", "limit"),
            ("bluetoothScan", "duration_ms"),
            ("healthQuantitySample", "quantity"),
            ("calendarEvents", "start_ms"),
            ("motionStepsSample", "from_ms"),
            ("localNetworkBrowse", "service_type"),
            ("captureRequest", "capability"),
        ] {
            let args = complete_args(method, &serde_json::json!({}), now);
            assert!(
                args.get(expected).is_some(),
                "{method} should get {expected}"
            );
        }
    }

    #[test]
    fn a_completed_call_derives_its_capability() {
        let now = 1_700_000_000_000.0;
        let args = complete_args("captureRequest", &serde_json::json!({}), now);
        assert_eq!(
            capability_of("captureRequest", &args),
            Some(Capability::Camera)
        );
        let args = complete_args("locationSample", &serde_json::json!({}), now);
        assert_eq!(
            capability_of("locationSample", &args),
            Some(Capability::Location)
        );
    }

    #[test]
    fn a_given_argument_is_never_overwritten() {
        let now = 1_700_000_000_000.0;
        let args = complete_args("contactsList", &serde_json::json!({ "limit": 7 }), now);
        assert_eq!(args, serde_json::json!({ "limit": 7 }));
    }

    #[test]
    fn the_arguments_no_default_can_replace_are_named() {
        let vide = serde_json::json!({});
        assert_eq!(missing_args("shareText", &vide), Some("text"));
        assert_eq!(missing_args("speechTranscribeFile", &vide), Some("path"));
        assert_eq!(missing_args("filesRead", &vide), Some("bookmark"));
        assert_eq!(missing_args("calendarCreateEvent", &vide), Some("title"));
        assert_eq!(missing_args("contactsList", &vide), None);
        assert_eq!(
            missing_args("shareText", &serde_json::json!({ "text": "bonjour" })),
            None
        );
    }

    #[test]
    fn a_permission_request_names_a_method_the_bridge_knows() {
        for capability in Capability::ALL {
            if let Some((method, args)) = permission_request(capability) {
                assert!(
                    METHODS.contains(&method),
                    "{capability:?} asks through {method}, which is not a known method"
                );
                assert_eq!(
                    capability_of(method, &args),
                    Some(capability),
                    "{capability:?} asks through {method}, which is gated on another capability"
                );
            }
        }
    }

    #[test]
    fn the_capabilities_the_system_never_asks_are_exactly_the_expected_ones() {
        for capability in Capability::ALL {
            let asks = permission_request(capability).is_some();
            let expected = !matches!(
                capability,
                Capability::PhotosPick
                    | Capability::FilesPick
                    | Capability::Clipboard
                    | Capability::Share
                    | Capability::FaceId
                    | Capability::Nfc
                    | Capability::LocalNetwork
                    | Capability::Bluetooth
                    | Capability::Gyroscope
                    | Capability::LiveActivity
            );
            assert_eq!(asks, expected, "{capability:?}");
        }
    }

    #[test]
    fn every_capability_has_a_status_decision_and_a_known_method() {
        for capability in Capability::ALL {
            match status_method(capability) {
                Some(method) => assert!(
                    METHODS.contains(&method),
                    "{capability:?} reports through {method}, which is not a known method"
                ),
                None => assert!(
                    matches!(
                        capability,
                        Capability::PhotosPick
                            | Capability::FilesPick
                            | Capability::Clipboard
                            | Capability::Share
                            | Capability::Location
                            | Capability::LocationAlways
                    ),
                    "{capability:?} should report a state"
                ),
            }
        }
    }

    #[test]
    fn a_call_serializes_to_the_wire_shape_the_native_side_receives() {
        assert_eq!(
            serde_json::to_value(call("contactsList", json!({"limit": 10}))).unwrap(),
            json!({"op": "call", "method": "contactsList", "args": {"limit": 10}})
        );
        let back: Request =
            serde_json::from_value(json!({"op": "call", "method": "photosPick", "args": {}}))
                .unwrap();
        assert_eq!(back, call("photosPick", json!({})));
    }

    #[test]
    fn requests_serialize_to_the_expected_json() {
        assert_eq!(
            serde_json::to_value(Request::GyroscopeSample).unwrap(),
            json!({"op": "gyroscope_sample"})
        );
        assert_eq!(
            serde_json::to_value(Request::Status {
                capability: Capability::LiveActivity
            })
            .unwrap(),
            json!({"op": "status", "capability": "live_activity"})
        );
        assert_eq!(
            serde_json::to_value(Request::BluetoothScan {
                duration_ms: 3000,
                services: vec!["180d".into()],
            })
            .unwrap(),
            json!({"op": "bluetooth_scan", "duration_ms": 3000, "services": ["180d"]})
        );
    }

    #[test]
    fn requests_round_trip() {
        let mut every = every_device_request();
        every.push(Request::Status {
            capability: Capability::Health,
        });
        for request in every {
            let value = serde_json::to_value(&request).unwrap();
            let back: Request = serde_json::from_value(value).unwrap();
            assert_eq!(back, request);
        }
    }

    #[test]
    fn replies_serialize_to_the_expected_json() {
        assert_eq!(
            serde_json::to_value(Reply::Status {
                state: PermissionState::Granted
            })
            .unwrap(),
            json!({"kind": "status", "state": "granted"})
        );
        assert_eq!(
            serde_json::to_value(Reply::Status {
                state: PermissionState::Unsupported
            })
            .unwrap(),
            json!({"kind": "status", "state": "unsupported"})
        );
        assert_eq!(
            serde_json::to_value(Reply::Gyroscope {
                x: 0.5,
                y: -0.25,
                z: 0.0,
                ts_ms: 1700
            })
            .unwrap(),
            json!({"kind": "gyroscope", "x": 0.5, "y": -0.25, "z": 0.0, "ts_ms": 1700})
        );
        assert_eq!(
            serde_json::to_value(Reply::Devices {
                devices: vec![
                    BtDevice {
                        id: "A1".into(),
                        name: Some("Sensor".into()),
                        rssi: -60,
                    },
                    BtDevice {
                        id: "B2".into(),
                        name: None,
                        rssi: -90,
                    },
                ],
            })
            .unwrap(),
            json!({
                "kind": "devices",
                "devices": [
                    {"id": "A1", "name": "Sensor", "rssi": -60},
                    {"id": "B2", "name": null, "rssi": -90}
                ]
            })
        );
    }

    #[test]
    fn error_codes_are_snake_case() {
        let expected = [
            (DeviceError::NotAuthorized, "not_authorized"),
            (DeviceError::NotSupported, "not_supported"),
            (DeviceError::Unavailable, "unavailable"),
            (DeviceError::Timeout, "timeout"),
            (DeviceError::Internal, "internal"),
        ];
        for (code, wire) in expected {
            assert_eq!(
                serde_json::to_value(Reply::Error { code }).unwrap(),
                json!({"kind": "error", "code": wire})
            );
        }
    }

    #[test]
    fn replies_round_trip() {
        let replies = [
            Reply::Status {
                state: PermissionState::Prompt,
            },
            Reply::Gyroscope {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                ts_ms: 42,
            },
            Reply::Devices { devices: vec![] },
            Reply::Error {
                code: DeviceError::Timeout,
            },
        ];
        for reply in replies {
            let value: Value = serde_json::to_value(&reply).unwrap();
            let back: Reply = serde_json::from_value(value).unwrap();
            assert_eq!(back, reply);
        }
    }
}
