//! NFC, read side: the wire types of the NDEF read and the pure checks applied before the
//! reader session is opened. No Tauri, no native code: it runs on the host.
//!
//! The native side (`ios/Sources/nfc.swift`) answers with one JSON object per call:
//!
//! ```text
//! {"kind":"status","state":"granted"}
//! {"kind":"ndef","records":[{"tnf":"well_known","type":"T","payload_text":"Hello","payload_base64":"..."}]}
//! {"kind":"error","code":"timeout"}
//! ```
//!
//! Reading only: no NFC write exists in this module.

use serde::{Deserialize, Serialize};

use this_platform::device_bridge::PermissionState;

/// The read timeout when the caller does not ask for one.
pub const NFC_READ_MS_DEFAULT: u32 = 15_000;
/// The shortest read the native side is asked for. A shorter session cannot reach a tag.
pub const NFC_READ_MS_MIN: u32 = 1_000;
/// The longest read: a single one-shot session, never a background listener.
pub const NFC_READ_MS_MAX: u32 = 15_000;

/// The read timeout, kept in the range the native side accepts. Clamped rather than refused,
/// like the Bluetooth scan duration.
pub fn read_timeout_ms(requested: Option<u32>) -> u32 {
    requested
        .unwrap_or(NFC_READ_MS_DEFAULT)
        .clamp(NFC_READ_MS_MIN, NFC_READ_MS_MAX)
}

/// Why a read failed. A user cancel is not an authorization failure, so it has its own code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NfcErrorCode {
    /// The device has no NFC reader, or the system cannot read now (busy, unsupported).
    Unavailable,
    /// No tag was read before the timeout.
    Timeout,
    /// The person dismissed the reader sheet.
    Cancelled,
    /// Something went wrong on the native side (for example, a missing entitlement).
    Internal,
}

/// One NDEF record of a tag, as the native side encodes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NdefRecord {
    /// The type name format: `well_known`, `media`, `absolute_uri`, `external`, `empty`,
    /// `unknown` or `unchanged`.
    pub tnf: String,
    /// The record type as text (`T` for text, `U` for URI, a MIME type...), or `None` when
    /// the type bytes are not UTF-8.
    #[serde(rename = "type")]
    pub record_type: Option<String>,
    /// The payload as text, when the record is a well-known text or URI record; otherwise `None`.
    pub payload_text: Option<String>,
    /// The raw payload, standard base64. Always present.
    pub payload_base64: String,
}

/// The answer of an NFC call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NfcReply {
    /// `granted` when a reader exists (NFC has no permission to query before a session: the
    /// system asks when the first session begins), `unsupported` otherwise.
    Status {
        state: PermissionState,
    },
    /// The records of the first NDEF message read. An empty list is a tag with no records.
    Ndef {
        records: Vec<NdefRecord>,
    },
    Error {
        code: NfcErrorCode,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_decodes_granted_and_unsupported() {
        assert_eq!(
            serde_json::from_str::<NfcReply>(r#"{"kind":"status","state":"granted"}"#).unwrap(),
            NfcReply::Status {
                state: PermissionState::Granted
            }
        );
        assert_eq!(
            serde_json::from_str::<NfcReply>(r#"{"kind":"status","state":"unsupported"}"#).unwrap(),
            NfcReply::Status {
                state: PermissionState::Unsupported
            }
        );
    }

    #[test]
    fn a_text_record_decodes_with_its_payload() {
        let reply: NfcReply = serde_json::from_str(
            r#"{"kind":"ndef","records":[{"tnf":"well_known","type":"T","payload_text":"Hello","payload_base64":"AGVuSGVsbG8="}]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            NfcReply::Ndef {
                records: vec![NdefRecord {
                    tnf: "well_known".into(),
                    record_type: Some("T".into()),
                    payload_text: Some("Hello".into()),
                    payload_base64: "AGVuSGVsbG8=".into(),
                }]
            }
        );
    }

    #[test]
    fn a_binary_record_decodes_with_null_type_and_text() {
        let reply: NfcReply = serde_json::from_str(
            r#"{"kind":"ndef","records":[{"tnf":"unknown","type":null,"payload_text":null,"payload_base64":"/w=="}]}"#,
        )
        .unwrap();
        let NfcReply::Ndef { records } = reply else {
            panic!("expected an ndef reply");
        };
        assert_eq!(records[0].record_type, None);
        assert_eq!(records[0].payload_text, None);
        assert_eq!(records[0].payload_base64, "/w==");
    }

    #[test]
    fn a_tag_without_records_is_an_empty_list_not_an_error() {
        let reply: NfcReply = serde_json::from_str(r#"{"kind":"ndef","records":[]}"#).unwrap();
        assert_eq!(reply, NfcReply::Ndef { records: vec![] });
    }

    #[test]
    fn every_error_code_the_native_side_sends_is_known() {
        for (wire, code) in [
            ("unavailable", NfcErrorCode::Unavailable),
            ("timeout", NfcErrorCode::Timeout),
            ("cancelled", NfcErrorCode::Cancelled),
            ("internal", NfcErrorCode::Internal),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: NfcReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, NfcReply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn an_unknown_kind_or_code_is_refused_not_guessed() {
        assert!(serde_json::from_str::<NfcReply>(r#"{"kind":"write","ok":true}"#).is_err());
        assert!(serde_json::from_str::<NfcReply>(r#"{"kind":"error","code":"busy"}"#).is_err());
    }

    #[test]
    fn a_record_without_its_payload_is_refused() {
        assert!(
            serde_json::from_str::<NfcReply>(
                r#"{"kind":"ndef","records":[{"tnf":"empty","type":null,"payload_text":null}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn the_read_timeout_defaults_to_the_bound() {
        assert_eq!(read_timeout_ms(None), NFC_READ_MS_DEFAULT);
        assert_eq!(NFC_READ_MS_DEFAULT, NFC_READ_MS_MAX);
    }

    #[test]
    fn the_read_timeout_is_clamped_to_the_native_range() {
        assert_eq!(read_timeout_ms(Some(0)), NFC_READ_MS_MIN);
        assert_eq!(read_timeout_ms(Some(999)), NFC_READ_MS_MIN);
        assert_eq!(read_timeout_ms(Some(5_000)), 5_000);
        assert_eq!(read_timeout_ms(Some(60_000)), NFC_READ_MS_MAX);
        assert_eq!(read_timeout_ms(Some(u32::MAX)), NFC_READ_MS_MAX);
    }

    #[test]
    fn a_null_record_type_and_text_serialize_as_null_keys() {
        let value = serde_json::to_value(NfcReply::Ndef {
            records: vec![NdefRecord {
                tnf: "empty".into(),
                record_type: None,
                payload_text: None,
                payload_base64: String::new(),
            }],
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "ndef",
                "records": [{"tnf": "empty", "type": null, "payload_text": null, "payload_base64": ""}]
            })
        );
    }
}
