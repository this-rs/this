//! The local network family: the answers of the iOS side for `Capability::LocalNetwork`.
//!
//! The device contract (`this_platform::device_bridge::Reply`) has no local network answer, so
//! this family owns its wire shapes. They are the ones written by `ios/Sources/local_network.swift`
//! and `ios/Sources/Reply.swift`:
//!
//! - `{"kind":"status","state":..}`, the same as the contract's status answer;
//! - `{"kind":"services","services":[{"name":..,"service_type":..}]}`, one browse;
//! - `{"kind":"error","code":..}`, with the codes below.
//!
//! No IP address is ever part of an answer: a service is its name and its type, and any other
//! field is refused by the parser (`deny_unknown_fields`).
//!
//! Pure: no Tauri and no native code, so it is checked on the host.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::PermissionState;

/// The Bonjour service types a browse may name: a closed list. Each one must also appear in
/// `NSBonjourServices` of the app's Info.plist, or iOS refuses the browse. Mirrors
/// `LocalNetworkServiceTypes.allowed` in `ios/Sources/local_network.swift`.
pub const SERVICE_TYPES: [&str; 1] = ["_http._tcp"];

/// Whether a service type is in the closed list. Exact match: no case folding, no suffix.
pub fn is_service_type_allowed(service_type: &str) -> bool {
    SERVICE_TYPES.contains(&service_type)
}

/// The checked service type of a browse, or the refusal to send back. A type outside the list
/// is refused as `unsupported` before the device is touched.
pub fn checked_service_type(service_type: &str) -> Result<&str, LocalNetworkError> {
    if is_service_type_allowed(service_type) {
        Ok(service_type)
    } else {
        Err(LocalNetworkError::Unsupported)
    }
}

/// The arguments of the native `localNetworkBrowse` method.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseArgs {
    pub service_type: String,
}

/// A service found by a browse: its name and its type. Nothing else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalService {
    pub name: String,
    pub service_type: String,
}

/// Why a local network request failed. `unsupported` is this family's own code: the device
/// contract names the same refusal `not_supported`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalNetworkError {
    /// The system refused the local network permission.
    NotAuthorized,
    /// The network cannot answer now (no radio, browser failed).
    Unavailable,
    /// Something went wrong on the native side.
    Internal,
    /// The service type is not in [`SERVICE_TYPES`].
    Unsupported,
}

/// What the native side answers for this family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalNetworkReply {
    Status { state: PermissionState },
    Services { services: Vec<LocalService> },
    Error { code: LocalNetworkError },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_closed_list_holds_the_http_service() {
        assert_eq!(SERVICE_TYPES, ["_http._tcp"]);
    }

    #[test]
    fn a_listed_service_type_is_accepted() {
        assert_eq!(checked_service_type("_http._tcp"), Ok("_http._tcp"));
        assert!(is_service_type_allowed("_http._tcp"));
    }

    #[test]
    fn a_service_type_outside_the_list_is_refused_as_unsupported() {
        for bad in [
            "_ssh._tcp",
            "_http._udp",
            "_http._tcp.",
            "_HTTP._tcp",
            " _http._tcp",
            "_http._tcp,_ssh._tcp",
            "",
        ] {
            assert!(!is_service_type_allowed(bad), "{bad:?} must be refused");
            assert_eq!(
                checked_service_type(bad),
                Err(LocalNetworkError::Unsupported),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_browse_arguments_carry_the_service_type() {
        let args = BrowseArgs {
            service_type: "_http._tcp".into(),
        };
        assert_eq!(
            serde_json::to_value(&args).unwrap(),
            json!({"service_type": "_http._tcp"})
        );
    }

    #[test]
    fn the_status_answer_decodes_into_the_permission_states() {
        for (wire, state) in [
            ("granted", PermissionState::Granted),
            ("denied", PermissionState::Denied),
            ("prompt", PermissionState::Prompt),
            ("unsupported", PermissionState::Unsupported),
        ] {
            let json = format!(r#"{{"kind":"status","state":"{wire}"}}"#);
            let reply: LocalNetworkReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, LocalNetworkReply::Status { state }, "{wire}");
        }
    }

    #[test]
    fn the_services_answer_decodes_names_and_types() {
        // What ios/Sources/local_network.swift sends for one browse.
        let reply: LocalNetworkReply = serde_json::from_str(
            r#"{"kind":"services","services":[{"name":"Imprimante","service_type":"_http._tcp"}]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            LocalNetworkReply::Services {
                services: vec![LocalService {
                    name: "Imprimante".into(),
                    service_type: "_http._tcp".into(),
                }]
            }
        );
    }

    #[test]
    fn an_empty_browse_is_a_list_not_an_error() {
        let reply: LocalNetworkReply =
            serde_json::from_str(r#"{"kind":"services","services":[]}"#).unwrap();
        assert_eq!(reply, LocalNetworkReply::Services { services: vec![] });
    }

    #[test]
    fn every_error_code_the_native_side_sends_is_decoded() {
        for (wire, code) in [
            ("not_authorized", LocalNetworkError::NotAuthorized),
            ("unavailable", LocalNetworkError::Unavailable),
            ("internal", LocalNetworkError::Internal),
            ("unsupported", LocalNetworkError::Unsupported),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: LocalNetworkReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, LocalNetworkReply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn a_service_carrying_an_address_is_refused() {
        // No IP address may cross the bridge, even if the native side sent one.
        for extra in [
            r#""ip":"192.168.1.10""#,
            r#""address":"fe80::1""#,
            r#""port":8080"#,
        ] {
            let json = format!(
                r#"{{"kind":"services","services":[{{"name":"A","service_type":"_http._tcp",{extra}}}]}}"#
            );
            assert!(
                serde_json::from_str::<LocalNetworkReply>(&json).is_err(),
                "{extra} must be refused"
            );
        }
    }

    #[test]
    fn an_unknown_kind_or_code_is_refused_not_guessed() {
        assert!(
            serde_json::from_str::<LocalNetworkReply>(r#"{"kind":"peers","peers":[]}"#).is_err()
        );
        assert!(
            serde_json::from_str::<LocalNetworkReply>(r#"{"kind":"error","code":"not_supported"}"#)
                .is_err(),
            "the device contract's code is not this family's code"
        );
    }
}
