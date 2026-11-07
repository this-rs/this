//! The clipboard and share family: the pasteboard (text, link, image) and the system share
//! sheet (text, link, image, file). Pure: no Tauri, no native code. The native side is
//! `ios/Sources/clipboard_share.swift`, which encodes the answers by hand.
//!
//! CONTENT OF THE PERSON. The pasteboard and what is shared are what the person copied or
//! wrote. The types below carry them, and nothing may log them: the native side logs no content.
//!
//! ## Why one payload, named
//!
//! A write and a share carry exactly ONE thing: the text, the link, the image, or the file.
//! Two at once is not ambiguous in JSON, but it is in an answer — the caller would have to
//! guess which one the person saw. So the family refuses it (`not_supported`) instead of
//! choosing, and an empty call is `missing_argument`.
//!
//! ## Why the image is bounded here
//!
//! A screenshot is a few megabytes of PNG; its base64 is a third more, and it would cross the
//! bridge as one JSON string. Above [`IMAGE_MAX_BYTES`] the answer is `too_large` — a caller can
//! downscale, and the bridge will not pretend it carried something it dropped.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::DeviceError;

use crate::wire::base64_len;

/// The largest image one call may carry, decoded. A 4K screenshot fits; a RAW file does not
/// belong on a pasteboard.
pub const IMAGE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The arguments of `share_text`: the text to hand to the share sheet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRequest {
    pub text: String,
}

/// What a write or a share carries, exactly one of the four.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Text(String),
    Url(String),
    /// The image, still base64: the native side decodes it (`Data(base64Encoded:)`).
    Image(String),
    /// A path inside the app's container (a recording, a produced document).
    File(String),
}

impl Payload {
    /// The wire field this payload came from.
    pub fn field(&self) -> &'static str {
        match self {
            Payload::Text(_) => "text",
            Payload::Url(_) => "url",
            Payload::Image(_) => "image_base64",
            Payload::File(_) => "path",
        }
    }
}

/// Why a clipboard or share call was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardError {
    /// No payload, or more than one: the caller must say which.
    MissingArgument,
    /// Nothing to share: the sheet has no presenter (the webview is not on screen).
    Unavailable,
    /// The image is above [`IMAGE_MAX_BYTES`].
    TooLarge,
    /// The image is not base64.
    BadBase64,
    /// The pasteboard holds something this family does not read.
    NotSupported,
    Internal,
}

/// What the native side answers for the clipboard and share commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipboardShareReply {
    /// The pasteboard, as whatever it holds. Every field is `null` when empty.
    Clipboard {
        text: Option<String>,
        url: Option<String>,
        image_base64: Option<String>,
        bytes: u64,
    },
    /// The payload was written to the pasteboard.
    ClipboardWritten,
    /// The share sheet closed. `completed` is true when the person chose a destination.
    Shared {
        completed: bool,
    },
    Error {
        code: ClipboardError,
    },
}

fn field<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key)?
        .as_str()
        .filter(|value| !value.trim().is_empty())
}

/// The one payload a write or a share carries, or the refusal to send back.
pub fn checked_payload(args: &serde_json::Value) -> Result<Payload, ClipboardShareReply> {
    let presents = ["text", "url", "image_base64", "path"]
        .into_iter()
        .filter_map(|key| field(args, key).map(|value| (key, value)))
        .collect::<Vec<_>>();
    let refus = |code| ClipboardShareReply::Error { code };
    match presents.as_slice() {
        [] => Err(refus(ClipboardError::MissingArgument)),
        // Deux contenus à la fois : le pont ne choisit pas à la place de la personne.
        [_, _, ..] => Err(refus(ClipboardError::NotSupported)),
        [("text", value)] => Ok(Payload::Text((*value).to_string())),
        [("url", value)] => Ok(Payload::Url((*value).to_string())),
        [("path", value)] => Ok(Payload::File((*value).to_string())),
        [("image_base64", value)] => {
            let Some(bytes) = base64_len(value) else {
                return Err(refus(ClipboardError::BadBase64));
            };
            if bytes > IMAGE_MAX_BYTES {
                return Err(refus(ClipboardError::TooLarge));
            }
            Ok(Payload::Image((*value).to_string()))
        }
        _ => Err(refus(ClipboardError::MissingArgument)),
    }
}

/// The refusal a clipboard or share call must get, if any, before the native side is touched.
pub fn check(method: &str, args: &serde_json::Value) -> Option<ClipboardShareReply> {
    match method {
        "clipboardWrite" | "shareItem" => checked_payload(args).err(),
        _ => None,
    }
}

/// The device error code of a family refusal, for the callers that map back to the contract.
pub fn device_code(code: ClipboardError) -> DeviceError {
    match code {
        ClipboardError::MissingArgument => DeviceError::MissingArgument,
        ClipboardError::Unavailable => DeviceError::Unavailable,
        ClipboardError::NotSupported => DeviceError::NotSupported,
        ClipboardError::TooLarge | ClipboardError::BadBase64 | ClipboardError::Internal => {
            DeviceError::Internal
        }
    }
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
    fn a_full_pasteboard_decodes_with_every_field() {
        let reply: ClipboardShareReply = serde_json::from_str(
            r#"{"kind":"clipboard","text":"bonjour","url":"https://exemple.fr","image_base64":"iVBORw0KGgo=","bytes":12000}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            ClipboardShareReply::Clipboard {
                text: Some("bonjour".into()),
                url: Some("https://exemple.fr".into()),
                image_base64: Some("iVBORw0KGgo=".into()),
                bytes: 12_000,
            }
        );
    }

    #[test]
    fn an_empty_pasteboard_is_a_null_everything_not_an_error() {
        let reply: ClipboardShareReply = serde_json::from_str(
            r#"{"kind":"clipboard","text":null,"url":null,"image_base64":null,"bytes":0}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            ClipboardShareReply::Clipboard {
                text: None,
                url: None,
                image_base64: None,
                bytes: 0,
            }
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
    fn errors_use_the_family_codes() {
        for (wire, code) in [
            ("not_supported", ClipboardError::NotSupported),
            ("missing_argument", ClipboardError::MissingArgument),
            ("unavailable", ClipboardError::Unavailable),
            ("too_large", ClipboardError::TooLarge),
            ("bad_base64", ClipboardError::BadBase64),
            ("internal", ClipboardError::Internal),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: ClipboardShareReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, ClipboardShareReply::Error { code }, "{wire}");
        }
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
            ClipboardShareReply::Clipboard {
                text: Some("a".into()),
                url: None,
                image_base64: None,
                bytes: 0,
            },
            ClipboardShareReply::ClipboardWritten,
            ClipboardShareReply::Shared { completed: false },
            ClipboardShareReply::Error {
                code: ClipboardError::NotSupported,
            },
        ];
        for reply in replies {
            let value = serde_json::to_value(&reply).unwrap();
            let back: ClipboardShareReply = serde_json::from_value(value).unwrap();
            assert_eq!(back, reply);
        }
    }

    // ---- Ce qu'une écriture et un partage acceptent -------------------------------------

    #[test]
    fn exactly_one_payload_is_accepted() {
        assert_eq!(
            checked_payload(&json!({"text": "bonjour"})),
            Ok(Payload::Text("bonjour".into()))
        );
        assert_eq!(
            checked_payload(&json!({"url": "https://exemple.fr"})),
            Ok(Payload::Url("https://exemple.fr".into()))
        );
        assert_eq!(
            checked_payload(&json!({"path": "/var/tmp/note.m4a"})),
            Ok(Payload::File("/var/tmp/note.m4a".into()))
        );
        assert_eq!(
            checked_payload(&json!({"image_base64": "iVBORw0KGgo="})),
            Ok(Payload::Image("iVBORw0KGgo=".into()))
        );
    }

    #[test]
    fn an_empty_call_is_a_missing_argument() {
        for args in [json!({}), json!({"text": "   "}), json!({"text": null})] {
            assert_eq!(
                checked_payload(&args),
                Err(ClipboardShareReply::Error {
                    code: ClipboardError::MissingArgument
                }),
                "{args}"
            );
        }
    }

    #[test]
    fn two_payloads_at_once_are_refused() {
        // Le pont ne choisit pas à la place de la personne : deux contenus, un refus.
        let args = json!({"text": "bonjour", "image_base64": "iVBORw0KGgo="});
        assert_eq!(
            checked_payload(&args),
            Err(ClipboardShareReply::Error {
                code: ClipboardError::NotSupported
            })
        );
    }

    #[test]
    fn an_image_above_the_bound_is_refused_before_it_crosses() {
        // 4 Mio utiles → 5 592 408 caractères : au-delà de IMAGE_MAX_BYTES.
        let gros = "A".repeat(5_592_404) + "AAAA";
        assert_eq!(
            checked_payload(&json!({ "image_base64": gros })),
            Err(ClipboardShareReply::Error {
                code: ClipboardError::TooLarge
            })
        );
    }

    #[test]
    fn an_image_that_is_not_base64_is_refused() {
        assert_eq!(
            checked_payload(&json!({"image_base64": "pas du tout du base64"})),
            Err(ClipboardShareReply::Error {
                code: ClipboardError::BadBase64
            })
        );
    }

    #[test]
    fn the_check_names_the_field_it_refused() {
        let args = json!({ "url": "https://exemple.fr" });
        assert_eq!(checked_payload(&args).unwrap().field(), "url");
        assert!(check("clipboardWrite", &regex_free_empty()).is_none());
        assert!(check("shareItem", &json!({})).is_some());
        // Les autres méthodes de la famille ne passent pas par ce contrôle.
        assert!(check("clipboardRead", &json!({})).is_none());
        assert!(check("shareText", &json!({})).is_none());
    }

    fn regex_free_empty() -> serde_json::Value {
        json!({ "text": "ok" })
    }

    #[test]
    fn every_family_code_maps_back_to_a_contract_code() {
        // Le pont répond le code de la famille, mais un appelant qui repasse par le contrat
        // doit retrouver un `DeviceError` valide pour chaque refus.
        for (code, contrat) in [
            (
                ClipboardError::MissingArgument,
                DeviceError::MissingArgument,
            ),
            (ClipboardError::Unavailable, DeviceError::Unavailable),
            (ClipboardError::NotSupported, DeviceError::NotSupported),
            (ClipboardError::TooLarge, DeviceError::Internal),
            (ClipboardError::BadBase64, DeviceError::Internal),
            (ClipboardError::Internal, DeviceError::Internal),
        ] {
            assert_eq!(device_code(code), contrat, "{code:?}");
        }
    }
}
