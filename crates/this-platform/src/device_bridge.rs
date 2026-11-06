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
    Internal,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            DeviceError::NotAuthorized => "not authorized",
            DeviceError::NotSupported => "not supported",
            DeviceError::Unavailable => "unavailable",
            DeviceError::Timeout => "timeout",
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
        "status" | "captureStatus" | "captureRequest" => return declared_capability(args),
        "gyroscopeSample" => Capability::Gyroscope,
        "bluetoothScan" => Capability::Bluetooth,
        "notificationsStatus" | "notificationsRequest" => Capability::Notifications,
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
        "locationRequest" | "locationSample" => Capability::Location,
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
        Capability::Photos | Capability::PhotosPick => "photosStatus",
        Capability::Calendar => "calendarStatus",
        Capability::Health => "healthStatus",
        Capability::FaceId => "faceIdStatus",
        Capability::Speech => "speechStatus",
        Capability::Nfc => "nfcStatus",
        Capability::LocalNetwork => "localNetworkStatus",
        Capability::MotionActivity => "motionActivityStatus",
        Capability::Location
        | Capability::LocationAlways
        | Capability::Clipboard
        | Capability::Share
        | Capability::FilesPick => return None,
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
            let args = if matches!(*method, "status" | "captureStatus" | "captureRequest") {
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
                        Capability::Location
                            | Capability::LocationAlways
                            | Capability::Clipboard
                            | Capability::Share
                            | Capability::FilesPick
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
