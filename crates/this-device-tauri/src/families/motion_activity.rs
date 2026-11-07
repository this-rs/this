//! The pure part of the activity and steps bridge: the window checked before the pedometer is
//! asked, and the wire types of the answers sent back by `ios/Sources/motion_activity.swift`.
//! No Tauri, no native code: it runs on the host.
//!
//! The answers reuse the contract's [`PermissionState`] and [`DeviceError`] for `status` and
//! `error`; a step count has no place in `device_bridge::Reply`, so it has its own variant here.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::{DeviceError, PermissionState};

/// The longest window the pedometer is asked for: the last 24 hours. Steps are a sample of
/// recent activity, not a history.
pub const WINDOW_MS_MAX: u64 = 24 * 60 * 60 * 1000;

/// A window of time, in Unix epoch milliseconds, already bounded (see [`bound_window`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub from_ms: u64,
    pub to_ms: u64,
}

/// Bounds a requested window to what the pedometer is asked for.
///
/// - the end is clamped to `now_ms`: a count cannot be read for a time that has not come;
/// - the start is clamped to at most [`WINDOW_MS_MAX`] before the end: a longer window keeps
///   its most recent 24 hours;
/// - an empty window (start not before end, or an end at or before the epoch) is refused with
///   [`DeviceError::Internal`], as a bad argument is refused elsewhere in the bridge.
pub fn bound_window(from_ms: u64, to_ms: u64, now_ms: u64) -> Result<Window, DeviceError> {
    let to = to_ms.min(now_ms);
    let from = from_ms.max(to.saturating_sub(WINDOW_MS_MAX));
    if from < to {
        Ok(Window {
            from_ms: from,
            to_ms: to,
        })
    } else {
        Err(DeviceError::Internal)
    }
}

/// The arguments sent to the native `motionStepsSample` method.
pub fn steps_args(window: Window) -> serde_json::Value {
    serde_json::json!({ "from_ms": window.from_ms, "to_ms": window.to_ms })
}

/// A step count over a window, as the pedometer answers for it. `from_ms` and `to_ms` are the
/// period the count covers, which may be narrower than the requested window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepsSample {
    pub steps: u64,
    pub from_ms: u64,
    pub to_ms: u64,
}

/// What the native side answers for this family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MotionReply {
    Status { state: PermissionState },
    Steps(StepsSample),
    Error { code: DeviceError },
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_MS: u64 = 60 * 60 * 1000;
    const NOW: u64 = 1_700_000_000_000;

    #[test]
    fn a_window_inside_the_bounds_is_kept_as_asked() {
        let window = bound_window(NOW - HOUR_MS, NOW - HOUR_MS / 2, NOW).unwrap();
        assert_eq!(
            window,
            Window {
                from_ms: NOW - HOUR_MS,
                to_ms: NOW - HOUR_MS / 2
            }
        );
    }

    #[test]
    fn an_end_in_the_future_is_clamped_to_now() {
        let window = bound_window(NOW - HOUR_MS, NOW + HOUR_MS, NOW).unwrap();
        assert_eq!(window.to_ms, NOW);
        assert_eq!(window.from_ms, NOW - HOUR_MS);
    }

    #[test]
    fn a_window_longer_than_a_day_keeps_its_last_day() {
        let window = bound_window(0, NOW, NOW).unwrap();
        assert_eq!(window.to_ms, NOW);
        assert_eq!(window.from_ms, NOW - WINDOW_MS_MAX);
        assert_eq!(window.to_ms - window.from_ms, WINDOW_MS_MAX);
    }

    #[test]
    fn a_window_of_exactly_a_day_is_kept_whole() {
        let window = bound_window(NOW - WINDOW_MS_MAX, NOW, NOW).unwrap();
        assert_eq!(window.from_ms, NOW - WINDOW_MS_MAX);
    }

    #[test]
    fn an_empty_window_is_refused() {
        assert_eq!(
            bound_window(NOW, NOW, NOW),
            Err(DeviceError::Internal),
            "from equal to to"
        );
        assert_eq!(
            bound_window(NOW, NOW - HOUR_MS, NOW),
            Err(DeviceError::Internal),
            "from after to"
        );
    }

    #[test]
    fn a_window_entirely_in_the_future_is_refused() {
        // The end is clamped to now, which leaves nothing before it: refused, not guessed.
        assert_eq!(
            bound_window(NOW + HOUR_MS, NOW + 2 * HOUR_MS, NOW),
            Err(DeviceError::Internal)
        );
    }

    #[test]
    fn a_zero_end_is_refused_without_underflow() {
        assert_eq!(bound_window(0, 0, 0), Err(DeviceError::Internal));
        assert_eq!(bound_window(0, 0, NOW), Err(DeviceError::Internal));
    }

    #[test]
    fn the_steps_arguments_carry_the_bounded_window() {
        let window = bound_window(NOW - HOUR_MS, NOW, NOW).unwrap();
        assert_eq!(
            steps_args(window),
            serde_json::json!({ "from_ms": NOW - HOUR_MS, "to_ms": NOW })
        );
    }

    // What the Swift side sends, byte for byte (see ios/Sources/motion_activity.swift).
    #[test]
    fn a_steps_answer_decodes_into_the_contract() {
        let reply: MotionReply = serde_json::from_str(
            r#"{"kind":"steps","steps":4213,"from_ms":1700000000000,"to_ms":1700003600000}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            MotionReply::Steps(StepsSample {
                steps: 4213,
                from_ms: 1_700_000_000_000,
                to_ms: 1_700_003_600_000
            })
        );
    }

    #[test]
    fn a_zero_step_count_is_a_count_not_an_error() {
        let reply: MotionReply = serde_json::from_str(
            r#"{"kind":"steps","steps":0,"from_ms":1700000000000,"to_ms":1700003600000}"#,
        )
        .unwrap();
        assert!(matches!(reply, MotionReply::Steps(s) if s.steps == 0));
    }

    #[test]
    fn a_fractional_step_count_is_refused_not_rounded() {
        assert!(
            serde_json::from_str::<MotionReply>(
                r#"{"kind":"steps","steps":4213.5,"from_ms":1,"to_ms":2}"#
            )
            .is_err()
        );
    }

    #[test]
    fn every_permission_state_of_the_activity_status_is_a_contract_state() {
        for (wire, state) in [
            ("granted", PermissionState::Granted),
            ("denied", PermissionState::Denied),
            ("prompt", PermissionState::Prompt),
            ("unsupported", PermissionState::Unsupported),
        ] {
            let json = format!(r#"{{"kind":"status","state":"{wire}"}}"#);
            let reply: MotionReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, MotionReply::Status { state }, "{wire}");
        }
    }

    #[test]
    fn every_error_code_of_this_family_is_a_contract_code() {
        for (wire, code) in [
            ("not_authorized", DeviceError::NotAuthorized),
            ("not_supported", DeviceError::NotSupported),
            ("unavailable", DeviceError::Unavailable),
            ("timeout", DeviceError::Timeout),
            ("internal", DeviceError::Internal),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: MotionReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, MotionReply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn an_unknown_kind_is_refused_not_guessed() {
        assert!(serde_json::from_str::<MotionReply>(r#"{"kind":"accelerometer","x":0}"#).is_err());
    }
}
