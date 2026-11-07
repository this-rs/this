//! Notifications and live activity, on iOS: the native commands that answer the permission of
//! `Capability::Notifications` and `Capability::LiveActivity`, and the wire contract they speak.
//!
//! The types are the device bridge's own ([`PermissionState`], [`DeviceError`], [`Reply`]): this
//! module adds only the names of the native commands and the tests of the JSON they send back.
//! The Swift side is `ios/Sources/notifications.swift` and `ios/Sources/live_activity.swift`.
//! No notification is planned here, and no activity is started.

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
}
