//! Notifications and live activity, on iOS: the native commands that answer the permission of
//! `Capability::Notifications` and `Capability::LiveActivity`, and the wire contract they speak.
//!
//! The types are the device bridge's own ([`PermissionState`], [`DeviceError`], [`Reply`]): this
//! module adds only the names of the native commands and the tests of the JSON they send back.
//! The Swift side is `ios/Sources/notifications.swift` and `ios/Sources/live_activity.swift`.
//! No notification is planned here. Les activités en direct se démarrent, se mettent à jour et
//! se terminent par [`ActivityRequest`] ; la réponse est une [`ActivityReply`].

use serde::{Deserialize, Serialize};
use this_platform::capability::Capability;
pub use this_platform::device_bridge::{DeviceError, PermissionState, Reply};

/// A native command of the notification and live activity family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifLiveCommand {
    /// Reads the notification permission, without prompting the person.
    NotificationsStatus,
    /// Asks the person for the notification permission, then answers the state.
    NotificationsRequest,
    /// Reads whether live activities are enabled, without starting one.
    LiveActivityStatus,
}

impl NotifLiveCommand {
    /// The `@objc` method on `ThisDevicePlugin` that answers the command.
    pub const fn native_method(self) -> &'static str {
        match self {
            NotifLiveCommand::NotificationsStatus => "notificationsStatus",
            NotifLiveCommand::NotificationsRequest => "notificationsRequest",
            NotifLiveCommand::LiveActivityStatus => "liveActivityStatus",
        }
    }

    /// The capability whose permission the command reads or asks for.
    pub const fn capability(self) -> Capability {
        match self {
            NotifLiveCommand::NotificationsStatus | NotifLiveCommand::NotificationsRequest => {
                Capability::Notifications
            }
            NotifLiveCommand::LiveActivityStatus => Capability::LiveActivity,
        }
    }
}

/// Les trois gestes sur une Live Activity.
///
/// Le contrat `Request` appartient à `this-platform` ; une famille y entre par `Request::Call`
/// (méthode + arguments). Cette énumération est donc le CORPS typé de ces trois appels, pas une
/// variante de `Request` : `method()` donne le sélecteur Swift, `args()` le JSON à envoyer.
///
/// À brancher dans `this_platform::device_bridge` : les trois méthodes ci-dessous doivent être
/// ajoutées à sa table (capacité `LiveActivity`, iOS seulement), sinon `answer` les refuse en
/// `not_supported` — voulu, c'est la porte du consentement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ActivityRequest {
    /// Démarre une activité. `titre` est obligatoire : sans lui l'écran verrouillé serait vide.
    Start {
        titre: String,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        progression: Option<f64>,
    },
    /// Met à jour l'activité `id`.
    Update {
        id: String,
        titre: String,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        progression: Option<f64>,
    },
    /// Termine l'activité `id` et la retire de l'écran.
    End { id: String },
}

/// Pourquoi une requête est refusée avant même de toucher l'appareil.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityRefusal {
    /// `titre` vide ou blanc.
    TitreAbsent,
    /// `id` vide pour une mise à jour ou une fin.
    IdAbsent,
    /// `progression` hors de 0…1 (ou NaN).
    ProgressionInvalide,
}

impl ActivityRefusal {
    /// Le code que voit l'appelant : nommé, jamais un succès silencieux.
    pub const fn code(self) -> &'static str {
        match self {
            ActivityRefusal::TitreAbsent => "titre_absent",
            ActivityRefusal::IdAbsent => "id_absent",
            ActivityRefusal::ProgressionInvalide => "progression_invalide",
        }
    }
}

impl ActivityRequest {
    /// Le `@objc` de `ThisDevicePlugin` qui répond (`ios/Sources/live_activity.swift`).
    pub const fn method(&self) -> &'static str {
        match self {
            ActivityRequest::Start { .. } => "liveActivityStart",
            ActivityRequest::Update { .. } => "liveActivityUpdate",
            ActivityRequest::End { .. } => "liveActivityEnd",
        }
    }

    /// Vérifie la requête sans toucher l'appareil : on refuse ici ce que le natif ne saurait
    /// de toute façon pas peindre, pour que l'erreur nomme la cause.
    pub fn validate(&self) -> Result<(), ActivityRefusal> {
        let progression_ok = |p: &Option<f64>| match p {
            None => true,
            Some(p) => (0.0..=1.0).contains(p), // faux pour NaN
        };
        match self {
            ActivityRequest::Start {
                titre, progression, ..
            } => {
                if titre.trim().is_empty() {
                    return Err(ActivityRefusal::TitreAbsent);
                }
                if !progression_ok(progression) {
                    return Err(ActivityRefusal::ProgressionInvalide);
                }
            }
            ActivityRequest::Update {
                id,
                titre,
                progression,
                ..
            } => {
                if id.trim().is_empty() {
                    return Err(ActivityRefusal::IdAbsent);
                }
                if titre.trim().is_empty() {
                    return Err(ActivityRefusal::TitreAbsent);
                }
                if !progression_ok(progression) {
                    return Err(ActivityRefusal::ProgressionInvalide);
                }
            }
            ActivityRequest::End { id } => {
                if id.trim().is_empty() {
                    return Err(ActivityRefusal::IdAbsent);
                }
            }
        }
        Ok(())
    }

    /// Les arguments tels que le Swift les décode (`ActivityArgs`), sans la balise `op` : la
    /// méthode appelée la porte déjà.
    pub fn args(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        if let Some(map) = value.as_object_mut() {
            map.remove("op");
        }
        value
    }
}

/// La réponse d'une famille activité. Forme propre à la famille (comme les autres) : `Reply`
/// est le contrat du pilote et ne porte pas d'identifiant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityReply {
    /// L'identifiant de l'activité ; `None` quand il n'y en a plus (fin réussie).
    Activity { id: Option<String> },
    /// Échec nommé : `not_authorized`, `unknown_activity`, `unavailable`, `titre_absent`…
    Error { code: String },
}

/// Les bornes d'une planification, en millisecondes. En dessous d'une seconde, le système
/// déclenche « tout de suite » et la promesse « dans un instant » est fausse ; au-delà de trente
/// jours, ce n'est plus un rappel, c'est une date.
pub const SCHEDULE_MS_MIN: i64 = 1_000;
pub const SCHEDULE_MS_MAX: i64 = 30 * 24 * 60 * 60 * 1000;
/// Le plus de notifications en attente qu'une lecture rend.
pub const PENDING_MAX: usize = 50;

/// One notification waiting to be shown.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingEntry {
    pub id: String,
    pub title: String,
    pub in_ms: i64,
}

/// Ce que la planification et la lecture des attentes répondent.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduledReply {
    Scheduled { id: String, in_ms: i64 },
    Pending { notifications: Vec<PendingEntry> },
    Error { code: String },
}

/// Le délai vérifié d'une planification : un texte, et une échéance dans les bornes.
pub fn checked_schedule(title: &str, in_ms: i64) -> Result<i64, ScheduledReply> {
    if title.trim().is_empty() || !(SCHEDULE_MS_MIN..=SCHEDULE_MS_MAX).contains(&in_ms) {
        return Err(ScheduledReply::Error {
            code: "internal".to_string(),
        });
    }
    Ok(in_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ALL: [NotifLiveCommand; 3] = [
        NotifLiveCommand::NotificationsStatus,
        NotifLiveCommand::NotificationsRequest,
        NotifLiveCommand::LiveActivityStatus,
    ];

    #[test]
    fn each_command_names_its_swift_method() {
        assert_eq!(
            NotifLiveCommand::NotificationsStatus.native_method(),
            "notificationsStatus"
        );
        assert_eq!(
            NotifLiveCommand::NotificationsRequest.native_method(),
            "notificationsRequest"
        );
        assert_eq!(
            NotifLiveCommand::LiveActivityStatus.native_method(),
            "liveActivityStatus"
        );
    }

    #[test]
    fn each_command_maps_to_its_capability() {
        assert_eq!(
            NotifLiveCommand::NotificationsStatus.capability(),
            Capability::Notifications
        );
        assert_eq!(
            NotifLiveCommand::NotificationsRequest.capability(),
            Capability::Notifications
        );
        assert_eq!(
            NotifLiveCommand::LiveActivityStatus.capability(),
            Capability::LiveActivity
        );
    }

    #[test]
    fn the_capability_names_are_the_contract_names() {
        assert_eq!(
            serde_json::to_value(Capability::Notifications).unwrap(),
            json!("notifications")
        );
        assert_eq!(
            serde_json::to_value(Capability::LiveActivity).unwrap(),
            json!("live_activity")
        );
    }

    #[test]
    fn every_command_is_bridged_to_a_catalogue_capability() {
        for command in ALL {
            assert!(
                Capability::ALL.contains(&command.capability()),
                "{command:?}"
            );
        }
    }

    // What ios/Sources/notifications.swift and live_activity.swift send, byte for byte.
    #[test]
    fn a_notification_status_decodes_into_the_contract() {
        for (wire, state) in [
            ("granted", PermissionState::Granted),
            ("denied", PermissionState::Denied),
            ("prompt", PermissionState::Prompt),
        ] {
            let json = format!(r#"{{"kind":"status","state":"{wire}"}}"#);
            let reply: Reply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, Reply::Status { state }, "{wire}");
        }
    }

    #[test]
    fn a_live_activity_status_decodes_into_the_contract() {
        let enabled: Reply =
            serde_json::from_str(r#"{"kind":"status","state":"granted"}"#).unwrap();
        assert_eq!(
            enabled,
            Reply::Status {
                state: PermissionState::Granted
            }
        );
        let disabled: Reply =
            serde_json::from_str(r#"{"kind":"status","state":"denied"}"#).unwrap();
        assert_eq!(
            disabled,
            Reply::Status {
                state: PermissionState::Denied
            }
        );
        let before_16_1: Reply =
            serde_json::from_str(r#"{"kind":"status","state":"unsupported"}"#).unwrap();
        assert_eq!(
            before_16_1,
            Reply::Status {
                state: PermissionState::Unsupported
            }
        );
    }

    #[test]
    fn a_failed_permission_request_is_an_internal_error_not_a_denial() {
        let reply: Reply = serde_json::from_str(r#"{"kind":"error","code":"internal"}"#).unwrap();
        assert_eq!(
            reply,
            Reply::Error {
                code: DeviceError::Internal
            }
        );
    }

    #[test]
    fn a_native_status_with_an_unknown_state_is_refused_not_guessed() {
        assert!(serde_json::from_str::<Reply>(r#"{"kind":"status","state":"ephemeral"}"#).is_err());
    }

    #[test]
    fn the_three_requests_round_trip() {
        for request in [
            ActivityRequest::Start {
                titre: "Build".into(),
                detail: "étape 2".into(),
                progression: Some(0.5),
            },
            ActivityRequest::Update {
                id: "A1".into(),
                titre: "Build".into(),
                detail: String::new(),
                progression: None,
            },
            ActivityRequest::End { id: "A1".into() },
        ] {
            let json = serde_json::to_string(&request).unwrap();
            let back: ActivityRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(back, request, "{json}");
        }
    }

    #[test]
    fn each_request_names_its_swift_method_and_drops_the_tag() {
        let start = ActivityRequest::Start {
            titre: "t".into(),
            detail: "d".into(),
            progression: Some(1.0),
        };
        assert_eq!(start.method(), "liveActivityStart");
        assert_eq!(
            start.args(),
            json!({"titre":"t","detail":"d","progression":1.0})
        );
        let end = ActivityRequest::End { id: "x".into() };
        assert_eq!(end.method(), "liveActivityEnd");
        assert_eq!(end.args(), json!({"id":"x"}));
        let update = ActivityRequest::Update {
            id: "x".into(),
            titre: "t".into(),
            detail: String::new(),
            progression: None,
        };
        assert_eq!(update.method(), "liveActivityUpdate");
    }

    #[test]
    fn a_start_without_title_is_refused() {
        // Clé absente : le décodage lui-même échoue.
        assert!(serde_json::from_str::<ActivityRequest>(r#"{"op":"start","detail":"d"}"#).is_err());
        // Titre vide ou blanc : décodé, mais refusé avant l'appareil.
        for titre in ["", "   "] {
            let request = ActivityRequest::Start {
                titre: titre.into(),
                detail: String::new(),
                progression: None,
            };
            assert_eq!(request.validate(), Err(ActivityRefusal::TitreAbsent));
        }
    }

    #[test]
    fn an_update_or_end_needs_an_id_and_progress_stays_in_range() {
        assert_eq!(
            ActivityRequest::End { id: " ".into() }.validate(),
            Err(ActivityRefusal::IdAbsent)
        );
        for p in [-0.1, 1.1, f64::NAN] {
            let request = ActivityRequest::Start {
                titre: "t".into(),
                detail: String::new(),
                progression: Some(p),
            };
            assert_eq!(
                request.validate(),
                Err(ActivityRefusal::ProgressionInvalide)
            );
        }
        let ok = ActivityRequest::Start {
            titre: "t".into(),
            detail: String::new(),
            progression: Some(0.0),
        };
        assert_eq!(ok.validate(), Ok(()));
    }

    #[test]
    fn the_activity_reply_round_trips_with_id_or_none_or_error() {
        for (wire, reply) in [
            (
                r#"{"kind":"activity","id":"A1"}"#,
                ActivityReply::Activity {
                    id: Some("A1".into()),
                },
            ),
            (
                r#"{"kind":"activity","id":null}"#,
                ActivityReply::Activity { id: None },
            ),
            (
                r#"{"kind":"error","code":"not_authorized"}"#,
                ActivityReply::Error {
                    code: "not_authorized".into(),
                },
            ),
        ] {
            assert_eq!(serde_json::from_str::<ActivityReply>(wire).unwrap(), reply);
            assert_eq!(serde_json::to_string(&reply).unwrap(), wire);
        }
    }

    #[test]
    fn a_schedule_keeps_its_delay_in_the_native_range() {
        assert!(checked_schedule("Rappel", 500).is_err());
        assert!(checked_schedule("Rappel", 60_000).is_ok());
        assert!(checked_schedule("Rappel", 60 * 24 * 60 * 60 * 1000).is_err());
        assert!(checked_schedule("   ", 60_000).is_err());
    }

    #[test]
    fn the_pending_list_and_the_scheduled_answer_decode() {
        let scheduled: ScheduledReply =
            serde_json::from_str(r#"{"kind":"scheduled","id":"a","in_ms":60000}"#).unwrap();
        assert_eq!(
            scheduled,
            ScheduledReply::Scheduled {
                id: "a".into(),
                in_ms: 60_000
            }
        );
        let pending: ScheduledReply = serde_json::from_str(
            r#"{"kind":"pending","notifications":[{"id":"a","title":"Rappel","in_ms":60000}]}"#,
        )
        .unwrap();
        assert_eq!(
            pending,
            ScheduledReply::Pending {
                notifications: vec![PendingEntry {
                    id: "a".into(),
                    title: "Rappel".into(),
                    in_ms: 60_000
                }]
            }
        );
    }

    #[test]
    fn a_pending_entry_refuses_a_field_it_does_not_know() {
        assert!(
            serde_json::from_str::<PendingEntry>(r#"{"id":"a","title":"t","in_ms":1,"x":2}"#)
                .is_err()
        );
    }
}
