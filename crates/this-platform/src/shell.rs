//! The conversation between a shell and the app.
//!
//! A shell **reports** ([`ShellEvent`]: the window was asked to close, the app came back to
//! the foreground, the network changed) and **receives** ([`ShellCommand`]: hide the window,
//! quit, show a notice, ask for a permission). Both are serialisable, so the same messages
//! travel over Tauri events, `postMessage`, or a test harness.
//!
//! The events are struct variants on purpose: serde's internally tagged enums do not accept
//! bare primitives (`Focus(bool)`), so every payload is named.

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::chrome::{ChromeEnv, Size};

/// Why the window is closing. A close button and the quit shortcut are different intents:
/// on macOS the first hides the window, the second quits the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseIntent {
    /// The window's close control (red button, Alt+F4).
    #[default]
    Window,
    /// An explicit quit (Cmd+Q, the application menu, the system asking to log out).
    Quit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    Info,
    Ok,
    Warn,
    Danger,
}

/// A message to show the person. Two notices with the same `key` replace each other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub key: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tone: Tone,
}

impl Notice {
    pub fn new(key: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            body: body.into(),
            tone: Tone::Info,
        }
    }

    pub fn with_tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

/// What a shell reports to the app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellEvent {
    /// The chrome as the shell now knows it (window state, device model, measured controls).
    Env {
        env: ChromeEnv,
    },
    /// The window is asked to close. The app answers with [`crate::close_action`].
    CloseRequested {
        intent: CloseIntent,
    },
    /// The person clicked the app's icon while no window was visible (macOS Dock).
    Reopen,
    /// The app left the foreground. Its network connections may be suspended.
    Background,
    /// The app came back to the foreground. Data may be stale: plan a resume.
    Foreground,
    /// The webview finished resuming (after `Foreground`, once it can run code again).
    Resumed,
    Focus {
        focused: bool,
    },
    Resized {
        size: Size,
    },
    Online {
        online: bool,
    },
    /// The system switched between light and dark appearance.
    ThemeChanged {
        dark: bool,
    },
}

/// What the app asks the shell to do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellCommand {
    Show,
    Hide,
    Quit,
    /// Show a notice: a system notification when permitted, the in-app toast otherwise.
    Notify {
        notice: Notice,
    },
    /// Ask for a device permission. The answer comes back as a later event.
    Request {
        capability: Capability,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged_by_name_on_the_wire() {
        let e = ShellEvent::CloseRequested {
            intent: CloseIntent::Quit,
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({ "type": "close_requested", "intent": "quit" })
        );
        let back: ShellEvent = serde_json::from_value(serde_json::json!({
            "type": "focus", "focused": false
        }))
        .unwrap();
        assert_eq!(back, ShellEvent::Focus { focused: false });
    }

    #[test]
    fn a_notice_defaults_to_info_and_keeps_its_tone() {
        let n = Notice::new("save", "Enregistré", "3 lignes");
        assert_eq!(n.tone, Tone::Info);
        assert_eq!(n.clone().with_tone(Tone::Warn).tone, Tone::Warn);
    }

    #[test]
    fn commands_carry_capabilities_by_name() {
        let c = ShellCommand::Request {
            capability: Capability::Camera,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({ "type": "request", "capability": "camera" })
        );
    }
}
