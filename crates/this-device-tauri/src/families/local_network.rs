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
pub const SERVICE_TYPES: [&str; 2] = ["_http._tcp", ATELIER_SERVICE_TYPE];

/// The Bonjour service type an Atelier announces itself with. Browse, resolve and advertise all
/// accept it. Must appear in `NSBonjourServices` of the app's Info.plist (iOS refuses a browse of
/// an undeclared type silently, with the Bonjour errors -72008 / NoAuth).
pub const ATELIER_SERVICE_TYPE: &str = "_atelier._tcp";

/// The types a service may be *published* under: only the Atelier's own. Announcing as
/// `_http._tcp` would pose as a web server. Mirrors `LocalNetworkServiceTypes.advertisable`
/// (iOS) and `MacLocalNetwork.advertisableTypes` (macOS).
pub const ADVERTISE_TYPES: [&str; 1] = [ATELIER_SERVICE_TYPE];

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

/// The arguments of the native `localNetworkAdvertise` method: announce this Atelier.
///
/// `id`, `proto` and `role` are the whole TXT record. `name` is the Bonjour instance name; when
/// absent the native side builds `Atelier-<first 8 of id>`. No address is sent: the system
/// publishes the host, and the port is the one of the server the Atelier already runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvertiseArgs {
    pub service_type: String,
    pub port: u16,
    pub id: String,
    pub proto: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Longest TXT value or instance name accepted. Bonjour allows 255 bytes per TXT entry and 63 per
/// instance name; this stays well under both.
pub const MAX_FIELD_LEN: usize = 48;

/// A TXT field: 1 to [`MAX_FIELD_LEN`] characters of `[A-Za-z0-9._-]`. The closed alphabet is
/// what keeps a person's name, a space, an `@`, an IP literal with `:` or a secret blob out of
/// the record: nothing free-form fits.
fn is_txt_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FIELD_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Whether a TXT value looks like an IPv4 address (digits and dots only, three dots).
fn looks_like_ipv4(value: &str) -> bool {
    value.chars().all(|c| c.is_ascii_digit() || c == '.') && value.matches('.').count() == 3
}

/// The checked advertise request, or the refusal to send back (`unsupported` for a type outside
/// [`ADVERTISE_TYPES`], `internal` for a malformed field or a zero port). Checked before the
/// device is touched.
pub fn checked_advertise(args: &AdvertiseArgs) -> Result<&AdvertiseArgs, LocalNetworkError> {
    if !ADVERTISE_TYPES.contains(&args.service_type.as_str()) {
        return Err(LocalNetworkError::Unsupported);
    }
    if args.port == 0 {
        return Err(LocalNetworkError::Internal);
    }
    for field in [&args.id, &args.proto, &args.role] {
        if !is_txt_token(field) || looks_like_ipv4(field) {
            return Err(LocalNetworkError::Internal);
        }
    }
    if let Some(name) = &args.name {
        if !is_txt_token(name) || looks_like_ipv4(name) {
            return Err(LocalNetworkError::Internal);
        }
    }
    Ok(args)
}

/// The arguments of the native `localNetworkUnadvertise` method: stop announcing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnadvertiseArgs {
    pub service_type: String,
}

/// What the native side answers for this family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalNetworkReply {
    Status {
        state: PermissionState,
    },
    Services {
        services: Vec<LocalService>,
    },
    /// The service is announced. `name` is the instance name actually published.
    Advertised {
        name: String,
    },
    /// The announcement is withdrawn (or there was none: stopping is idempotent).
    Unadvertised,
    Error {
        code: LocalNetworkError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_closed_list_holds_the_http_and_the_atelier_services() {
        assert_eq!(SERVICE_TYPES, ["_http._tcp", "_atelier._tcp"]);
        assert!(is_service_type_allowed("_atelier._tcp"));
        assert_eq!(ADVERTISE_TYPES, ["_atelier._tcp"]);
        for t in ADVERTISE_TYPES {
            assert!(is_service_type_allowed(t), "{t} must also be browsable");
        }
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

/// Une résolution : le nom du service, son type, et la borne de temps.
///
/// `host` est une adresse ou un nom sur le réseau local. C'est une donnée d'infrastructure, pas
/// une donnée de la personne : elle est rendue telle quelle, sans être enregistrée nulle part.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    pub host: String,
    pub port: u16,
}

/// Le délai d'une résolution : borné. Une résolution qui ne répond pas doit rendre la main.
pub fn clamp_resolve_timeout(timeout_ms: u32) -> u32 {
    timeout_ms.clamp(1_000, 15_000)
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn a_resolution_timeout_is_bounded() {
        assert_eq!(clamp_resolve_timeout(1), 1_000);
        assert_eq!(clamp_resolve_timeout(5_000), 5_000);
        assert_eq!(clamp_resolve_timeout(99_000), 15_000);
    }

    #[test]
    fn a_resolved_service_keeps_its_host_and_port() {
        let resolved: Resolved =
            serde_json::from_str(r#"{"host":"macbook.local.","port":9902}"#).unwrap();
        assert_eq!(resolved.port, 9902);
        assert!(serde_json::from_str::<Resolved>(r#"{"host":"x","port":1,"extra":true}"#).is_err());
    }
}

#[cfg(test)]
mod advertise_tests {
    use super::*;
    use serde_json::json;

    fn args() -> AdvertiseArgs {
        AdvertiseArgs {
            service_type: "_atelier._tcp".into(),
            port: 9902,
            id: "a1b2c3d4e5f6".into(),
            proto: "1".into(),
            role: "desktop".into(),
            name: None,
        }
    }

    #[test]
    fn a_well_formed_announcement_is_accepted() {
        let a = args();
        assert_eq!(checked_advertise(&a), Ok(&a));
    }

    #[test]
    fn only_the_atelier_type_may_be_announced() {
        for bad in [
            "_http._tcp",
            "_ssh._tcp",
            "_atelier._udp",
            "_ATELIER._tcp",
            "",
        ] {
            let mut a = args();
            a.service_type = bad.into();
            assert_eq!(
                checked_advertise(&a),
                Err(LocalNetworkError::Unsupported),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_txt_field_cannot_carry_a_name_an_address_or_a_secret() {
        for bad in [
            "Théotime Rivière",
            "theo@example.com",
            "192.168.1.10",
            "fe80::1",
            "sk-abc def",
            "",
            &"x".repeat(MAX_FIELD_LEN + 1),
        ] {
            for field in 0..3 {
                let mut a = args();
                match field {
                    0 => a.id = bad.into(),
                    1 => a.proto = bad.into(),
                    _ => a.role = bad.into(),
                }
                assert_eq!(
                    checked_advertise(&a),
                    Err(LocalNetworkError::Internal),
                    "{bad:?} in field {field}"
                );
            }
        }
        let mut a = args();
        a.name = Some("Théotime".into());
        assert_eq!(checked_advertise(&a), Err(LocalNetworkError::Internal));
    }

    #[test]
    fn a_zero_port_is_refused() {
        let mut a = args();
        a.port = 0;
        assert_eq!(checked_advertise(&a), Err(LocalNetworkError::Internal));
    }

    #[test]
    fn the_advertise_arguments_have_exactly_the_wire_shape() {
        assert_eq!(
            serde_json::to_value(args()).unwrap(),
            json!({"service_type":"_atelier._tcp","port":9902,"id":"a1b2c3d4e5f6","proto":"1","role":"desktop"})
        );
        // A TXT field the contract does not know (an ip, a user name) is refused at parse time.
        for extra in [r#""ip":"10.0.0.2""#, r#""user":"theo""#, r#""secret":"x""#] {
            let j = format!(
                r#"{{"service_type":"_atelier._tcp","port":1,"id":"a","proto":"1","role":"r",{extra}}}"#
            );
            assert!(
                serde_json::from_str::<AdvertiseArgs>(&j).is_err(),
                "{extra}"
            );
        }
    }

    #[test]
    fn the_advertise_answers_decode_and_unknown_fields_are_not_guessed() {
        let r: LocalNetworkReply =
            serde_json::from_str(r#"{"kind":"advertised","name":"Atelier-a1b2c3d4"}"#).unwrap();
        assert_eq!(
            r,
            LocalNetworkReply::Advertised {
                name: "Atelier-a1b2c3d4".into()
            }
        );
        let r: LocalNetworkReply = serde_json::from_str(r#"{"kind":"unadvertised"}"#).unwrap();
        assert_eq!(r, LocalNetworkReply::Unadvertised);
        assert!(
            serde_json::from_str::<UnadvertiseArgs>(r#"{"service_type":"_atelier._tcp","x":1}"#)
                .is_err()
        );
    }
}
