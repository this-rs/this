//! # `this-platform` — the shell contract of a this-rs app
//!
//! Every platform shell (Tauri desktop, Tauri mobile, the browser) answers the same
//! questions and receives the same events. This crate holds **the decisions**, not the
//! code that asks the OS: what to reserve for the traffic lights, whether closing a window
//! quits the app, whether a resumed app must resynchronise, which permission strings a
//! capability needs. A shell measures; this crate decides.
//!
//! - [`form`] — [`Platform`] (the system) and [`FormFactor`] (the screen). Not
//!   `Surface`: that name is the server exposure (REST, GraphQL, gRPC, WebSocket).
//! - [`chrome`] — window and device chrome: title bar, traffic lights or caption buttons,
//!   cutouts (notch, Dynamic Island), safe areas, and the CSS variables that express them.
//! - [`lifecycle`] — closing, reopening, resuming: [`close_action`] and [`resume_plan`].
//! - [`shell`] — the events a shell reports and the commands it receives.
//! - [`capability`] — device capabilities and the permission strings each one requires.
//! - [`device_bridge`] — the requests a module may make to the device, and the consent
//!   gate that refuses them unless the person allowed it (fail-closed).
//!
//! Adapters live elsewhere: `this-shell-tauri` (Tauri 2 plugin), `this-shell-web`.

pub mod capability;
pub mod chrome;
pub mod device_bridge;
pub mod form;
pub mod lifecycle;
pub mod manifest;
pub mod shell;

pub use capability::{Capability, PermissionKeys};
pub use chrome::{ChromeEnv, ChromeLayout, Cutout, CutoutSource, Insets, Size, TitleBar};
pub use form::{FormFactor, Platform};
pub use lifecycle::{
    CloseAction, CloseIntent, ClosePreference, ResumePlan, close_action, resume_plan,
};
pub use manifest::{android_permissions, info_plist_entries, missing_usage_texts};
pub use shell::{ShellCommand, ShellEvent};
