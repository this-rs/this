//! The location family: the wire shape of one position fix, and the empty request that asks
//! for it. No Tauri here: it runs on the host, so the contract is checked without a phone.
//!
//! The native side answers `location_sample` with a [`PositionFix`] (see
//! `ios/Sources/location.swift`). A coordinate is personal data: [`PositionFix`] never prints
//! its values in `Debug`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A request for one position fix. The native call takes no argument: on the wire it is `{}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionRequest {}

/// One position fix: latitude and longitude in degrees, the horizontal accuracy in meters, and
/// the time of the fix in Unix milliseconds, as an integer.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PositionFix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: f64,
    pub ts_ms: u64,
}

impl fmt::Debug for PositionFix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PositionFix { <redacted> }")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use this_platform::capability::Capability;

    use super::*;

    // What the Swift side sends, byte for byte (see `LocationFix` in location.swift). The `kind`
    // tag is the one every answer of the bridge carries; the fields here are the fix itself.
    #[test]
    fn a_position_fix_decodes_into_the_contract() {
        let fix: PositionFix = serde_json::from_str(
            r#"{"kind":"location","latitude":48.8566,"longitude":2.3522,"accuracy_m":12.5,"ts_ms":1700000000123}"#,
        )
        .unwrap();
        assert_eq!(fix.latitude, 48.8566);
        assert_eq!(fix.longitude, 2.3522);
        assert_eq!(fix.accuracy_m, 12.5);
        assert_eq!(fix.ts_ms, 1_700_000_000_123);
    }

    #[test]
    fn whole_coordinates_are_accepted_as_numbers() {
        let fix: PositionFix =
            serde_json::from_str(r#"{"latitude":0,"longitude":-90,"accuracy_m":5,"ts_ms":1}"#)
                .unwrap();
        assert_eq!(fix.latitude, 0.0);
        assert_eq!(fix.longitude, -90.0);
    }

    #[test]
    fn a_fix_missing_its_accuracy_is_refused() {
        assert!(
            serde_json::from_str::<PositionFix>(r#"{"latitude":1.0,"longitude":2.0,"ts_ms":1}"#)
                .is_err()
        );
    }

    #[test]
    fn a_fractional_timestamp_is_refused_by_the_contract() {
        // serde rejects a float for a u64: the native side must send an integer.
        assert!(
            serde_json::from_str::<PositionFix>(
                r#"{"latitude":1.0,"longitude":2.0,"accuracy_m":5.0,"ts_ms":1700000000123.0}"#
            )
            .is_err()
        );
    }

    #[test]
    fn the_position_request_is_the_empty_object() {
        assert_eq!(
            serde_json::to_value(PositionRequest::default()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn a_fix_round_trips_through_json() {
        let fix = PositionFix {
            latitude: 45.764,
            longitude: 4.8357,
            accuracy_m: 3.0,
            ts_ms: 1_700_000_000_000,
        };
        let value: Value = serde_json::to_value(fix).unwrap();
        let back: PositionFix = serde_json::from_value(value).unwrap();
        assert!(back == fix);
    }

    #[test]
    fn debug_never_prints_a_coordinate() {
        let fix = PositionFix {
            latitude: 48.8566,
            longitude: 2.3522,
            accuracy_m: 12.5,
            ts_ms: 1,
        };
        let shown = format!("{fix:?}");
        assert!(!shown.contains("48.8566"), "{shown}");
        assert!(!shown.contains("2.3522"), "{shown}");
    }

    #[test]
    fn the_location_capability_arguments_are_the_contract_names() {
        // The payload sent to the native `status` and `location_request` methods.
        assert_eq!(
            json!({ "capability": Capability::Location }),
            json!({ "capability": "location" })
        );
        assert_eq!(
            json!({ "capability": Capability::LocationAlways }),
            json!({ "capability": "location_always" })
        );
    }
}

/// Les précisions qu'un flux accepte, du plus grossier au plus fin. Une chaîne inconnue est
/// refusée plutôt que traduite en silence : « précision » veut dire quelque chose de précis.
pub const ACCURACIES: [&str; 5] = [
    "three_kilometers",
    "hundred_meters",
    "ten_meters",
    "best",
    "best_for_navigation",
];

/// Le plus d'échantillons qu'un flux garde en tampon. Au-delà, les plus anciens tombent : un
/// flux qui grandit sans fin est une fuite qui s'appelle un historique.
pub const RING_MAX: usize = 200;

/// Les bornes d'un démarrage de flux : une précision connue, et un filtre de distance sensé.
pub fn check_stream(accuracy: &str, distance_filter_m: u32) -> Result<(&'static str, u32), String> {
    let Some(accuracy) = ACCURACIES.iter().copied().find(|a| *a == accuracy) else {
        return Err("précision inconnue".to_string());
    };
    Ok((accuracy, distance_filter_m.min(1_000)))
}

/// Une position du tampon, telle que le natif l'écrit.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamFix {
    pub lat: f64,
    pub lon: f64,
    pub accuracy_m: f64,
    pub ts_ms: i64,
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn an_unknown_accuracy_is_refused() {
        assert!(check_stream("teleport", 5).is_err());
        assert_eq!(check_stream("ten_meters", 5), Ok(("ten_meters", 5)));
    }

    #[test]
    fn the_distance_filter_is_bounded_not_refused() {
        // Un filtre absurde ne mérite pas un refus : la borne haute dit ce qu'on peut faire.
        assert_eq!(check_stream("best", 50_000), Ok(("best", 1_000)));
    }

    #[test]
    fn a_fix_decodes_and_refuses_an_unknown_field() {
        let fix: StreamFix = serde_json::from_str(
            r#"{"lat":48.85,"lon":2.35,"accuracy_m":12.5,"ts_ms":1700000000000}"#,
        )
        .unwrap();
        assert_eq!(fix.accuracy_m, 12.5);
        assert!(
            serde_json::from_str::<StreamFix>(
                r#"{"lat":1.0,"lon":2.0,"accuracy_m":1.0,"ts_ms":0,"x":1}"#
            )
            .is_err()
        );
    }
}
