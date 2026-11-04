//! # `this-shell-tauri` — the Tauri 2 shell of a this-rs app
//!
//! One line connects an app to the platform decisions of [`this_platform`]:
//!
//! ```ignore
//! tauri::Builder::default()
//!     .plugin(this_shell_tauri::init(
//!         this_shell_tauri::ShellConfig::default()
//!             .close(this_platform::ClosePreference::Platform),
//!     ))
//! ```
//!
//! What the plugin does, and only this:
//!
//! - **Closing the window** asks [`this_platform::close_action`]: on macOS the window is hidden
//!   and the app stays in the Dock; elsewhere, or on an explicit quit, the app closes.
//! - **Clicking the Dock icon** with no visible window shows the main window again
//!   ([`RunEvent::Reopen`], macOS only).
//! - **Window changes** (resize, focus, scale) are reported as [`ShellEvent::Env`] on the
//!   `shell://event` channel, with the measured chrome.
//! - **iOS**: the webview stops adding its own safe-area inset, so the app reserves it once.
//!
//! The plugin exposes two commands, `plugin:this-shell|env` and `plugin:this-shell|quit`.
//! Their permissions come with the plugin (`this-shell:default`).
//!
//! The rules live in `this-platform` and are tested there; this crate only connects them to
//! the windowing system, so it has no logic of its own to test without a window.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime, WebviewWindow, WindowEvent};
use this_platform::chrome::{ChromeEnv, Size, TitleBar, WindowState};
use this_platform::{
    CloseAction, CloseIntent, ClosePreference, Platform, ShellEvent, close_action,
};

pub use this_platform as platform;

/// The channel on which the plugin reports [`ShellEvent`]s to the webview.
pub const EVENT: &str = "shell://event";

/// What the plugin needs to know about the app.
#[derive(Clone, Debug)]
pub struct ShellConfig {
    /// The label of the main window. Default: `main`.
    pub window: String,
    /// The person's preference for closing a window. Default: the platform's habit.
    pub close: ClosePreference,
    /// Reload the data when the app comes back after this long. Read by the resume
    /// decision (`this_platform::resume_plan`), which the app calls; default 30 s.
    pub resync_after: Duration,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            window: "main".into(),
            close: ClosePreference::Platform,
            resync_after: Duration::from_secs(30),
        }
    }
}

impl ShellConfig {
    pub fn window(mut self, label: impl Into<String>) -> Self {
        self.window = label.into();
        self
    }

    pub fn close(mut self, preference: ClosePreference) -> Self {
        self.close = preference;
        self
    }

    pub fn resync_after(mut self, after: Duration) -> Self {
        self.resync_after = after;
        self
    }
}

/// State the plugin keeps for the lifetime of the app.
struct State {
    config: ShellConfig,
    /// Set when the application itself is exiting (Cmd+Q, menu quit, system logout).
    /// A window closing during an exit is a quit, not a close button.
    quitting: AtomicBool,
}

/// The platform the binary was built for. Decided at compile time: a binary is one platform.
pub fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "ios") {
        Platform::Ios
    } else if cfg!(target_os = "android") {
        Platform::Android
    } else {
        Platform::Linux
    }
}

/// Build the plugin. Add it once, before the app runs.
pub fn init<R: Runtime>(config: ShellConfig) -> TauriPlugin<R> {
    let main_window = config.window.clone();
    Builder::new("this-shell")
        .invoke_handler(tauri::generate_handler![env, quit])
        .setup({
            let config = config.clone();
            move |app, _api| {
                app.manage(State {
                    config,
                    quitting: AtomicBool::new(false),
                });
                Ok(())
            }
        })
        // The windows of `tauri.conf.json` are created after `setup`: the main window is
        // attached when it exists, not when the plugin starts.
        // The webview is what exists when the window's content is ready; the window alone can
        // be queried before its webview is registered, and the handler would silently not attach.
        .on_webview_ready(move |webview| {
            if webview.label() != main_window {
                return;
            }
            let app = webview.app_handle();
            if let Some(window) = app.get_webview_window(webview.label()) {
                attach(&window);
                #[cfg(target_os = "ios")]
                cover_safe_area(&window);
            }
        })
        .on_event(|app, event| match event {
            RunEvent::ExitRequested { .. } => {
                if let Some(state) = app.try_state::<State>() {
                    state.quitting.store(true, Ordering::SeqCst);
                }
            }
            // Clicking the Dock icon while every window is hidden: bring the main window back.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen {
                has_visible_windows,
                ..
            } => {
                let main = app
                    .try_state::<State>()
                    .and_then(|state| app.get_webview_window(&state.config.window));
                if let (false, Some(window)) = (has_visible_windows, main) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        })
        .build()
}

/// Route the window's own events: the close button, and the chrome changes.
fn attach<R: Runtime>(window: &WebviewWindow<R>) {
    let app = window.app_handle().clone();
    let target = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            let quitting = app
                .try_state::<State>()
                .map(|s| s.quitting.load(Ordering::SeqCst))
                .unwrap_or(false);
            let intent = if quitting {
                CloseIntent::Quit
            } else {
                CloseIntent::Window
            };
            let preference = app
                .try_state::<State>()
                .map(|s| s.config.close)
                .unwrap_or_default();
            if close_action(current_platform(), preference, intent) == CloseAction::Hide {
                api.prevent_close();
                let _ = target.hide();
            }
            let _ = app.emit(EVENT, ShellEvent::CloseRequested { intent });
        }
        WindowEvent::Resized(_)
        | WindowEvent::Focused(_)
        | WindowEvent::ScaleFactorChanged { .. } => {
            let _ = app.emit(
                EVENT,
                ShellEvent::Env {
                    env: env_of(&target),
                },
            );
        }
        // Mobile only: the system suspended the app, or brought it back. Reported as-is; the
        // app decides what a return means (`this_platform::resume_plan`).
        #[cfg(any(target_os = "ios", target_os = "android"))]
        WindowEvent::Suspended => {
            let _ = app.emit(EVENT, ShellEvent::Background);
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        WindowEvent::Resumed => {
            let _ = app.emit(EVENT, ShellEvent::Foreground);
        }
        _ => {}
    });
}

/// The chrome as the window knows it now. The webview adds the safe areas and the viewport.
pub fn env_of<R: Runtime>(window: &WebviewWindow<R>) -> ChromeEnv {
    let (title_bar, controls) = declared_title_bar(window.app_handle(), window.label());
    ChromeEnv {
        platform: current_platform(),
        title_bar,
        window: WindowState {
            fullscreen: window.is_fullscreen().unwrap_or(false),
            maximized: window.is_maximized().unwrap_or(false),
            focused: window.is_focused().unwrap_or(true),
        },
        device_model: device_model(),
        controls,
        ..Default::default()
    }
}

/// The title bar style and the place of the lights, read from the window's configuration:
/// the configuration is the single source, the app does not repeat it in Rust.
fn declared_title_bar<R: Runtime>(app: &AppHandle<R>, label: &str) -> (TitleBar, Option<Size>) {
    let Some(cfg) = app.config().app.windows.iter().find(|w| w.label == label) else {
        return (TitleBar::Native, None);
    };
    if !cfg.decorations {
        return (TitleBar::Hidden, None);
    }
    if !cfg!(target_os = "macos") {
        // `titleBarStyle` only exists on macOS; elsewhere the bar stays native.
        return (TitleBar::Native, None);
    }
    match cfg.title_bar_style {
        tauri::TitleBarStyle::Visible => (TitleBar::Native, None),
        _ => {
            // Measured on macOS (Tauri 2.12): `x` is the left margin of the first light,
            // `y` is not its top. The lights are centred on `2y - 5`, three buttons of
            // 52 px with the margin on each side.
            let controls = cfg.traffic_light_position.as_ref().map(|p| Size {
                width: (p.x * 2.0 + 52.0) as f32,
                height: (p.y * 2.0 - 5.0) as f32,
            });
            (TitleBar::Overlay, controls)
        }
    }
}

/// The machine identifier (`iPhone17,2`). In the simulator, the model is in an environment
/// variable; `uname` gives the architecture of the host.
#[cfg(target_os = "ios")]
fn device_model() -> Option<String> {
    if let Ok(model) = std::env::var("SIMULATOR_MODEL_IDENTIFIER") {
        return Some(model);
    }
    // SAFETY: `uname` fills a structure we own; `machine` is NUL-terminated.
    unsafe {
        let mut info: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut info) != 0 {
            return None;
        }
        let machine = std::ffi::CStr::from_ptr(info.machine.as_ptr());
        machine.to_str().ok().map(str::to_owned)
    }
}

#[cfg(not(target_os = "ios"))]
fn device_model() -> Option<String> {
    None
}

/// On iOS the page covers the whole screen and handles the safe areas itself.
///
/// Measured (simulators iPhone 16 Pro and 16e, wry 0.57): by default the web view's scroll
/// view removes the safe area itself (`contentInsetAdjustmentBehavior = automatic`) and the
/// page also receives it through `env(safe-area-inset-*)`. The viewport was 778 px on a
/// 874 px screen: 62 + 34 removed twice. `never` leaves a single authority: this shell's
/// layout.
#[cfg(target_os = "ios")]
fn cover_safe_area<R: Runtime>(window: &WebviewWindow<R>) {
    use objc2::runtime::AnyObject;
    let _ = window.with_webview(|webview| {
        let wk = webview.inner() as *mut AnyObject;
        if wk.is_null() {
            return;
        }
        // SAFETY: `inner()` is the live WKWebView, on the main thread (with_webview).
        // UIScrollViewContentInsetAdjustmentBehavior.never = 2.
        unsafe {
            let scroll: *mut AnyObject = objc2::msg_send![&*wk, scrollView];
            if !scroll.is_null() {
                let _: () = objc2::msg_send![&*scroll, setContentInsetAdjustmentBehavior: 2isize];
            }
        }
    });
}

/// The chrome of the calling window, for the front to lay out its header and margins.
#[tauri::command]
fn env<R: Runtime>(window: WebviewWindow<R>) -> ChromeEnv {
    env_of(&window)
}

/// Quit the app now. The front calls it from an explicit quit control.
#[tauri::command]
fn quit<R: Runtime>(app: AppHandle<R>) {
    if let Some(state) = app.try_state::<State>() {
        state.quitting.store(true, Ordering::SeqCst);
    }
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_keeps_the_main_window_and_the_platform_habit() {
        let c = ShellConfig::default();
        assert_eq!(c.window, "main");
        assert_eq!(c.close, ClosePreference::Platform);
        assert_eq!(c.resync_after, Duration::from_secs(30));
    }

    #[test]
    fn the_builder_overrides_one_field_at_a_time() {
        let c = ShellConfig::default()
            .window("panel")
            .close(ClosePreference::Quit)
            .resync_after(Duration::from_secs(5));
        assert_eq!(c.window, "panel");
        assert_eq!(c.close, ClosePreference::Quit);
        assert_eq!(c.resync_after, Duration::from_secs(5));
    }

    #[test]
    fn the_binary_knows_its_platform_without_asking_the_os() {
        let p = current_platform();
        assert_eq!(
            p.is_desktop(),
            cfg!(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux"
            ))
        );
        // and the layout for a bare desktop env reserves what the platform needs
        let _ = this_platform::chrome::layout(&ChromeEnv {
            platform: p,
            ..Default::default()
        });
    }
}
