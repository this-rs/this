//! The clipboard and share family: the wire types of the pasteboard (text only) and of the
//! share sheet. Pure: no Tauri, no native code. The native side is
//! `ios/Sources/clipboard_share.swift`, which encodes the answers by hand.
//!
//! CONTENT OF THE PERSON. The pasteboard and the shared text are what the person copied or
//! wrote. The types below carry them, and nothing may log them: the native side logs no content.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::DeviceError;

/// The arguments of `clipboard_write` and `share_text`: the text to put on the pasteboard, or
/// to hand to the share sheet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRequest {
    pub text: String,
}

/// What the native side answers for the clipboard and share commands.
///
/// `Error` reuses the device contract's [`DeviceError`]: a non-text pasteboard is
/// `not_supported`, a missing presenter is `unavailable`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipboardShareReply {
    /// The pasteboard read as text. `text` is `null` when the pasteboard holds nothing.
    ClipboardText {
        text: Option<String>,
    },
    /// The text was written to the pasteboard.
    ClipboardWritten,
    /// The share sheet closed. `completed` is true when the person chose a destination.
    Shared {
        completed: bool,
    },
    Error {
        code: DeviceError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_text_request_is_the_text_field() {
        let value = serde_json::to_value(TextRequest {
            text: "bonjour".into(),
        })
        .unwrap();
        assert_eq!(value, json!({"text": "bonjour"}));
    }

    #[test]
    fn a_text_request_without_text_is_refused() {
        assert!(serde_json::from_str::<TextRequest>(r#"{}"#).is_err());
    }

    #[test]
    fn clipboard_text_decodes_with_its_text() {
        let reply: ClipboardShareReply =
            serde_json::from_str(r#"{"kind":"clipboard_text","text":"bonjour"}"#).unwrap();
        assert_eq!(
            reply,
            ClipboardShareReply::ClipboardText {
                text: Some("bonjour".into())
            }
        );
    }

    #[test]
    fn an_empty_pasteboard_is_a_null_text_not_an_error() {
        let reply: ClipboardShareReply =
            serde_json::from_str(r#"{"kind":"clipboard_text","text":null}"#).unwrap();
        assert_eq!(reply, ClipboardShareReply::ClipboardText { text: None });
    }

    #[test]
    fn clipboard_text_serializes_a_null_text_as_null() {
        assert_eq!(
            serde_json::to_value(ClipboardShareReply::ClipboardText { text: None }).unwrap(),
            json!({"kind": "clipboard_text", "text": null})
        );
    }

    #[test]
    fn a_write_answers_with_its_kind_only() {
        let reply: ClipboardShareReply =
            serde_json::from_str(r#"{"kind":"clipboard_written"}"#).unwrap();
        assert_eq!(reply, ClipboardShareReply::ClipboardWritten);
        assert_eq!(
            serde_json::to_value(ClipboardShareReply::ClipboardWritten).unwrap(),
            json!({"kind": "clipboard_written"})
        );
    }

    #[test]
    fn a_closed_share_sheet_says_whether_a_destination_was_chosen() {
        for completed in [true, false] {
            let wire = format!(r#"{{"kind":"shared","completed":{completed}}}"#);
            let reply: ClipboardShareReply = serde_json::from_str(&wire).unwrap();
            assert_eq!(reply, ClipboardShareReply::Shared { completed });
        }
    }

    #[test]
    fn errors_use_the_device_contract_codes() {
        for (wire, code) in [
            ("not_supported", DeviceError::NotSupported),
            ("unavailable", DeviceError::Unavailable),
            ("internal", DeviceError::Internal),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: ClipboardShareReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, ClipboardShareReply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn the_unsupported_code_is_refused() {
        // `unsupported` is not a device error code: the contract names it `not_supported`.
        // A native answer with it must be refused, not guessed.
        assert!(
            serde_json::from_str::<ClipboardShareReply>(r#"{"kind":"error","code":"unsupported"}"#)
                .is_err()
        );
    }

    #[test]
    fn an_unknown_kind_is_refused_not_guessed() {
        assert!(
            serde_json::from_str::<ClipboardShareReply>(r#"{"kind":"clipboard_image","data":""}"#)
                .is_err()
        );
    }

    #[test]
    fn every_reply_round_trips() {
        let replies = [
            ClipboardShareReply::ClipboardText {
                text: Some("a".into()),
            },
            ClipboardShareReply::ClipboardText { text: None },
            ClipboardShareReply::ClipboardWritten,
            ClipboardShareReply::Shared { completed: false },
            ClipboardShareReply::Error {
                code: DeviceError::NotSupported,
            },
        ];
        for reply in replies {
            let value = serde_json::to_value(&reply).unwrap();
            let back: ClipboardShareReply = serde_json::from_value(value).unwrap();
            assert_eq!(back, reply);
        }
    }
}
