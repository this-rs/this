//! The camera and microphone family: the permission state of the two capabilities, the request
//! that asks the person for them, and — since the audit of 08/11/2026 — the capture itself.
//!
//! Before that audit, "camera" and "microphone" meant the permission and nothing else: an app
//! that could ask for the camera and never take a picture. The bridge now carries one photo and
//! one bounded audio recording, with the bounds checked HERE: a capture is expensive, and a
//! bound that only lives in a unit test bounds nothing.
//!
//! Not reachable from a webview: a request shows the system prompt, so a caller must pass the
//! consent gate ([`this_platform::device_bridge::gate`]) first.

use serde::{Deserialize, Serialize};
use this_platform::capability::Capability;

/// The native method that reads the state, without prompting the person.
pub const STATUS_METHOD: &str = "captureStatus";
/// The native method that asks the person for the permission if it is not decided yet.
pub const REQUEST_METHOD: &str = "captureRequest";

/// A capability of this family. Serialized as the catalogue names it (`"camera"`,
/// `"microphone"`), which is the word the native side reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCapability {
    Camera,
    Microphone,
}

impl CaptureCapability {
    /// The catalogue capability this family covers.
    pub fn capability(self) -> Capability {
        match self {
            CaptureCapability::Camera => Capability::Camera,
            CaptureCapability::Microphone => Capability::Microphone,
        }
    }
}

/// The payload sent to [`STATUS_METHOD`] and [`REQUEST_METHOD`]: `{"capability": ...}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureArgs {
    pub capability: CaptureCapability,
}

/// The smallest photo worth taking: below, the answer is a thumbnail, not a picture.
pub const PHOTO_PX_MIN: u32 = 64;
/// The largest side of a photo the bridge carries. Above, the base64 alone crosses several
/// megabytes of JSON for a detail nobody reads.
pub const PHOTO_PX_MAX: u32 = 4096;

/// The shortest recording. Below, the file is a click.
pub const RECORD_MS_MIN: u32 = 500;
/// The longest recording one call carries: a minute.
pub const RECORD_MS_MAX: u32 = 60_000;

/// The checked bounds of a photo: the largest side, and the JPEG quality.
///
/// Clamped rather than refused: the contract has no code for a silly quality, and the nearest
/// bound is what the caller meant. 0 would produce a black square, which is not a picture.
pub fn checked_photo(args: &serde_json::Value) -> (u32, f64) {
    let max_px = args
        .get("max_px")
        .and_then(serde_json::Value::as_u64)
        .map(|px| (px as u32).clamp(PHOTO_PX_MIN, PHOTO_PX_MAX))
        .unwrap_or(1280);
    let quality = args
        .get("quality")
        .and_then(serde_json::Value::as_f64)
        .map(|q| q.clamp(0.1, 1.0))
        .unwrap_or(0.8);
    (max_px, quality)
}

/// The checked bounds of a recording, in milliseconds.
pub fn checked_record(args: &serde_json::Value) -> u32 {
    args.get("max_ms")
        .and_then(serde_json::Value::as_u64)
        .map(|ms| (ms as u32).clamp(RECORD_MS_MIN, RECORD_MS_MAX))
        .unwrap_or(RECORD_MS_MAX)
}

/// One photo, as the native side sends it back. The bytes cross as base64 because they cross
/// JSON; the size is checked by the caller, not here.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Photo {
    pub data_base64: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

/// One recording, already written to a file in the app's container.
///
/// The path, not the bytes: a minute of audio is a file, and the graph keeps facts, not
/// payloads. Whoever needs the content reads it through the document path.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    pub path: String,
    pub bytes: u64,
    pub duration_ms: u64,
}

/// Every answer of the family after the permission pair, tagged by `kind`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureReply {
    Photo(Photo),
    Audio(Audio),
    /// A recording is running: it stops on `microphoneStop`, or alone at `max_ms`.
    Recording {
        max_ms: u32,
    },
    Error {
        code: CaptureError,
    },
}

/// Why a capture was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureError {
    /// The person has not granted the camera or the microphone.
    NotAuthorized,
    /// No camera, no microphone, or the system refuses the session.
    Unsupported,
    /// Already recording: one recording at a time, or the files would overlap.
    Unavailable,
    /// `microphoneStop` with nothing running: there is no file to hand back.
    NothingRecording,
    Timeout,
    Internal,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use this_platform::device_bridge::{DeviceError, PermissionState, Reply};

    use super::*;

    #[test]
    fn the_payload_is_the_catalogue_name_of_the_capability() {
        let camera = serde_json::to_value(CaptureArgs {
            capability: CaptureCapability::Camera,
        })
        .unwrap();
        assert_eq!(camera, json!({"capability": "camera"}));

        let microphone = serde_json::to_value(CaptureArgs {
            capability: CaptureCapability::Microphone,
        })
        .unwrap();
        assert_eq!(microphone, json!({"capability": "microphone"}));
    }

    #[test]
    fn the_family_names_the_same_words_as_the_catalogue() {
        // A rename in the catalogue must not drift away from the native side.
        for (family, catalogue) in [
            (CaptureCapability::Camera, Capability::Camera),
            (CaptureCapability::Microphone, Capability::Microphone),
        ] {
            assert_eq!(family.capability(), catalogue);
            assert_eq!(
                serde_json::to_value(family).unwrap(),
                serde_json::to_value(catalogue).unwrap()
            );
        }
    }

    #[test]
    fn the_native_side_payload_decodes_into_the_family() {
        let args: CaptureArgs = serde_json::from_str(r#"{"capability":"camera"}"#).unwrap();
        assert_eq!(args.capability, CaptureCapability::Camera);
        let args: CaptureArgs = serde_json::from_str(r#"{"capability":"microphone"}"#).unwrap();
        assert_eq!(args.capability, CaptureCapability::Microphone);
    }

    #[test]
    fn a_capability_outside_the_family_is_refused() {
        // Gyroscope and location are not this family: refused, not guessed.
        for bad in [
            r#"{"capability":"gyroscope"}"#,
            r#"{"capability":"location"}"#,
            r#"{"capability":"Camera"}"#,
            r#"{}"#,
        ] {
            assert!(serde_json::from_str::<CaptureArgs>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_state_answers_decode_into_the_contract() {
        for (wire, state) in [
            ("granted", PermissionState::Granted),
            ("denied", PermissionState::Denied),
            ("prompt", PermissionState::Prompt),
            ("unsupported", PermissionState::Unsupported),
        ] {
            let json = format!(r#"{{"kind":"status","state":"{wire}"}}"#);
            let reply: Reply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, Reply::Status { state }, "{wire}");
        }
    }

    #[test]
    fn an_unknown_capability_is_answered_as_not_supported() {
        let reply: Reply =
            serde_json::from_str(r#"{"kind":"error","code":"not_supported"}"#).unwrap();
        assert_eq!(
            reply,
            Reply::Error {
                code: DeviceError::NotSupported
            }
        );
    }

    #[test]
    fn the_request_answer_is_a_status_not_a_grant_of_its_own() {
        // The answer to a request is the state after the prompt, as a status: the family
        // sends no other shape.
        let value: Value = serde_json::to_value(Reply::Status {
            state: PermissionState::Denied,
        })
        .unwrap();
        assert_eq!(value, json!({"kind": "status", "state": "denied"}));
    }

    #[test]
    fn the_method_names_are_the_ones_the_native_side_exposes() {
        assert_eq!(STATUS_METHOD, "captureStatus");
        assert_eq!(REQUEST_METHOD, "captureRequest");
    }

    // ---- La capture elle-même ---------------------------------------------------------

    #[test]
    fn a_photo_keeps_its_bounds_in_the_native_range() {
        // Les bornes existent pour que la base64 ne traverse pas des mégaoctets de JSON pour
        // un détail que personne ne lit.
        assert_eq!(checked_photo(&json!({"max_px": 1})).0, PHOTO_PX_MIN);
        assert_eq!(checked_photo(&json!({"max_px": 1280})).0, 1280);
        assert_eq!(checked_photo(&json!({"max_px": 99_999})).0, PHOTO_PX_MAX);
        // Sans argument : le défaut documenté, pas zéro.
        assert_eq!(checked_photo(&json!({})), (1280, 0.8));
    }

    #[test]
    fn a_quality_of_zero_is_not_a_picture() {
        assert_eq!(checked_photo(&json!({"quality": 0.0})).1, 0.1);
        assert_eq!(checked_photo(&json!({"quality": 1.0})).1, 1.0);
        assert_eq!(checked_photo(&json!({"quality": 7.0})).1, 1.0);
    }

    #[test]
    fn a_recording_is_bounded_to_a_minute() {
        assert_eq!(checked_record(&json!({"max_ms": 1})), RECORD_MS_MIN);
        assert_eq!(checked_record(&json!({"max_ms": 5_000})), 5_000);
        assert_eq!(checked_record(&json!({"max_ms": 600_000})), RECORD_MS_MAX);
        // Un enregistrement de zéro milliseconde n'est pas un enregistrement.
        assert_eq!(checked_record(&json!({})), RECORD_MS_MAX);
    }

    // Ce que le natif envoie, octet pour octet. Une forme qui dérive est un `internal` muet.
    #[test]
    fn every_capture_answer_decodes() {
        let photo: CaptureReply = serde_json::from_str(
            r#"{"kind":"photo","data_base64":"/9j/4AAQ","width":1280,"height":960,"bytes":98000}"#,
        )
        .unwrap();
        assert_eq!(
            photo,
            CaptureReply::Photo(Photo {
                data_base64: "/9j/4AAQ".into(),
                width: 1280,
                height: 960,
                bytes: 98_000,
            })
        );

        let audio: CaptureReply = serde_json::from_str(
            r#"{"kind":"audio","path":"/var/mobile/Containers/Data/Application/X/tmp/note.m4a","bytes":42000,"duration_ms":12000}"#,
        )
        .unwrap();
        assert_eq!(
            audio,
            CaptureReply::Audio(Audio {
                path: "/var/mobile/Containers/Data/Application/X/tmp/note.m4a".into(),
                bytes: 42_000,
                duration_ms: 12_000,
            })
        );

        let running: CaptureReply =
            serde_json::from_str(r#"{"kind":"recording","max_ms":60000}"#).unwrap();
        assert_eq!(running, CaptureReply::Recording { max_ms: 60_000 });
    }

    #[test]
    fn a_photo_without_its_size_is_refused() {
        // Sans largeur ni hauteur, l'appelant ne peut pas savoir ce qu'il a reçu : la forme
        // est refusée plutôt que devinée.
        let brut = r#"{"kind":"photo","data_base64":"/9j/4AAQ","bytes":98000}"#;
        assert!(serde_json::from_str::<CaptureReply>(brut).is_err());
    }

    #[test]
    fn every_capture_error_code_is_the_contract_word() {
        for (code, texte) in [
            (CaptureError::NotAuthorized, "not_authorized"),
            (CaptureError::Unsupported, "unsupported"),
            (CaptureError::Unavailable, "unavailable"),
            (CaptureError::NothingRecording, "nothing_recording"),
            (CaptureError::Timeout, "timeout"),
            (CaptureError::Internal, "internal"),
        ] {
            let valeur = serde_json::to_value(CaptureReply::Error { code }).unwrap();
            assert_eq!(valeur, json!({"kind":"error","code":texte}));
        }
    }
}
