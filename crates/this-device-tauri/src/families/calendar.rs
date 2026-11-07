//! The calendar family: the pure part of the EventKit bridge. The types of a read window, of
//! the events it returns and of a new event; the checks applied before the device is touched;
//! the tests of the answers the native side sends back. No Tauri, no native code: it runs on
//! the host, as `crate::wire` does.
//!
//! The calendar is not a [`this_platform::device_bridge::Request`]: it has its own native
//! methods (`ios/Sources/calendar.swift`) and its own answers. Errors and permission states
//! reuse [`Reply`], as the other families do.

use serde::{Deserialize, Serialize};
use this_platform::device_bridge::{DeviceError, Reply};

/// The longest window a read may cover: 31 days, in milliseconds.
pub const WINDOW_MS_MAX: i64 = 31 * 24 * 60 * 60 * 1000;
/// The most events one read answers. The native side sends at most this many, and the rest of
/// the window is not read.
pub const EVENTS_MAX: usize = 200;

/// A read window, as Unix epoch milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// An event as the bridge reads it: title, dates, location. Nothing else is part of the
/// answer: no notes, no attachments, no attendees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub title: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub location: Option<String>,
}

/// An event to create in the default calendar. The title and both dates are required.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewEvent {
    pub title: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub location: Option<String>,
}

/// One calendar the person has, as the bridge lists it. `allows_modifications` is what decides
/// whether an event may be created or changed there: a read-only calendar (a subscribed one)
/// accepts a create and then refuses it, which is worse than saying so first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalendarSummary {
    pub id: String,
    pub title: String,
    pub allows_modifications: bool,
}

/// A change to an existing event. Every field but the `id` is optional: a patch names what it
/// changes, and a patch that changes nothing is refused rather than sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventPatch {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub start_ms: Option<i64>,
    #[serde(default)]
    pub end_ms: Option<i64>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl EventPatch {
    /// True when the patch asks for no change at all.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.start_ms.is_none()
            && self.end_ms.is_none()
            && self.location.is_none()
            && self.notes.is_none()
    }
}

/// What the calendar methods answer with data. A state or an error is a [`Reply`] instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CalendarAnswer {
    CalendarEvents { events: Vec<CalendarEvent> },
    CalendarCreated { event_id: Option<String> },
    Calendars { calendars: Vec<CalendarSummary> },
    EventUpdated { event_id: String },
    EventDeleted { event_id: String },
}

/// The refusal for an argument the native side must not receive.
fn refused() -> Reply {
    Reply::Error {
        code: DeviceError::Internal,
    }
}

/// Checks a read window: it runs forward and is no longer than [`WINDOW_MS_MAX`]. The
/// subtraction is checked, so extreme values are refused rather than wrapped.
pub fn check_window(window: Window) -> Result<(), Reply> {
    match window.end_ms.checked_sub(window.start_ms) {
        Some(span) if span > 0 && span <= WINDOW_MS_MAX => Ok(()),
        _ => Err(refused()),
    }
}

/// Checks a new event: the title is not blank, and the end is after the start. A new event
/// has no length bound: only the order of its two dates matters.
pub fn check_new_event(event: &NewEvent) -> Result<(), Reply> {
    if event.title.trim().is_empty() {
        return Err(refused());
    }
    match event.end_ms.checked_sub(event.start_ms) {
        Some(span) if span > 0 => Ok(()),
        _ => Err(refused()),
    }
}

/// The checked id of an event: blank is a caller bug, not an event to look for.
pub fn check_id(id: &str) -> Result<(), Reply> {
    if id.trim().is_empty() {
        Err(refused())
    } else {
        Ok(())
    }
}

/// The checked patch: it changes something, and if it gives both dates, the end is after the
/// start. A patch that gives only the end cannot be checked against the start it does not
/// know — the native side does that against the stored event, and answers `internal` if the
/// result would be an event that ends before it begins.
pub fn check_patch(patch: &EventPatch) -> Result<(), Reply> {
    check_id(&patch.id)?;
    if patch.is_empty() {
        return Err(refused());
    }
    if let (Some(start), Some(end)) = (patch.start_ms, patch.end_ms) {
        match end.checked_sub(start) {
            Some(span) if span > 0 => {}
            _ => return Err(refused()),
        }
    }
    if let Some(title) = &patch.title
        && title.trim().is_empty()
    {
        return Err(refused());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    fn window(start_ms: i64, end_ms: i64) -> Window {
        Window { start_ms, end_ms }
    }

    fn new_event(title: &str, start_ms: i64, end_ms: i64) -> NewEvent {
        NewEvent {
            title: title.into(),
            start_ms,
            end_ms,
            location: None,
        }
    }

    #[test]
    fn a_window_of_31_days_is_accepted() {
        assert_eq!(check_window(window(0, WINDOW_MS_MAX)), Ok(()));
        assert_eq!(check_window(window(0, 31 * DAY_MS)), Ok(()));
    }

    #[test]
    fn a_window_longer_than_31_days_is_refused() {
        assert_eq!(check_window(window(0, WINDOW_MS_MAX + 1)), Err(refused()));
        assert_eq!(check_window(window(0, 365 * DAY_MS)), Err(refused()));
    }

    #[test]
    fn an_empty_or_backwards_window_is_refused() {
        assert_eq!(check_window(window(5, 5)), Err(refused()));
        assert_eq!(check_window(window(10, 5)), Err(refused()));
    }

    #[test]
    fn an_overflowing_window_is_refused_not_wrapped() {
        assert_eq!(check_window(window(i64::MIN, i64::MAX)), Err(refused()));
        assert_eq!(check_window(window(i64::MAX, i64::MIN)), Err(refused()));
    }

    #[test]
    fn a_blank_title_is_refused() {
        assert_eq!(check_new_event(&new_event("", 0, 1000)), Err(refused()));
        assert_eq!(
            check_new_event(&new_event("   \t", 0, 1000)),
            Err(refused())
        );
    }

    #[test]
    fn an_end_not_after_the_start_is_refused() {
        assert_eq!(
            check_new_event(&new_event("Point", 1000, 1000)),
            Err(refused())
        );
        assert_eq!(
            check_new_event(&new_event("Point", 1000, 999)),
            Err(refused())
        );
    }

    #[test]
    fn a_new_event_has_no_length_bound() {
        // A read is bounded to 31 days; a new event is not.
        assert_eq!(
            check_new_event(&new_event("Congrès", 0, 400 * DAY_MS)),
            Ok(())
        );
    }

    #[test]
    fn a_valid_new_event_is_accepted() {
        let event = NewEvent {
            title: "Point".into(),
            start_ms: 1_700_000_000_000,
            end_ms: 1_700_000_900_000,
            location: Some("Salle 2".into()),
        };
        assert_eq!(check_new_event(&event), Ok(()));
    }

    #[test]
    fn events_decode_with_a_null_location() {
        // What the Swift side writes: `location` is `null` when the event has none.
        let answer: CalendarAnswer = serde_json::from_str(
            r#"{"kind":"calendar_events","events":[{"title":"Point","start_ms":1700000000000,"end_ms":1700003600000,"location":null},{"title":"Déjeuner","start_ms":1700010000000,"end_ms":1700013600000,"location":"Lyon"}]}"#,
        )
        .unwrap();
        assert_eq!(
            answer,
            CalendarAnswer::CalendarEvents {
                events: vec![
                    CalendarEvent {
                        title: "Point".into(),
                        start_ms: 1_700_000_000_000,
                        end_ms: 1_700_003_600_000,
                        location: None,
                    },
                    CalendarEvent {
                        title: "Déjeuner".into(),
                        start_ms: 1_700_010_000_000,
                        end_ms: 1_700_013_600_000,
                        location: Some("Lyon".into()),
                    },
                ],
            }
        );
    }

    #[test]
    fn an_empty_window_is_a_list_not_an_error() {
        let answer: CalendarAnswer =
            serde_json::from_str(r#"{"kind":"calendar_events","events":[]}"#).unwrap();
        assert_eq!(answer, CalendarAnswer::CalendarEvents { events: vec![] });
    }

    #[test]
    fn notes_and_attendees_are_not_part_of_the_answer() {
        // Even if a native side sent them, they are not read: the answer keeps the four fields.
        let answer: CalendarAnswer = serde_json::from_str(
            r#"{"kind":"calendar_events","events":[{"title":"Point","start_ms":1,"end_ms":2,"location":null,"notes":"secret","attendees":["a@b.c"]}]}"#,
        )
        .unwrap();
        let CalendarAnswer::CalendarEvents { events } = answer else {
            panic!("expected events");
        };
        assert_eq!(
            events,
            vec![CalendarEvent {
                title: "Point".into(),
                start_ms: 1,
                end_ms: 2,
                location: None,
            }]
        );
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap(),
            json!({"title": "Point", "start_ms": 1, "end_ms": 2, "location": null})
        );
    }

    #[test]
    fn a_created_event_answers_its_identifier_or_null() {
        let with: CalendarAnswer =
            serde_json::from_str(r#"{"kind":"calendar_created","event_id":"ABC-123"}"#).unwrap();
        assert_eq!(
            with,
            CalendarAnswer::CalendarCreated {
                event_id: Some("ABC-123".into())
            }
        );
        let without: CalendarAnswer =
            serde_json::from_str(r#"{"kind":"calendar_created","event_id":null}"#).unwrap();
        assert_eq!(without, CalendarAnswer::CalendarCreated { event_id: None });
    }

    #[test]
    fn an_unknown_calendar_kind_is_refused_not_guessed() {
        assert!(serde_json::from_str::<CalendarAnswer>(r#"{"kind":"calendar_deleted"}"#).is_err());
    }

    #[test]
    fn a_window_is_sent_with_the_keys_the_native_side_reads() {
        assert_eq!(
            serde_json::to_value(window(1, 2)).unwrap(),
            json!({"start_ms": 1, "end_ms": 2})
        );
    }

    #[test]
    fn a_new_event_is_sent_with_null_for_a_missing_location() {
        let value = serde_json::to_value(new_event("Point", 1, 2)).unwrap();
        assert_eq!(
            value,
            json!({"title": "Point", "start_ms": 1, "end_ms": 2, "location": null})
        );
    }

    #[test]
    fn the_bounds_are_the_ones_the_native_side_checks() {
        assert_eq!(WINDOW_MS_MAX, 2_678_400_000);
        assert_eq!(EVENTS_MAX, 200);
    }

    #[test]
    fn an_empty_patch_is_refused_instead_of_sent() {
        let vide = EventPatch {
            id: "E1".into(),
            title: None,
            start_ms: None,
            end_ms: None,
            location: None,
            notes: None,
        };
        assert!(vide.is_empty());
        assert!(check_patch(&vide).is_err());
    }

    #[test]
    fn a_patch_that_names_no_event_is_refused() {
        let sans_id = EventPatch {
            id: "  ".into(),
            title: Some("Rendez-vous".into()),
            start_ms: None,
            end_ms: None,
            location: None,
            notes: None,
        };
        assert!(check_patch(&sans_id).is_err());
        assert!(check_id("").is_err());
    }

    #[test]
    fn a_patch_with_both_dates_keeps_them_in_order() {
        let inversee = EventPatch {
            id: "E1".into(),
            title: None,
            start_ms: Some(2_000),
            end_ms: Some(1_000),
            location: None,
            notes: None,
        };
        assert!(check_patch(&inversee).is_err());

        let bonne = EventPatch {
            id: "E1".into(),
            title: Some("Dejeuner".into()),
            start_ms: Some(1_000),
            end_ms: Some(2_000),
            location: None,
            notes: None,
        };
        assert_eq!(check_patch(&bonne), Ok(()));
    }

    #[test]
    fn a_calendar_list_decodes_with_its_right_to_write() {
        let brut = r#"{"kind":"calendars","calendars":[{"id":"C1","title":"Perso","allows_modifications":true}]}"#;
        let answer: CalendarAnswer = serde_json::from_str(brut).unwrap();
        assert_eq!(
            answer,
            CalendarAnswer::Calendars {
                calendars: vec![CalendarSummary {
                    id: "C1".into(),
                    title: "Perso".into(),
                    allows_modifications: true,
                }],
            }
        );
    }

    #[test]
    fn an_update_carries_the_event_it_changed() {
        let brut = r#"{"kind":"event_updated","event_id":"E1"}"#;
        let answer: CalendarAnswer = serde_json::from_str(brut).unwrap();
        assert_eq!(
            answer,
            CalendarAnswer::EventUpdated {
                event_id: "E1".into()
            }
        );
        let supprime: CalendarAnswer =
            serde_json::from_str(r#"{"kind":"event_deleted","event_id":"E2"}"#).unwrap();
        assert_eq!(
            supprime,
            CalendarAnswer::EventDeleted {
                event_id: "E2".into()
            }
        );
    }

    #[test]
    fn a_calendar_summary_refuses_a_field_it_does_not_know() {
        // Les STRUCTS sont stricts : un champ ajouté côté natif par erreur est une erreur, pas
        // un silence. (L'enum, lui, est étiqueté par `kind` : serde ignore un champ en plus à ce
        // niveau — c'est pourquoi la vérification porte sur les types qui portent des données.)
        let brut = r#"{"id":"C1","title":"Perso","allows_modifications":true,"surprise":1}"#;
        assert!(serde_json::from_str::<CalendarSummary>(brut).is_err());
    }
}
