//! The device families: one module per family, each owning the wire shape of its own
//! requests and answers, and the checks applied before the device is touched.
//!
//! Every module here is **pure**: no Tauri, no native code, no clock. That is what makes the
//! contract testable without a phone, and what keeps the decision (`families/`, `device_bridge`)
//! apart from the effect (`ios/Sources/*.swift`).
//!
//! A family is reachable only through [`crate::answer`] with a `Request::Call`, whose
//! capability is derived from the method by `this_platform::device_bridge::capability_of`.
//! Adding a family means adding its module here **and** its methods to that table: a module
//! that is not in the table answers `not_supported`.

pub mod bluetooth;
pub mod calendar;
pub mod camera_mic;
pub mod clipboard_share;
pub mod contacts;
pub mod face_speech;
pub mod files;
pub mod health;
pub mod local_network;
pub mod location;
pub mod motion_activity;
pub mod nfc;
pub mod notif_live;
pub mod photos;
