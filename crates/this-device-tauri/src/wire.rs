//! The pure part of the bridge: the checks applied before the device is touched, and the
//! tests of the answers the native side sends back. No Tauri, no native code: it runs on the
//! host, so the contract is checked without a phone.

use this_platform::device_bridge::{DeviceError, Reply};

/// The shortest scan the native side is asked for. Shorter scans find nothing useful.
pub const SCAN_MS_MIN: u32 = 100;
/// The longest scan. A scan is a sample, not a background listener.
pub const SCAN_MS_MAX: u32 = 30_000;

/// The scan duration, kept in the range the native side accepts. Clamped rather than refused:
/// the contract has no code for a bad duration, and a scan of the nearest bound is what the
/// caller asked for in spirit.
pub fn clamp_duration(duration_ms: u32) -> u32 {
    duration_ms.clamp(SCAN_MS_MIN, SCAN_MS_MAX)
}

/// A service identifier, as CoreBluetooth reads it: a 16-bit or 32-bit short form (4 or 8 hex
/// digits), or a full 128-bit UUID in the 8-4-4-4-12 form. Anything else is refused: an
/// unreadable filter must never turn into a scan of every device.
pub fn is_service_uuid(service: &str) -> bool {
    fn hex(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
    }
    match service.len() {
        4 | 8 => hex(service),
        36 => {
            let parts: Vec<&str> = service.split('-').collect();
            parts.len() == 5
                && [8, 4, 4, 4, 12]
                    .iter()
                    .zip(&parts)
                    .all(|(len, part)| part.len() == *len && hex(part))
        }
        _ => false,
    }
}

/// The decoded length of a base64 string, or `None` when it is not base64 at all.
///
/// Hand-rolled on purpose: the bridge has no `base64` dependency, and the check needed before
/// crossing it is the length and the alphabet, not the bytes.
pub fn base64_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|b| **b == b'=').count();
    if padding > 2 {
        return None;
    }
    let body = &text[..text.len() - padding];
    let alphabet = |b: u8| b.is_ascii_alphanumeric() || b == b'+' || b == b'/';
    if !body.bytes().all(alphabet) {
        return None;
    }
    Some(bytes.len() / 4 * 3 - padding)
}

/// The checked arguments of a Bluetooth scan, or the refusal to send back.
pub fn scan_args(duration_ms: u32, services: &[String]) -> Result<(u32, Vec<String>), Reply> {
    if services.iter().any(|s| !is_service_uuid(s)) {
        return Err(Reply::Error {
            code: DeviceError::Internal,
        });
    }
    Ok((clamp_duration(duration_ms), services.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use this_platform::device_bridge::{BtDevice, PermissionState};

    // What the Swift side sends, byte for byte (see ios/Sources/Reply.swift). The Swift
    // encoder writes `null` for a missing name and integers for `ts_ms`.
    #[test]
    fn gyroscope_sample_decodes_into_the_contract() {
        let reply: Reply = serde_json::from_str(
            r#"{"kind":"gyroscope","x":0.5,"y":-0.25,"z":0.0,"ts_ms":1700000000123}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            Reply::Gyroscope {
                x: 0.5,
                y: -0.25,
                z: 0.0,
                ts_ms: 1_700_000_000_123
            }
        );
    }

    #[test]
    fn devices_decode_with_an_unknown_name_as_none() {
        let reply: Reply = serde_json::from_str(
            r#"{"kind":"devices","devices":[{"id":"A1B2","name":"Sensor","rssi":-60},{"id":"C3","name":null,"rssi":-90}]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            Reply::Devices {
                devices: vec![
                    BtDevice {
                        id: "A1B2".into(),
                        name: Some("Sensor".into()),
                        rssi: -60
                    },
                    BtDevice {
                        id: "C3".into(),
                        name: None,
                        rssi: -90
                    },
                ]
            }
        );
    }

    #[test]
    fn an_empty_scan_is_a_list_not_an_error() {
        let reply: Reply = serde_json::from_str(r#"{"kind":"devices","devices":[]}"#).unwrap();
        assert_eq!(reply, Reply::Devices { devices: vec![] });
    }

    #[test]
    fn every_error_code_the_native_side_sends_is_a_contract_code() {
        for (wire, code) in [
            ("not_authorized", DeviceError::NotAuthorized),
            ("not_supported", DeviceError::NotSupported),
            ("unavailable", DeviceError::Unavailable),
            ("timeout", DeviceError::Timeout),
            ("internal", DeviceError::Internal),
            ("missing_argument", DeviceError::MissingArgument),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: Reply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, Reply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn every_permission_state_the_native_side_sends_is_a_contract_state() {
        for (wire, state) in [
            ("granted", PermissionState::Granted),
            ("denied", PermissionState::Denied),
            ("prompt", PermissionState::Prompt),
            ("limited", PermissionState::Limited),
            ("unsupported", PermissionState::Unsupported),
        ] {
            let json = format!(r#"{{"kind":"status","state":"{wire}"}}"#);
            let reply: Reply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, Reply::Status { state }, "{wire}");
        }
    }

    #[test]
    fn a_native_answer_with_an_unknown_kind_is_refused_not_guessed() {
        assert!(serde_json::from_str::<Reply>(r#"{"kind":"camera","frames":[]}"#).is_err());
    }

    #[test]
    fn a_fractional_timestamp_is_refused_by_the_contract() {
        // serde rejects a float for a u64: the native side must send an integer.
        assert!(
            serde_json::from_str::<Reply>(
                r#"{"kind":"gyroscope","x":0,"y":0,"z":0,"ts_ms":1700000000123.0}"#
            )
            .is_err()
        );
    }

    #[test]
    fn the_capability_argument_is_the_contract_name() {
        // The payload sent to the native `status` method.
        let payload = json!({ "capability": this_platform::capability::Capability::Bluetooth });
        assert_eq!(payload, json!({ "capability": "bluetooth" }));
        let payload = json!({ "capability": this_platform::capability::Capability::Gyroscope });
        assert_eq!(payload, json!({ "capability": "gyroscope" }));
    }

    #[test]
    fn the_scan_duration_is_kept_in_the_native_range() {
        assert_eq!(clamp_duration(0), SCAN_MS_MIN);
        assert_eq!(clamp_duration(1), SCAN_MS_MIN);
        assert_eq!(clamp_duration(3_000), 3_000);
        assert_eq!(clamp_duration(u32::MAX), SCAN_MS_MAX);
    }

    #[test]
    fn service_filters_accept_the_three_uuid_forms() {
        assert!(is_service_uuid("180d"));
        assert!(is_service_uuid("0000180D"));
        assert!(is_service_uuid("0000180d-0000-1000-8000-00805f9b34fb"));
    }

    #[test]
    fn a_malformed_service_filter_is_refused() {
        for bad in [
            "",
            "180",
            "180g",
            "0000180d00001000800000805f9b34fb",
            "0000180d-0000-1000-8000-00805f9b34fz",
            "0000180d_0000_1000_8000_00805f9b34fb",
        ] {
            assert!(!is_service_uuid(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn scan_args_refuse_a_bad_filter_instead_of_scanning_everything() {
        let reply = scan_args(3_000, &["180d".into(), "not-a-uuid".into()]).unwrap_err();
        assert_eq!(
            reply,
            Reply::Error {
                code: DeviceError::Internal
            }
        );
    }

    #[test]
    fn scan_args_keep_the_valid_filter_and_the_clamped_duration() {
        let (duration, services) = scan_args(0, &["180d".into()]).unwrap();
        assert_eq!(duration, SCAN_MS_MIN);
        assert_eq!(services, vec!["180d".to_string()]);
    }
}
