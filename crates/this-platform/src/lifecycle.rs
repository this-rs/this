//! Closing, reopening and resuming: the decisions a shell asks for.
//!
//! The rules are pure, so they are the same on every shell and testable without a window.
//! The clock is an argument, never read here.

use serde::{Deserialize, Serialize};

use crate::form::Platform;
pub use crate::shell::CloseIntent;

/// The person's preference for closing a window. `Platform` follows the system's habit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosePreference {
    #[default]
    Platform,
    Hide,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    /// Hide the window; the app keeps running and can be reopened.
    Hide,
    /// Quit the app.
    Quit,
}

/// Can a hidden window be brought back by the person? Only where the system offers a way:
/// macOS keeps the app in the Dock, and a click reopens it ([`crate::ShellEvent::Reopen`]).
/// Elsewhere there is no tray icon in this contract yet, so hiding would make the app
/// unreachable: closing quits instead.
pub fn can_reopen_hidden(platform: Platform) -> bool {
    matches!(platform, Platform::Macos)
}

/// What to do with a close request.
///
/// - an explicit quit always quits (Cmd+Q is not a close button);
/// - a window close hides only when the platform can reopen the window, and the person has
///   not asked for quitting;
/// - otherwise, closing quits.
pub fn close_action(
    platform: Platform,
    preference: ClosePreference,
    intent: CloseIntent,
) -> CloseAction {
    if intent == CloseIntent::Quit {
        return CloseAction::Quit;
    }
    let wants_hide = match preference {
        ClosePreference::Quit => false,
        ClosePreference::Hide => true,
        ClosePreference::Platform => matches!(platform, Platform::Macos),
    };
    if wants_hide && can_reopen_hidden(platform) {
        CloseAction::Hide
    } else {
        CloseAction::Quit
    }
}

/// What to do when the app comes back to the foreground.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePlan {
    /// Reload the data: it may have changed while the app was away.
    pub resync: bool,
    /// Open the live stream again: the system suspended the connection in the background.
    pub reconnect: bool,
}

/// What to do on resume.
///
/// - Offline: nothing can be done yet. The caller plans again on the next
///   [`crate::ShellEvent::Online`] event (`online: true`).
/// - Online: the live stream is always reopened (the system suspended it). The data is
///   reloaded when the last synchronisation is older than `stale_after_ms`, or unknown.
///
/// `last_sync_ms` and `now_ms` are the same clock, in milliseconds since any fixed epoch.
pub fn resume_plan(
    last_sync_ms: Option<u64>,
    now_ms: u64,
    stale_after_ms: u64,
    online: bool,
) -> ResumePlan {
    if !online {
        return ResumePlan {
            resync: false,
            reconnect: false,
        };
    }
    let stale = match last_sync_ms {
        None => true,
        Some(at) => now_ms.saturating_sub(at) >= stale_after_ms,
    };
    ResumePlan {
        resync: stale,
        reconnect: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quit_request_always_quits_even_where_hiding_exists() {
        assert_eq!(
            close_action(Platform::Macos, ClosePreference::Hide, CloseIntent::Quit),
            CloseAction::Quit,
            "Cmd+Q is not the red button"
        );
    }

    #[test]
    fn on_macos_closing_the_window_keeps_the_app_in_the_dock() {
        assert_eq!(
            close_action(
                Platform::Macos,
                ClosePreference::Platform,
                CloseIntent::Window
            ),
            CloseAction::Hide
        );
    }

    #[test]
    fn elsewhere_closing_the_window_quits_because_nothing_can_bring_it_back() {
        for p in [
            Platform::Windows,
            Platform::Linux,
            Platform::Ios,
            Platform::Android,
            Platform::Web,
        ] {
            assert!(!can_reopen_hidden(p), "{p:?}");
            assert_eq!(
                close_action(p, ClosePreference::Platform, CloseIntent::Window),
                CloseAction::Quit,
                "{p:?}"
            );
        }
    }

    #[test]
    fn a_hide_preference_degrades_to_quit_where_it_would_strand_the_app() {
        assert_eq!(
            close_action(
                Platform::Windows,
                ClosePreference::Hide,
                CloseIntent::Window
            ),
            CloseAction::Quit
        );
    }

    #[test]
    fn a_quit_preference_quits_on_macos_too() {
        assert_eq!(
            close_action(Platform::Macos, ClosePreference::Quit, CloseIntent::Window),
            CloseAction::Quit
        );
    }

    #[test]
    fn resume_while_offline_does_nothing_until_the_network_is_back() {
        let p = resume_plan(None, 1_000, 30_000, false);
        assert_eq!(
            p,
            ResumePlan {
                resync: false,
                reconnect: false
            }
        );
    }

    #[test]
    fn resume_always_reopens_the_stream_when_online() {
        let fresh = resume_plan(Some(990_000), 1_000_000, 30_000, true);
        assert!(fresh.reconnect, "the system suspended the connection");
        assert!(!fresh.resync, "synchronised 10 s ago: no reload needed");
    }

    #[test]
    fn resume_reloads_when_the_last_sync_is_stale_or_unknown() {
        assert!(resume_plan(Some(0), 60_000, 30_000, true).resync);
        assert!(resume_plan(None, 60_000, 30_000, true).resync);
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_underflow() {
        // last sync "in the future" (clock adjusted): treated as fresh, never a panic
        let p = resume_plan(Some(5_000), 1_000, 30_000, true);
        assert!(!p.resync);
    }
}
