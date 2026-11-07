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

/// A request that names one contact, by the `id` a list or a search returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactRequest {
    pub id: String,
}

/// A search: the text to look for, and a bound on the answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsSearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// A contact to add to the person's book. `name` is required: a nameless card is a bug, not a
/// contact, and the book is not the place to store one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewContact {
    pub name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub phones: Vec<String>,
}

/// The most addresses, and the most phone numbers, one new contact may carry. A vCard with a
/// hundred of them is an import, and an import is not one call.
pub const HANDLES_MAX: usize = 20;

/// What the native contacts commands answer. Same `kind` convention as the device contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContactsReply {
    Status {
        state: PermissionState,
    },
    Contacts {
        contacts: Vec<Contact>,
    },
    /// One contact, by id or by creation.
    Contact {
        contact: Contact,
    },
    Error {
        code: DeviceError,
    },
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

/// The checked text of a search: blank would match the whole book, which is not a search.
pub fn checked_query(query: &str) -> Result<&str, ContactsReply> {
    let query = query.trim();
    if query.is_empty() {
        Err(ContactsReply::Error {
            code: DeviceError::MissingArgument,
        })
    } else {
        Ok(query)
    }
}

/// The checked shape of a new contact: a name, and no more handles than a card can hold.
pub fn checked_new_contact(contact: &NewContact) -> Result<(), ContactsReply> {
    let refus = |code| ContactsReply::Error { code };
    if contact.name.trim().is_empty() {
        return Err(refus(DeviceError::MissingArgument));
    }
    if contact.emails.len() > HANDLES_MAX || contact.phones.len() > HANDLES_MAX {
        return Err(refus(DeviceError::Internal));
    }
    if contact
        .emails
        .iter()
        .chain(contact.phones.iter())
        .any(|handle| handle.trim().is_empty())
    {
        return Err(refus(DeviceError::Internal));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_search_needs_a_text() {
        assert_eq!(
            checked_query("   "),
            Err(ContactsReply::Error {
                code: DeviceError::MissingArgument
            })
        );
        assert_eq!(checked_query(" Ada "), Ok("Ada"));
    }

    #[test]
    fn a_new_contact_needs_a_name_and_stays_small() {
        let vide = NewContact {
            name: "  ".into(),
            emails: vec![],
            phones: vec![],
        };
        assert!(checked_new_contact(&vide).is_err());

        let trop = NewContact {
            name: "Ada".into(),
            emails: (0..HANDLES_MAX + 1)
                .map(|i| format!("a{i}@ex.fr"))
                .collect(),
            phones: vec![],
        };
        assert!(checked_new_contact(&trop).is_err());

        let bon = NewContact {
            name: "Ada".into(),
            emails: vec!["ada@ex.fr".into()],
            phones: vec!["+33600000000".into()],
        };
        assert_eq!(checked_new_contact(&bon), Ok(()));
    }

    #[test]
    fn one_contact_decodes_in_its_own_envelope() {
        let brut = r#"{"kind":"contact","contact":{"id":"X","name":"Ada","emails":["a@b.fr"],"phones":[]}}"#;
        let reply: ContactsReply = serde_json::from_str(brut).unwrap();
        assert_eq!(
            reply,
            ContactsReply::Contact {
                contact: Contact {
                    id: "X".into(),
                    name: "Ada".into(),
                    emails: vec!["a@b.fr".into()],
                    phones: vec![],
                }
            }
        );
    }

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
