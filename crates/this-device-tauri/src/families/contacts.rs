//! The contacts family: the wire types of the contacts bridge and the check of the requested
//! size. Pure: no Tauri, no native code. The native side is `ios/Sources/contacts.swift`.
//!
//! PERSONAL DATA OF THIRD PARTIES. A contact is a person who never agreed to this app. The
//! native side reads only the fields of [`Contact`], and [`Contact`] refuses any other field
//! on the way back, so a key added on the Swift side by mistake is an error, not a leak.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::{DeviceError, PermissionState};

/// The size of a list when the request does not name one.
pub const LIMIT_DEFAULT: u32 = 50;
/// The smallest list asked for.
pub const LIMIT_MIN: u32 = 1;
/// The largest list asked for. Above it, a list is refused, not clamped: a caller that asks for
/// more than this has a bug, and a silent cut would hide it.
pub const LIMIT_MAX: u32 = 200;

fn default_limit() -> u32 {
    LIMIT_DEFAULT
}

/// A request for a list of contacts: at most `limit` of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsListRequest {
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// One contact. `name` is the given and family names joined by a space, empty when the book
/// holds neither. Any other field in the answer is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
}

/// What the native contacts commands answer. Same `kind` convention as the device contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContactsReply {
    Status { state: PermissionState },
    Contacts { contacts: Vec<Contact> },
    Error { code: DeviceError },
}

/// The checked limit of a list, or the refusal to send back.
///
/// A limit outside `LIMIT_MIN..=LIMIT_MAX` (so `0` and anything above 200) is refused with
/// [`DeviceError::Internal`]: the contract has no code for a bad argument, and `Internal` is
/// the code the device bridge already uses for a refused scan filter.
pub fn checked_limit(limit: u32) -> Result<u32, ContactsReply> {
    if (LIMIT_MIN..=LIMIT_MAX).contains(&limit) {
        Ok(limit)
    } else {
        Err(ContactsReply::Error {
            code: DeviceError::Internal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_without_a_limit_asks_for_the_default() {
        let request: ContactsListRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(request.limit, LIMIT_DEFAULT);
        assert_eq!(LIMIT_DEFAULT, 50);
    }

    #[test]
    fn a_request_with_a_limit_keeps_it() {
        let request: ContactsListRequest = serde_json::from_str(r#"{"limit":10}"#).unwrap();
        assert_eq!(request, ContactsListRequest { limit: 10 });
    }

    #[test]
    fn a_negative_or_fractional_limit_is_refused_by_the_decoder() {
        assert!(serde_json::from_str::<ContactsListRequest>(r#"{"limit":-1}"#).is_err());
        assert!(serde_json::from_str::<ContactsListRequest>(r#"{"limit":1.5}"#).is_err());
        assert!(serde_json::from_str::<ContactsListRequest>(r#"{"limit":"10"}"#).is_err());
    }

    #[test]
    fn the_request_serializes_to_the_payload_the_swift_side_reads() {
        let request = ContactsListRequest { limit: 50 };
        assert_eq!(serde_json::to_value(request).unwrap(), json!({"limit": 50}));
    }

    #[test]
    fn the_limit_bounds_are_inclusive_and_zero_and_above_200_are_refused() {
        assert_eq!(checked_limit(LIMIT_MIN), Ok(1));
        assert_eq!(checked_limit(50), Ok(50));
        assert_eq!(checked_limit(LIMIT_MAX), Ok(200));
        for refused in [0, 201, 10_000, u32::MAX] {
            assert_eq!(
                checked_limit(refused),
                Err(ContactsReply::Error {
                    code: DeviceError::Internal
                }),
                "{refused}"
            );
        }
    }

    // What the Swift side sends, byte for byte (see ios/Sources/contacts.swift).
    #[test]
    fn a_contacts_answer_decodes_into_the_contract() {
        let reply: ContactsReply = serde_json::from_str(
            r#"{"kind":"contacts","contacts":[{"id":"ABC-1","name":"Ada Lovelace","emails":["ada@example.org"],"phones":["+33 1 23 45 67 89"]}]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            ContactsReply::Contacts {
                contacts: vec![Contact {
                    id: "ABC-1".into(),
                    name: "Ada Lovelace".into(),
                    emails: vec!["ada@example.org".into()],
                    phones: vec!["+33 1 23 45 67 89".into()],
                }],
            }
        );
    }

    #[test]
    fn an_empty_book_is_a_list_not_an_error() {
        let reply: ContactsReply =
            serde_json::from_str(r#"{"kind":"contacts","contacts":[]}"#).unwrap();
        assert_eq!(reply, ContactsReply::Contacts { contacts: vec![] });
    }

    #[test]
    fn a_contact_without_a_name_or_addresses_has_empty_fields() {
        let reply: ContactsReply = serde_json::from_str(
            r#"{"kind":"contacts","contacts":[{"id":"X","name":"","emails":[],"phones":[]}]}"#,
        )
        .unwrap();
        assert_eq!(
            reply,
            ContactsReply::Contacts {
                contacts: vec![Contact {
                    id: "X".into(),
                    name: String::new(),
                    emails: vec![],
                    phones: vec![],
                }],
            }
        );
    }

    #[test]
    fn a_field_beyond_the_contract_is_refused_not_kept() {
        // A birthday, a note or a photo must never cross the bridge unnoticed.
        for extra in [
            r#""birthday":"1815-12-10""#,
            r#""note":"private""#,
            r#""imageData":"AAAA""#,
        ] {
            let json = format!(
                r#"{{"kind":"contacts","contacts":[{{"id":"X","name":"Y","emails":[],"phones":[],{extra}}}]}}"#
            );
            assert!(
                serde_json::from_str::<ContactsReply>(&json).is_err(),
                "{extra}"
            );
        }
    }

    #[test]
    fn the_status_and_error_answers_are_the_device_contract_codes() {
        let status: ContactsReply =
            serde_json::from_str(r#"{"kind":"status","state":"prompt"}"#).unwrap();
        assert_eq!(
            status,
            ContactsReply::Status {
                state: PermissionState::Prompt
            }
        );
        for (wire, code) in [
            ("not_authorized", DeviceError::NotAuthorized),
            ("internal", DeviceError::Internal),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: ContactsReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, ContactsReply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn an_unknown_kind_is_refused_not_guessed() {
        assert!(serde_json::from_str::<ContactsReply>(r#"{"kind":"photos","items":[]}"#).is_err());
    }
}
