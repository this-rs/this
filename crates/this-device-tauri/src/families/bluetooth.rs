//! The Bluetooth family beyond the scan: the radio's state, a connection, and what a
//! connection makes possible (services, read, write, notifications).
//!
//! Pure: no Tauri, no native code. The native side is `ios/Sources/BluetoothCentral.swift`
//! (the CoreBluetooth engine) and `ios/Sources/Bluetooth.swift` (the selectors). This module
//! owns the wire shapes and the checks applied **before** the radio is touched, so a bad
//! argument is a refusal here rather than an unknown selector or an `internal` on the phone.
//!
//! Wire format of every answer (one JSON object, a `kind` discriminator):
//!
//! ```text
//! {"kind":"state","state":"powered_on"}
//! {"kind":"device","id":"<uuid>","name":"Sensor","state":"connected"}
//! {"kind":"services","services":[{"uuid":"180D","characteristics":[{"uuid":"2A37","properties":["read","notify"]}]}]}
//! {"kind":"value","characteristic":"2A37","data_base64":"AQI="}
//! {"kind":"written","bytes":2}
//! {"kind":"subscribed","characteristic":"2A37"}
//! {"kind":"notifications","notifications":[{"characteristic":"2A37","data_base64":"AQI=","ts_ms":1700000000000}]}
//! {"kind":"error","code":"not_supported"}
//! ```
//!
//! ## Why the identifier is the one the scan returned
//!
//! CoreBluetooth does not expose a MAC address, and never will (Apple's privacy rule). A
//! peripheral is addressed by the UUID `identifier` the system gives it, which is stable for
//! one app on one phone, and not across devices. So `bluetoothConnect` takes the `id` of a
//! device a scan returned — nothing else can name a peripheral.
//!
//! ## Why notifications are pulled
//!
//! Subscribing turns the stream on inside the engine, which keeps the last values in a small
//! ring. `bluetoothNotifications` drains it. A push would have to cross from Swift to the
//! webview as a Tauri event, which this bridge does not do yet: a pull that says what it is
//! beats a promise of push that would silently drop values.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wire::{base64_len, is_service_uuid};

/// The shortest connection timeout. Below, the radio has not even answered yet.
pub const CONNECT_MS_MIN: u32 = 1_000;
/// The longest connection timeout. The bridge's own native timeout is 10 s; this bound leaves
/// room for one honest answer instead of a puzzled caller.
pub const CONNECT_MS_MAX: u32 = 15_000;

/// The largest write one call may carry. An ATT packet is 20 bytes by default, 512 at most;
/// a longer payload is a chunking problem for the caller, not something to truncate here.
pub const WRITE_MAX_BYTES: usize = 512;

/// How many notifications the engine keeps per characteristic before the oldest is dropped.
pub const RING_MAX: usize = 32;

/// The connection timeout, kept in the range the native side accepts.
pub fn clamp_connect(timeout_ms: u32) -> u32 {
    timeout_ms.clamp(CONNECT_MS_MIN, CONNECT_MS_MAX)
}

/// A peripheral identifier: the 128-bit UUID form CoreBluetooth gives (`8-4-4-4-12`).
///
/// Refused otherwise: a short form would be a service UUID, not a peripheral, and connecting
/// to something the caller invented would hide a typo as `unavailable`.
pub fn is_peripheral_id(id: &str) -> bool {
    is_service_uuid(id) && id.len() == 36
}

/// A characteristic identifier: short form (4 or 8 hex digits) or a full 128-bit UUID.
pub fn is_characteristic(c: &str) -> bool {
    is_service_uuid(c)
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)?.as_str().filter(|s| !s.trim().is_empty())
}

fn refusal(code: BluetoothError) -> BluetoothReply {
    BluetoothReply::Error { code }
}

/// The checked arguments of `bluetoothConnect`: the peripheral, and a bounded timeout.
pub fn checked_connect(args: &Value) -> Result<(String, u32), BluetoothReply> {
    let Some(id) = text_arg(args, "id") else {
        return Err(refusal(BluetoothError::MissingArgument));
    };
    if !is_peripheral_id(id) {
        return Err(refusal(BluetoothError::NotSupported));
    }
    let timeout = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .map(|ms| clamp_connect(ms as u32))
        .unwrap_or(CONNECT_MS_MIN);
    Ok((id.to_ascii_uppercase(), timeout))
}

/// The checked argument of a call that names only a peripheral.
pub fn checked_peripheral(args: &Value) -> Result<String, BluetoothReply> {
    let Some(id) = text_arg(args, "id") else {
        return Err(refusal(BluetoothError::MissingArgument));
    };
    if !is_peripheral_id(id) {
        return Err(refusal(BluetoothError::NotSupported));
    }
    Ok(id.to_ascii_uppercase())
}

/// The refusal a call must get, if any, before the radio is touched.
///
/// One entry point, called by the bridge for every `Request::Call`: a bound that only lives in
/// a unit test bounds nothing.
pub fn check(method: &str, args: &Value) -> Option<BluetoothReply> {
    match method {
        "bluetoothConnect" => checked_connect(args).err(),
        "bluetoothDisconnect" | "bluetoothServices" => checked_peripheral(args).err(),
        "bluetoothRead" | "bluetoothSubscribe" | "bluetoothNotifications" => {
            checked_characteristic(args).err()
        }
        "bluetoothWrite" => checked_write(args).err(),
        _ => None,
    }
}

/// The checked arguments of a call that names one characteristic.
pub fn checked_characteristic(args: &Value) -> Result<(String, String), BluetoothReply> {
    let Some(id) = text_arg(args, "id") else {
        return Err(refusal(BluetoothError::MissingArgument));
    };
    if !is_peripheral_id(id) {
        return Err(refusal(BluetoothError::NotSupported));
    }
    let Some(characteristic) = text_arg(args, "characteristic") else {
        return Err(refusal(BluetoothError::MissingArgument));
    };
    if !is_characteristic(characteristic) {
        return Err(refusal(BluetoothError::NotSupported));
    }
    Ok((id.to_ascii_uppercase(), characteristic.to_ascii_uppercase()))
}

/// The checked arguments of `bluetoothWrite`: the characteristic, and at most 512 bytes.
///
/// The payload crosses as base64 so it survives JSON; the byte count is what this side checks,
/// because a truncated Bluetooth write is a silent corruption, not an error the device reports.
pub fn checked_write(args: &Value) -> Result<(String, String, usize), BluetoothReply> {
    let (id, characteristic) = checked_characteristic(args)?;
    let Some(data) = text_arg(args, "data_base64") else {
        return Err(refusal(BluetoothError::MissingArgument));
    };
    let Some(bytes) = base64_len(data) else {
        return Err(refusal(BluetoothError::BadBase64));
    };
    if bytes > WRITE_MAX_BYTES {
        return Err(refusal(BluetoothError::TooLarge));
    }
    Ok((id, characteristic, bytes))
}

/// The state of the radio itself, as CoreBluetooth reports it. Distinct from the permission:
/// a phone with Bluetooth switched off is `powered_off`, not `not_authorized`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RadioState {
    PoweredOn,
    PoweredOff,
    Unauthorized,
    Unsupported,
    Resetting,
    Unknown,
}

/// Whether the peripheral answers right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    Connected,
    Disconnected,
}

/// What a characteristic can do. A property the native side invents is refused, not ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    Read,
    Write,
    WriteWithoutResponse,
    Notify,
    Indicate,
}

/// One characteristic of a service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Characteristic {
    pub uuid: String,
    pub properties: Vec<Property>,
}

/// One service, with its characteristics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub uuid: String,
    pub characteristics: Vec<Characteristic>,
}

/// One value pushed by a subscribed characteristic, as the engine buffered it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    pub characteristic: String,
    pub data_base64: String,
    pub ts_ms: u64,
}

/// Why a Bluetooth call was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BluetoothError {
    /// The peripheral is not one this app knows: not scanned, or already forgotten.
    NotSupported,
    /// A required argument is absent, and no honest default can replace it.
    MissingArgument,
    /// Nowhere to connect: the radio is off, or the peripheral is out of reach.
    Unavailable,
    /// The device did not answer in time.
    Timeout,
    /// The payload is bigger than one write may carry ([`WRITE_MAX_BYTES`]).
    TooLarge,
    /// The payload is not base64.
    BadBase64,
    /// Something went wrong on the shell side.
    Internal,
}

/// Every answer of the family, tagged by `kind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BluetoothReply {
    State {
        state: RadioState,
    },
    Device {
        id: String,
        name: Option<String>,
        state: LinkState,
    },
    Services {
        services: Vec<Service>,
    },
    Value {
        characteristic: String,
        data_base64: String,
    },
    Written {
        bytes: usize,
    },
    Subscribed {
        characteristic: String,
    },
    Notifications {
        notifications: Vec<Notification>,
    },
    Error {
        code: BluetoothError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_uuid_is_a_peripheral_only_in_the_full_form() {
        assert!(is_peripheral_id("6F2A9C1E-3B4D-4E5F-8A9B-0C1D2E3F4A5B"));
        // Short forms are service UUIDs, not peripherals.
        assert!(!is_peripheral_id("180D"));
        assert!(!is_peripheral_id("180D0000"));
        // Not a UUID at all.
        assert!(!is_peripheral_id("sensor-1"));
        assert!(!is_peripheral_id(""));
    }

    #[test]
    fn a_characteristic_accepts_every_form_corebluetooth_reads() {
        for ok in ["2A37", "2A370000", "6F2A9C1E-3B4D-4E5F-8A9B-0C1D2E3F4A5B"] {
            assert!(is_characteristic(ok), "{ok}");
        }
        for ko in ["", "2A3", "ZZZZ", "6F2A9C1E3B4D4E5F8A9B0C1D2E3F4A5B"] {
            assert!(!is_characteristic(ko), "{ko}");
        }
    }

    #[test]
    fn a_connect_without_an_id_is_a_missing_argument() {
        assert_eq!(
            checked_connect(&json!({})),
            Err(refusal(BluetoothError::MissingArgument))
        );
        assert_eq!(
            checked_connect(&json!({"id": "   "})),
            Err(refusal(BluetoothError::MissingArgument))
        );
    }

    #[test]
    fn a_made_up_identifier_is_refused_not_attempted() {
        // Un identifiant inventé ne doit pas devenir `unavailable` sur l'appareil : c'est une
        // faute d'appel, pas une panne de la radio.
        assert_eq!(
            checked_connect(&json!({"id": "sensor-1"})),
            Err(refusal(BluetoothError::NotSupported))
        );
    }

    #[test]
    fn a_connect_keeps_its_timeout_in_the_native_range() {
        let id = "6F2A9C1E-3B4D-4E5F-8A9B-0C1D2E3F4A5B";
        for (donne, attendu) in [
            (1_u32, CONNECT_MS_MIN),
            (7_000, 7_000),
            (60_000, CONNECT_MS_MAX),
        ] {
            let (_, timeout) = checked_connect(&json!({"id": id, "timeout_ms": donne})).unwrap();
            assert_eq!(timeout, attendu, "{donne}");
        }
        // Sans timeout : la borne basse, pas zéro (zéro serait « n'attends pas »).
        let (_, defaut) = checked_connect(&json!({"id": id})).unwrap();
        assert_eq!(defaut, CONNECT_MS_MIN);
    }

    #[test]
    fn the_identifier_is_upper_cased_once_and_for_all() {
        // CoreBluetooth rend des UUID en majuscules ; deux écritures du même identifiant ne
        // doivent pas désigner deux périphériques.
        let (id, _) =
            checked_connect(&json!({"id": "6f2a9c1e-3b4d-4e5f-8a9b-0c1d2e3f4a5b"})).unwrap();
        assert_eq!(id, "6F2A9C1E-3B4D-4E5F-8A9B-0C1D2E3F4A5B");
    }

    #[test]
    fn base64_is_checked_by_length_and_alphabet() {
        assert_eq!(base64_len("AQI="), Some(2));
        assert_eq!(base64_len("AQID"), Some(3));
        assert_eq!(base64_len(""), None);
        assert_eq!(base64_len("AQI"), None, "longueur non multiple de 4");
        assert_eq!(base64_len("AQ=D"), None, "padding au milieu");
        assert_eq!(base64_len("A==="), None, "trois octets de padding");
        assert_eq!(base64_len("AQI€"), None, "hors alphabet");
    }

    #[test]
    fn a_write_above_the_bound_is_refused_before_it_is_sent() {
        let id = "6F2A9C1E-3B4D-4E5F-8A9B-0C1D2E3F4A5B";
        // 513 octets utiles → 688 caractères avec padding.
        let gros = "A".repeat(684) + "BAAA";
        let refus = checked_write(&json!({
            "id": id,
            "characteristic": "2A37",
            "data_base64": gros
        }));
        assert_eq!(refus, Err(refusal(BluetoothError::TooLarge)));

        let ok = checked_write(&json!({
            "id": id,
            "characteristic": "2A37",
            "data_base64": "AQI="
        }))
        .unwrap();
        assert_eq!(ok, (id.to_string(), "2A37".to_string(), 2));
    }

    #[test]
    fn a_write_needs_a_characteristic_and_a_payload() {
        let id = "6F2A9C1E-3B4D-4E5F-8A9B-0C1D2E3F4A5B";
        for args in [
            json!({"id": id}),
            json!({"id": id, "characteristic": "2A37"}),
            json!({"id": id, "characteristic": "2A37", "data_base64": "pas du base64"}),
        ] {
            let refus = checked_write(&args).unwrap_err();
            assert!(
                matches!(
                    refus,
                    BluetoothReply::Error {
                        code: BluetoothError::MissingArgument | BluetoothError::BadBase64
                    }
                ),
                "{args} → {refus:?}"
            );
        }
    }

    // What the Swift side sends, byte for byte. A shape that drifts is a silent `internal`.
    #[test]
    fn every_answer_of_the_native_side_decodes() {
        let cas: Vec<(&str, BluetoothReply)> = vec![
            (
                r#"{"kind":"state","state":"powered_on"}"#,
                BluetoothReply::State {
                    state: RadioState::PoweredOn,
                },
            ),
            (
                r#"{"kind":"device","id":"A1","name":null,"state":"connected"}"#,
                BluetoothReply::Device {
                    id: "A1".into(),
                    name: None,
                    state: LinkState::Connected,
                },
            ),
            (
                r#"{"kind":"services","services":[{"uuid":"180D","characteristics":[{"uuid":"2A37","properties":["read","notify"]}]}]}"#,
                BluetoothReply::Services {
                    services: vec![Service {
                        uuid: "180D".into(),
                        characteristics: vec![Characteristic {
                            uuid: "2A37".into(),
                            properties: vec![Property::Read, Property::Notify],
                        }],
                    }],
                },
            ),
            (
                r#"{"kind":"value","characteristic":"2A37","data_base64":"AQI="}"#,
                BluetoothReply::Value {
                    characteristic: "2A37".into(),
                    data_base64: "AQI=".into(),
                },
            ),
            (
                r#"{"kind":"written","bytes":2}"#,
                BluetoothReply::Written { bytes: 2 },
            ),
            (
                r#"{"kind":"subscribed","characteristic":"2A37"}"#,
                BluetoothReply::Subscribed {
                    characteristic: "2A37".into(),
                },
            ),
            (
                r#"{"kind":"notifications","notifications":[{"characteristic":"2A37","data_base64":"AQI=","ts_ms":1700000000000}]}"#,
                BluetoothReply::Notifications {
                    notifications: vec![Notification {
                        characteristic: "2A37".into(),
                        data_base64: "AQI=".into(),
                        ts_ms: 1_700_000_000_000,
                    }],
                },
            ),
            (
                r#"{"kind":"error","code":"unavailable"}"#,
                BluetoothReply::Error {
                    code: BluetoothError::Unavailable,
                },
            ),
        ];
        for (brut, attendu) in cas {
            let lu: BluetoothReply =
                serde_json::from_str(brut).unwrap_or_else(|e| panic!("{brut}: {e}"));
            assert_eq!(lu, attendu, "{brut}");
        }
    }

    #[test]
    fn a_property_the_native_side_invents_is_refused() {
        let brut = r#"{"kind":"services","services":[{"uuid":"180D","characteristics":[{"uuid":"2A37","properties":["teleport"]}]}]}"#;
        assert!(serde_json::from_str::<BluetoothReply>(brut).is_err());
    }

    #[test]
    fn every_error_code_is_readable_by_the_caller() {
        for (code, texte) in [
            (BluetoothError::NotSupported, "not_supported"),
            (BluetoothError::MissingArgument, "missing_argument"),
            (BluetoothError::Unavailable, "unavailable"),
            (BluetoothError::Timeout, "timeout"),
            (BluetoothError::TooLarge, "too_large"),
            (BluetoothError::BadBase64, "bad_base64"),
            (BluetoothError::Internal, "internal"),
        ] {
            let valeur = serde_json::to_value(BluetoothReply::Error { code }).unwrap();
            assert_eq!(valeur, json!({"kind":"error","code":texte}));
        }
    }
}
