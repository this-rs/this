//! The camera and microphone family: the permission state of the two capabilities, and the
//! request that asks the person for them. Permission only: no frame and no sample is read here.
//!
//! The answers are the shared contract ([`this_platform::device_bridge::Reply`]), so this file
//! adds only what the family needs on top of it: the capability argument the native side reads,
//! and the names of its two methods.
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
}
