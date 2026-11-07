//! The Face ID and speech-recognition family: the pure part, checked on the host.
//!
//! Face ID is an authentication of one action, not an identity read: the native side answers
//! a verdict and nothing else. [`FaceIdVerdict`] has two values, and [`FaceSpeechReply`] refuses
//! unknown fields, so a native answer that tried to carry biometric data would not decode.
//!
//! Speech is the transcription of one audio file the app already holds, bounded in duration and
//! run on device only. The microphone is another family and is not here.
//!
//! No Tauri, no native code: the wire answers and the argument checks, tested without a phone.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::PermissionState;

/// Longest motif the system shows for a Face ID confirmation, in Unicode scalars.
pub const FACE_ID_REASON_MAX: usize = 120;
/// Shortest and longest audio file accepted, in seconds.
pub const SPEECH_SECONDS_MIN: u32 = 1;
pub const SPEECH_SECONDS_MAX: u32 = 60;

/// The verdict of a Face ID confirmation. Nothing else comes back: no template, no match score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceIdVerdict {
    Granted,
    /// Refused, cancelled, failed, locked out, or no usable biometry: all the same answer.
    NotAuthorized,
}

/// The biometry the device offers, read without prompting the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Biometry {
    FaceId,
    TouchId,
    /// No biometry usable right now.
    #[serde(rename = "none")]
    NoBiometry,
}

/// Why a speech transcription was not run, or did not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechRefusal {
    /// The recognizer cannot run on the device for this language. There is no server fallback.
    Unsupported,
    NotAuthorized,
    /// The file is longer than the bound the caller asked for.
    TooLong,
    /// The file is outside the app's container, or cannot be read as audio.
    Unreadable,
    /// The recognizer exists but cannot answer now.
    Unavailable,
    Timeout,
}

/// Every answer the native side sends for this family, tagged by `kind`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FaceSpeechReply {
    FaceIdBiometry { biometry: Biometry },
    FaceId { verdict: FaceIdVerdict },
    SpeechStatus { state: PermissionState },
    SpeechTranscript { text: String },
    SpeechRefused { code: SpeechRefusal },
}

/// An argument refused before the native side is touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Reason,
    Locale,
    Seconds,
}

/// The motif for a Face ID confirmation: trimmed, non-empty, at most [`FACE_ID_REASON_MAX`]
/// characters, with no control character. Refused rather than cut, so the person reads what the
/// app wrote. The content itself must carry no personal data: this checks shape, not meaning.
pub fn check_reason(reason: &str) -> Result<&str, Invalid> {
    let trimmed = reason.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > FACE_ID_REASON_MAX
        || trimmed.chars().any(char::is_control)
    {
        return Err(Invalid::Reason);
    }
    Ok(trimmed)
}

/// A language tag in the simple BCP 47 form the recognizer takes: a 2- or 3-letter lowercase
/// language, optionally followed by a 2-letter uppercase region (`fr`, `fr-FR`, `en-US`).
/// Scripts, variants and extensions are refused: the accepted set is closed on purpose.
pub fn check_locale(tag: &str) -> Result<&str, Invalid> {
    let (language, region) = match tag.split_once('-') {
        Some((language, region)) => (language, Some(region)),
        None => (tag, None),
    };
    let language_ok =
        (2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_lowercase());
    let region_ok =
        region.is_none_or(|r| r.len() == 2 && r.bytes().all(|b| b.is_ascii_uppercase()));
    if language_ok && region_ok {
        Ok(tag)
    } else {
        Err(Invalid::Locale)
    }
}

/// The longest transcription the caller asks for, in seconds. Refused when outside the bounds,
/// not clamped: a caller asking for an empty or unbounded transcription has a bug to see.
pub fn check_seconds(seconds: u32) -> Result<u32, Invalid> {
    if (SPEECH_SECONDS_MIN..=SPEECH_SECONDS_MAX).contains(&seconds) {
        Ok(seconds)
    } else {
        Err(Invalid::Seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // What the Swift side sends, byte for byte (ios/Sources/face_id.swift, speech.swift).

    #[test]
    fn face_id_biometry_decodes_into_the_contract() {
        for (wire, biometry) in [
            ("face_id", Biometry::FaceId),
            ("touch_id", Biometry::TouchId),
            ("none", Biometry::NoBiometry),
        ] {
            let json = format!(r#"{{"kind":"face_id_biometry","biometry":"{wire}"}}"#);
            let reply: FaceSpeechReply = serde_json::from_str(&json).unwrap();
            assert_eq!(
                reply,
                FaceSpeechReply::FaceIdBiometry { biometry },
                "{wire}"
            );
        }
    }

    #[test]
    fn a_face_id_verdict_is_granted_or_not_authorized_and_nothing_else() {
        let granted: FaceSpeechReply =
            serde_json::from_str(r#"{"kind":"face_id","verdict":"granted"}"#).unwrap();
        assert_eq!(
            granted,
            FaceSpeechReply::FaceId {
                verdict: FaceIdVerdict::Granted
            }
        );
        let refused: FaceSpeechReply =
            serde_json::from_str(r#"{"kind":"face_id","verdict":"not_authorized"}"#).unwrap();
        assert_eq!(
            refused,
            FaceSpeechReply::FaceId {
                verdict: FaceIdVerdict::NotAuthorized
            }
        );
        assert!(
            serde_json::from_str::<FaceSpeechReply>(r#"{"kind":"face_id","verdict":"matched"}"#)
                .is_err()
        );
    }

    #[test]
    fn a_face_id_answer_carrying_biometric_data_is_refused() {
        // The contract carries a verdict only. Any extra field, a template or a score, is an
        // error, never silently dropped.
        for json in [
            r#"{"kind":"face_id","verdict":"granted","template":"AAEC"}"#,
            r#"{"kind":"face_id","verdict":"granted","score":0.97}"#,
            r#"{"kind":"face_id_biometry","biometry":"face_id","image":"AAEC"}"#,
        ] {
            assert!(
                serde_json::from_str::<FaceSpeechReply>(json).is_err(),
                "{json}"
            );
        }
    }

    #[test]
    fn an_unknown_biometry_is_refused_not_guessed() {
        assert!(
            serde_json::from_str::<FaceSpeechReply>(
                r#"{"kind":"face_id_biometry","biometry":"optic_id"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn speech_status_uses_the_permission_states() {
        for (wire, state) in [
            ("granted", PermissionState::Granted),
            ("denied", PermissionState::Denied),
            ("prompt", PermissionState::Prompt),
        ] {
            let json = format!(r#"{{"kind":"speech_status","state":"{wire}"}}"#);
            let reply: FaceSpeechReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, FaceSpeechReply::SpeechStatus { state }, "{wire}");
        }
    }

    #[test]
    fn a_transcript_decodes_as_its_text() {
        let reply: FaceSpeechReply =
            serde_json::from_str(r#"{"kind":"speech_transcript","text":"bonjour le monde"}"#)
                .unwrap();
        assert_eq!(
            reply,
            FaceSpeechReply::SpeechTranscript {
                text: "bonjour le monde".into()
            }
        );
    }

    #[test]
    fn every_refusal_the_native_side_sends_is_a_known_code() {
        for (wire, code) in [
            ("unsupported", SpeechRefusal::Unsupported),
            ("not_authorized", SpeechRefusal::NotAuthorized),
            ("too_long", SpeechRefusal::TooLong),
            ("unreadable", SpeechRefusal::Unreadable),
            ("unavailable", SpeechRefusal::Unavailable),
            ("timeout", SpeechRefusal::Timeout),
        ] {
            let json = format!(r#"{{"kind":"speech_refused","code":"{wire}"}}"#);
            let reply: FaceSpeechReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, FaceSpeechReply::SpeechRefused { code }, "{wire}");
        }
    }

    #[test]
    fn an_unknown_kind_is_refused_not_guessed() {
        assert!(
            serde_json::from_str::<FaceSpeechReply>(r#"{"kind":"camera","frames":[]}"#).is_err()
        );
    }

    #[test]
    fn a_reason_is_trimmed_and_kept() {
        assert_eq!(
            check_reason("  Confirmer le virement  "),
            Ok("Confirmer le virement")
        );
    }

    #[test]
    fn an_empty_or_blank_reason_is_refused() {
        assert_eq!(check_reason(""), Err(Invalid::Reason));
        assert_eq!(check_reason("   \n\t "), Err(Invalid::Reason));
    }

    #[test]
    fn a_reason_is_bounded_in_characters_not_bytes() {
        let at_the_limit = "é".repeat(FACE_ID_REASON_MAX);
        assert_eq!(check_reason(&at_the_limit), Ok(at_the_limit.as_str()));
        let over = "é".repeat(FACE_ID_REASON_MAX + 1);
        assert_eq!(check_reason(&over), Err(Invalid::Reason));
    }

    #[test]
    fn a_reason_with_a_control_character_is_refused() {
        assert_eq!(check_reason("ligne 1\nligne 2"), Err(Invalid::Reason));
        assert_eq!(check_reason("nul\u{0}"), Err(Invalid::Reason));
    }

    #[test]
    fn the_simple_bcp47_locales_are_accepted() {
        for tag in ["fr", "fr-FR", "en-US", "de", "pt-BR"] {
            assert_eq!(check_locale(tag), Ok(tag), "{tag}");
        }
    }

    #[test]
    fn a_locale_outside_the_simple_form_is_refused() {
        for bad in [
            "",
            "FR",
            "fr_FR",
            "fr-fr",
            "english",
            "f",
            "fr-",
            "fr-FRA",
            "zh-Hant-TW",
            "fr-FR-x",
        ] {
            assert_eq!(check_locale(bad), Err(Invalid::Locale), "{bad:?}");
        }
    }

    #[test]
    fn the_transcription_duration_is_refused_outside_its_bounds() {
        assert_eq!(check_seconds(0), Err(Invalid::Seconds));
        assert_eq!(check_seconds(61), Err(Invalid::Seconds));
        assert_eq!(check_seconds(SPEECH_SECONDS_MIN), Ok(SPEECH_SECONDS_MIN));
        assert_eq!(check_seconds(SPEECH_SECONDS_MAX), Ok(SPEECH_SECONDS_MAX));
    }
}
