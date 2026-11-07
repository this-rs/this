//! The health family: HealthKit, read only. The pure part, as `wire.rs`: the closed list of
//! quantity types, the bounds, the arguments and the answers. No Tauri, no native code.
//!
//! The native side (`ios/Sources/health.swift`) holds the same list and refuses any name it
//! does not know. This module refuses first, so a name outside the list is never sent.
//!
//! Read only: nothing here writes to HealthKit, and no request asks for write (share) access.
//! HealthKit does not reveal whether a read was granted: a status is `prompt`, never `granted`
//! by default, and an empty samples list is not a refusal (it can mean no data or no access).

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::{DeviceError, PermissionState};

/// The quantity types the bridge may read. Closed: any other name is refused as not supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantityType {
    /// Steps taken (`HKQuantityTypeIdentifierStepCount`), a count.
    StepCount,
    /// Heart rate (`HKQuantityTypeIdentifierHeartRate`), beats per minute.
    HeartRate,
    /// Body mass (`HKQuantityTypeIdentifierBodyMass`), kilograms.
    BodyMass,
}

impl QuantityType {
    /// The whole closed list, in the order the documentation gives it.
    pub const ALL: [QuantityType; 3] = [
        QuantityType::StepCount,
        QuantityType::HeartRate,
        QuantityType::BodyMass,
    ];

    /// The contract name of the type: the serde name, and the name the native side receives.
    pub fn name(self) -> &'static str {
        match self {
            QuantityType::StepCount => "step_count",
            QuantityType::HeartRate => "heart_rate",
            QuantityType::BodyMass => "body_mass",
        }
    }

    /// The unit every sample is expressed in. The native side converts to it before answering.
    pub fn unit(self) -> &'static str {
        match self {
            QuantityType::StepCount => "count",
            QuantityType::HeartRate => "count/min",
            QuantityType::BodyMass => "kg",
        }
    }

    /// The type named `name`, or `None` if the name is not in the closed list.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|quantity| quantity.name() == name)
    }
}

/// The look-back window of a sample request, in milliseconds: one minute at least, 24 hours at
/// most. A window outside the range is clamped into it.
pub const WINDOW_MS_MIN: u64 = 60_000;
pub const WINDOW_MS_MAX: u64 = 24 * 60 * 60 * 1000;
/// How many samples one answer may carry: one at least, ten at most (clamped into the range).
pub const LIMIT_MIN: u32 = 1;
pub const LIMIT_MAX: u32 = 10;

/// The window kept in the bounded range. Clamped rather than refused, as the scan duration.
pub fn clamp_window_ms(window_ms: u64) -> u64 {
    window_ms.clamp(WINDOW_MS_MIN, WINDOW_MS_MAX)
}

/// The sample count kept in the bounded range.
pub fn clamp_limit(limit: u32) -> u32 {
    limit.clamp(LIMIT_MIN, LIMIT_MAX)
}

/// The arguments of a status request: one quantity type, by name.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StatusArgs {
    pub quantity: String,
}

/// The arguments of a permission request: the quantity types to ask read access for.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RequestArgs {
    pub quantities: Vec<String>,
}

/// The arguments of a sample request: one quantity type, a look-back window, a sample count.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SampleArgs {
    pub quantity: String,
    pub window_ms: u64,
    pub limit: u32,
}

/// What the health family answers. `status` and `error` mirror the device contract; `samples`
/// is the health answer, which the device contract has no kind for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HealthReply {
    Status {
        state: PermissionState,
    },
    Error {
        code: DeviceError,
    },
    Samples {
        quantity: QuantityType,
        unit: String,
        samples: Vec<HealthSample>,
    },
}

/// One sample: its value in the type's unit, and its start and end in Unix epoch milliseconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthSample {
    pub value: f64,
    pub start_ms: u64,
    pub end_ms: u64,
}

fn refused(code: DeviceError) -> HealthReply {
    HealthReply::Error { code }
}

/// The checked quantity type of a name, or the refusal to send back: `not_supported` for a name
/// outside the closed list.
pub fn checked_quantity(name: &str) -> Result<QuantityType, HealthReply> {
    QuantityType::from_name(name).ok_or_else(|| refused(DeviceError::NotSupported))
}

/// The checked types of a permission request. The whole request is refused if one name is not
/// in the list, and an empty request is refused as an internal error (nothing to ask for).
pub fn checked_quantities(names: &[String]) -> Result<Vec<QuantityType>, HealthReply> {
    if names.is_empty() {
        return Err(refused(DeviceError::Internal));
    }
    names.iter().map(|name| checked_quantity(name)).collect()
}

/// The checked arguments of a sample request: the type, the window and the count, bounded.
pub fn sample_args(args: &SampleArgs) -> Result<(QuantityType, u64, u32), HealthReply> {
    let quantity = checked_quantity(&args.quantity)?;
    Ok((
        quantity,
        clamp_window_ms(args.window_ms),
        clamp_limit(args.limit),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_supported() -> HealthReply {
        HealthReply::Error {
            code: DeviceError::NotSupported,
        }
    }

    #[test]
    fn the_closed_list_is_exactly_the_three_documented_types() {
        let names: Vec<&str> = QuantityType::ALL.iter().map(|q| q.name()).collect();
        assert_eq!(names, ["step_count", "heart_rate", "body_mass"]);
        assert_eq!(QuantityType::ALL.len(), 3);
    }

    #[test]
    fn each_name_is_the_serde_name_of_its_type() {
        for quantity in QuantityType::ALL {
            let value = serde_json::to_value(quantity).unwrap();
            assert_eq!(value, serde_json::Value::String(quantity.name().into()));
            assert_eq!(QuantityType::from_name(quantity.name()), Some(quantity));
        }
    }

    #[test]
    fn a_name_outside_the_closed_list_is_refused_as_not_supported() {
        for bad in [
            "",
            "sleep_analysis",
            "active_energy_burned",
            "STEP_COUNT",
            "stepCount",
            "bodyMass",
            "heart_rate ",
            " body_mass",
        ] {
            assert_eq!(QuantityType::from_name(bad), None, "{bad:?}");
            assert_eq!(checked_quantity(bad), Err(not_supported()), "{bad:?}");
        }
    }

    #[test]
    fn a_permission_request_with_one_unknown_name_is_refused_whole() {
        let names = ["heart_rate".to_string(), "sleep_analysis".to_string()];
        assert_eq!(checked_quantities(&names), Err(not_supported()));
    }

    #[test]
    fn an_empty_permission_request_is_refused_as_internal() {
        assert_eq!(
            checked_quantities(&[]),
            Err(HealthReply::Error {
                code: DeviceError::Internal
            })
        );
    }

    #[test]
    fn each_type_has_its_unit() {
        assert_eq!(QuantityType::StepCount.unit(), "count");
        assert_eq!(QuantityType::HeartRate.unit(), "count/min");
        assert_eq!(QuantityType::BodyMass.unit(), "kg");
    }

    #[test]
    fn sample_arguments_decode_from_the_contract_json() {
        let args: SampleArgs =
            serde_json::from_str(r#"{"quantity":"heart_rate","window_ms":3600000,"limit":5}"#)
                .unwrap();
        assert_eq!(
            args,
            SampleArgs {
                quantity: "heart_rate".into(),
                window_ms: 3_600_000,
                limit: 5,
            }
        );
    }

    #[test]
    fn sample_arguments_are_checked_and_bounded() {
        let args = SampleArgs {
            quantity: "body_mass".into(),
            window_ms: u64::MAX,
            limit: 1_000,
        };
        assert_eq!(
            sample_args(&args),
            Ok((QuantityType::BodyMass, WINDOW_MS_MAX, LIMIT_MAX))
        );
        let args = SampleArgs {
            quantity: "step_count".into(),
            window_ms: 0,
            limit: 0,
        };
        assert_eq!(
            sample_args(&args),
            Ok((QuantityType::StepCount, WINDOW_MS_MIN, LIMIT_MIN))
        );
    }

    #[test]
    fn sample_arguments_with_an_unknown_type_are_refused() {
        let args = SampleArgs {
            quantity: "sleep_analysis".into(),
            window_ms: 3_600_000,
            limit: 1,
        };
        assert_eq!(sample_args(&args), Err(not_supported()));
    }

    #[test]
    fn the_window_is_kept_at_most_24_hours() {
        assert_eq!(WINDOW_MS_MAX, 86_400_000);
        assert_eq!(clamp_window_ms(86_400_001), 86_400_000);
        assert_eq!(clamp_window_ms(3_000), WINDOW_MS_MIN);
    }

    #[test]
    fn the_sample_count_is_kept_between_one_and_ten() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(7), 7);
        assert_eq!(clamp_limit(11), 10);
    }

    #[test]
    fn the_native_samples_answer_decodes_into_the_contract() {
        let reply: HealthReply = serde_json::from_str(
            r#"{"kind":"samples","quantity":"heart_rate","unit":"count/min","samples":[{"value":72.0,"start_ms":1700000000000,"end_ms":1700000060000}]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            HealthReply::Samples {
                quantity: QuantityType::HeartRate,
                unit: "count/min".into(),
                samples: vec![HealthSample {
                    value: 72.0,
                    start_ms: 1_700_000_000_000,
                    end_ms: 1_700_000_060_000,
                }],
            }
        );
    }

    #[test]
    fn an_empty_samples_answer_is_a_list_not_an_error() {
        let reply: HealthReply = serde_json::from_str(
            r#"{"kind":"samples","quantity":"body_mass","unit":"kg","samples":[]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            HealthReply::Samples {
                quantity: QuantityType::BodyMass,
                unit: "kg".into(),
                samples: vec![],
            }
        );
    }

    #[test]
    fn a_status_answer_decodes_and_prompt_is_the_read_state() {
        let reply: HealthReply =
            serde_json::from_str(r#"{"kind":"status","state":"prompt"}"#).unwrap();
        assert_eq!(
            reply,
            HealthReply::Status {
                state: PermissionState::Prompt
            }
        );
    }

    #[test]
    fn a_samples_answer_encodes_to_the_wire_shape() {
        let reply = HealthReply::Samples {
            quantity: QuantityType::StepCount,
            unit: "count".into(),
            samples: vec![HealthSample {
                value: 4210.0,
                start_ms: 1,
                end_ms: 2,
            }],
        };
        assert_eq!(
            serde_json::to_value(reply).unwrap(),
            serde_json::json!({
                "kind": "samples",
                "quantity": "step_count",
                "unit": "count",
                "samples": [{"value": 4210.0, "start_ms": 1, "end_ms": 2}]
            })
        );
    }

    #[test]
    fn a_fractional_timestamp_is_refused_by_the_contract() {
        assert!(
            serde_json::from_str::<HealthReply>(
                r#"{"kind":"samples","quantity":"step_count","unit":"count","samples":[{"value":1,"start_ms":1700000000000.0,"end_ms":1}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn an_unknown_answer_kind_is_refused_not_guessed() {
        assert!(serde_json::from_str::<HealthReply>(r#"{"kind":"read_all","data":[]}"#).is_err());
    }
}
