//! Device capabilities, and the declarations each one requires.
//!
//! The app asks for a capability; the shell answers. But before asking, the system needs a
//! declaration: on iOS an app that requests the camera without `NSCameraUsageDescription`
//! in `Info.plist` is terminated by the system, not refused. [`Capability::keys`] gives the
//! declarations, so a generator can write `Info.plist` and `AndroidManifest.xml` from the
//! capabilities a module actually uses, and never more.

use serde::{Deserialize, Serialize};

/// A device capability, not a module capability: it belongs to the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Notifications, local or pushed.
    Notifications,
    Camera,
    Microphone,
    Location,
    Clipboard,
    /// The system share sheet.
    Share,
    /// A live activity: content that lives outside the app (Dynamic Island, lock screen).
    /// Needs a native module (ActivityKit), not a webview.
    LiveActivity,
    /// The contacts book, read by the app (`CNContactStore`).
    Contacts,
    /// Photos the person picks in the system picker: no permission, no declaration. The
    /// app receives only what was chosen.
    PhotosPick,
    /// The whole photo library, read by the app (PhotoKit): a permission and a text.
    Photos,
    /// Files the person picks in the system document picker: no permission, no
    /// declaration. Access outlives a restart only through a security-scoped bookmark.
    FilesPick,
    /// Bluetooth (CoreBluetooth): scanning and connecting to peripherals.
    Bluetooth,
    /// The gyroscope and the accelerometer, read raw: no permission and no declaration.
    Gyroscope,
    /// Activity and steps (CoreMotion activity, pedometer): a permission and a text.
    MotionActivity,
    /// Calendar events, read and written (full access, iOS 17 and later).
    Calendar,
    /// Health data, read (HealthKit).
    Health,
    /// Face ID, to confirm an action the person has authorised.
    FaceId,
    /// Speech recognition.
    Speech,
    /// NFC tags, read.
    Nfc,
    /// Devices on the local network (Bonjour, local discovery).
    LocalNetwork,
    /// Location in the background, as well as in use.
    LocationAlways,
}

/// What a capability needs written into the app's manifest, before it can be requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionKeys {
    /// `Info.plist` keys that take a text, shown to the person in the permission dialog.
    pub ios_usage: &'static [&'static str],
    /// `Info.plist` keys that take `true` (a feature switch, not a dialog).
    pub ios_flags: &'static [&'static str],
    /// `AndroidManifest.xml` permissions.
    pub android: &'static [&'static str],
    /// The first Android API level that needs the permission at runtime, if it does.
    pub android_runtime_from: Option<u32>,
}

const NONE: PermissionKeys = PermissionKeys {
    ios_usage: &[],
    ios_flags: &[],
    android: &[],
    android_runtime_from: None,
};

impl Capability {
    /// Every capability the app can ask for. The generated `Info.plist` covers this whole
    /// list: an iOS key cannot be added after the build.
    pub const ALL: [Capability; 21] = [
        Capability::Notifications,
        Capability::Camera,
        Capability::Microphone,
        Capability::Location,
        Capability::LocationAlways,
        Capability::Clipboard,
        Capability::Share,
        Capability::LiveActivity,
        Capability::Contacts,
        Capability::PhotosPick,
        Capability::Photos,
        Capability::FilesPick,
        Capability::Bluetooth,
        Capability::Gyroscope,
        Capability::MotionActivity,
        Capability::Calendar,
        Capability::Health,
        Capability::FaceId,
        Capability::Speech,
        Capability::Nfc,
        Capability::LocalNetwork,
    ];

    pub fn keys(self) -> PermissionKeys {
        match self {
            // iOS asks at runtime without an Info.plist key; Android 13+ needs the permission.
            Capability::Notifications => PermissionKeys {
                android: &["android.permission.POST_NOTIFICATIONS"],
                android_runtime_from: Some(33),
                ..NONE
            },
            Capability::Camera => PermissionKeys {
                ios_usage: &["NSCameraUsageDescription"],
                android: &["android.permission.CAMERA"],
                android_runtime_from: Some(23),
                ..NONE
            },
            Capability::Microphone => PermissionKeys {
                ios_usage: &["NSMicrophoneUsageDescription"],
                android: &["android.permission.RECORD_AUDIO"],
                android_runtime_from: Some(23),
                ..NONE
            },
            Capability::Location => PermissionKeys {
                ios_usage: &["NSLocationWhenInUseUsageDescription"],
                android: &["android.permission.ACCESS_FINE_LOCATION"],
                android_runtime_from: Some(23),
                ..NONE
            },
            Capability::LiveActivity => PermissionKeys {
                ios_flags: &["NSSupportsLiveActivities"],
                ..NONE
            },
            // Android side not declared yet: this step targets iOS first.
            Capability::Contacts => PermissionKeys {
                ios_usage: &["NSContactsUsageDescription"],
                android: &["android.permission.READ_CONTACTS"],
                android_runtime_from: Some(23),
                ..NONE
            },
            Capability::Photos => PermissionKeys {
                ios_usage: &["NSPhotoLibraryUsageDescription"],
                ..NONE
            },
            Capability::Bluetooth => PermissionKeys {
                ios_usage: &["NSBluetoothAlwaysUsageDescription"],
                ..NONE
            },
            Capability::MotionActivity => PermissionKeys {
                ios_usage: &["NSMotionUsageDescription"],
                ..NONE
            },
            Capability::Calendar => PermissionKeys {
                ios_usage: &["NSCalendarsFullAccessUsageDescription"],
                ..NONE
            },
            Capability::Health => PermissionKeys {
                ios_usage: &["NSHealthShareUsageDescription"],
                ..NONE
            },
            Capability::FaceId => PermissionKeys {
                ios_usage: &["NSFaceIDUsageDescription"],
                ..NONE
            },
            Capability::Speech => PermissionKeys {
                ios_usage: &["NSSpeechRecognitionUsageDescription"],
                ..NONE
            },
            Capability::Nfc => PermissionKeys {
                ios_usage: &["NFCReaderUsageDescription"],
                ..NONE
            },
            Capability::LocalNetwork => PermissionKeys {
                ios_usage: &["NSLocalNetworkUsageDescription"],
                ..NONE
            },
            Capability::LocationAlways => PermissionKeys {
                ios_usage: &["NSLocationAlwaysAndWhenInUseUsageDescription"],
                ..NONE
            },
            Capability::Clipboard
            | Capability::Share
            | Capability::PhotosPick
            | Capability::FilesPick
            | Capability::Gyroscope => NONE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Capability::Notifications => "Notifications",
            Capability::Camera => "Caméra",
            Capability::Microphone => "Microphone",
            Capability::Location => "Localisation",
            Capability::Clipboard => "Presse-papiers",
            Capability::Share => "Partage",
            Capability::LiveActivity => "Activité en direct",
            Capability::Contacts => "Contacts",
            Capability::PhotosPick => "Photos (sélection)",
            Capability::Photos => "Photothèque",
            Capability::FilesPick => "Fichiers (sélection)",
            Capability::Bluetooth => "Bluetooth",
            Capability::Gyroscope => "Gyroscope et accéléromètre",
            Capability::MotionActivity => "Activité (pas, mouvement)",
            Capability::Calendar => "Calendrier",
            Capability::Health => "Santé",
            Capability::FaceId => "Face ID",
            Capability::Speech => "Reconnaissance vocale",
            Capability::Nfc => "NFC",
            Capability::LocalNetwork => "Réseau local",
            Capability::LocationAlways => "Localisation en arrière-plan",
        }
    }
}

/// The `Info.plist` text keys required by a set of capabilities, without duplicates, in a
/// stable order. What a generator writes, and what a test checks against.
pub fn required_ios_usage<'a>(
    capabilities: impl IntoIterator<Item = &'a Capability>,
) -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = capabilities
        .into_iter()
        .flat_map(|c| c.keys().ios_usage.iter().copied())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// The `AndroidManifest.xml` permissions required by a set of capabilities.
pub fn required_android_permissions<'a>(
    capabilities: impl IntoIterator<Item = &'a Capability>,
) -> Vec<&'static str> {
    let mut perms: Vec<&'static str> = capabilities
        .into_iter()
        .flat_map(|c| c.keys().android.iter().copied())
        .collect();
    perms.sort_unstable();
    perms.dedup();
    perms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ios_usage_key_is_a_usage_description() {
        for c in Capability::ALL {
            for k in c.keys().ios_usage {
                assert!(
                    (k.starts_with("NS") || k.starts_with("NFC"))
                        && k.ends_with("UsageDescription"),
                    "{c:?}: {k}"
                );
            }
        }
    }

    #[test]
    fn the_gyroscope_is_read_raw_and_needs_no_declaration() {
        assert_eq!(Capability::Gyroscope.keys(), NONE);
    }

    #[test]
    fn every_android_permission_is_a_system_permission() {
        for c in Capability::ALL {
            for p in c.keys().android {
                assert!(p.starts_with("android.permission."), "{c:?}: {p}");
            }
        }
    }

    #[test]
    fn a_camera_request_needs_its_text_before_it_is_asked() {
        assert_eq!(
            required_ios_usage(&[Capability::Camera]),
            vec!["NSCameraUsageDescription"]
        );
    }

    #[test]
    fn live_activities_need_a_switch_not_a_dialog() {
        let k = Capability::LiveActivity.keys();
        assert!(k.ios_usage.is_empty());
        assert_eq!(k.ios_flags, &["NSSupportsLiveActivities"]);
    }

    #[test]
    fn notifications_need_no_plist_key_but_an_android_runtime_grant() {
        let k = Capability::Notifications.keys();
        assert!(
            k.ios_usage.is_empty(),
            "iOS asks at runtime, no Info.plist key"
        );
        assert_eq!(k.android_runtime_from, Some(33));
    }

    #[test]
    fn requirements_are_deduplicated_and_stable() {
        let caps = [Capability::Location, Capability::Camera, Capability::Camera];
        assert_eq!(
            required_ios_usage(&caps),
            vec![
                "NSCameraUsageDescription",
                "NSLocationWhenInUseUsageDescription"
            ]
        );
    }

    #[test]
    fn nothing_is_required_for_clipboard_or_share() {
        for c in [Capability::Clipboard, Capability::Share] {
            assert_eq!(c.keys(), NONE);
        }
    }

    #[test]
    fn system_pickers_need_no_declaration_at_all() {
        // Le sélecteur système donne ce que la personne choisit : aucune permission.
        for c in [Capability::PhotosPick, Capability::FilesPick] {
            assert_eq!(c.keys(), NONE, "{c:?}");
        }
    }

    #[test]
    fn contacts_and_photo_library_each_need_their_text() {
        assert_eq!(
            required_ios_usage(&[Capability::Contacts, Capability::Photos]),
            vec![
                "NSContactsUsageDescription",
                "NSPhotoLibraryUsageDescription"
            ]
        );
        assert_eq!(
            Capability::Contacts.keys().android,
            &["android.permission.READ_CONTACTS"]
        );
    }

    #[test]
    fn the_new_wire_names_are_stable() {
        assert_eq!(
            serde_json::to_value(Capability::Contacts).unwrap(),
            "contacts"
        );
        assert_eq!(
            serde_json::to_value(Capability::PhotosPick).unwrap(),
            "photos_pick"
        );
        assert_eq!(
            serde_json::to_value(Capability::FilesPick).unwrap(),
            "files_pick"
        );
    }

    #[test]
    fn the_wire_names_are_stable() {
        assert_eq!(
            serde_json::to_value(Capability::LiveActivity).unwrap(),
            "live_activity"
        );
    }
}
