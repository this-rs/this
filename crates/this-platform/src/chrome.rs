//! # Window and device chrome: what the system draws over the app
//!
//! Each surface puts something on the window, and none of it is fixed:
//!
//! | System | What it draws | What makes it vary |
//! |---|---|---|
//! | macOS | the three traffic lights, top left | native or integrated bar; **they disappear in full screen** |
//! | Windows | minimise / maximise / close, right | native bar, or drawn by the app |
//! | Linux | depends on the window manager | same; often right |
//! | iPhone | status bar, notch **or** Dynamic Island **or** nothing, home indicator | device model, orientation |
//! | Android | status bar, punch hole, navigation bar | device, orientation |
//! | browser | nothing, unless installed as an app (safe areas) | display mode, full screen |
//!
//! The app should not write any of these cases. It receives a [`ChromeLayout`]: how much to
//! reserve on each side, where the window controls are and who draws them, whether the header
//! drags the window, where the cutout is. [`ChromeLayout::css_vars`] and
//! [`ChromeLayout::attributes`] express it as CSS variables and attributes.
//!
//! Everything here is pure. The shell **measures** (safe-area insets reported by the webview,
//! full-screen state, device model); this module **decides**. The dimensions of the controls
//! and of the island are platform constants, documented, not measurements of this device; a
//! shell that knows better replaces them through [`ChromeEnv::controls`].

use serde::{Deserialize, Serialize};

use crate::form::Platform;

/// Where the title bar of a desktop window is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleBar {
    /// The system bar sits above the content: the app reserves nothing.
    #[default]
    Native,
    /// A transparent bar over the content: the system draws its controls, the app leaves them
    /// room and serves as the drag handle.
    Overlay,
    /// No bar at all: the app draws its own controls.
    Hidden,
}

impl TitleBar {
    pub fn id(self) -> &'static str {
        match self {
            TitleBar::Native => "native",
            TitleBar::Overlay => "overlay",
            TitleBar::Hidden => "hidden",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

/// The window state that changes where the controls sit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    /// In full screen macOS **removes** the traffic lights (they come back only on hover).
    pub fullscreen: bool,
    pub maximized: bool,
    /// Without focus, macOS greys the lights; the header can do the same.
    pub focused: bool,
}

/// Four margins, in CSS pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Insets {
    pub const ZERO: Insets = Insets {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    };

    pub fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }
}

/// A rectangle in CSS pixels, in the viewport's frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    #[default]
    Portrait,
    Landscape,
}

impl Size {
    pub fn orientation(self) -> Orientation {
        if self.width > self.height {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        }
    }
}

/// What the screen has cut into its top edge (or side, in landscape).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cutout {
    /// Full screen (iPhone SE, iPad, desktop).
    #[default]
    None,
    /// The Face ID notch: a fixed cutout, nothing is shown in it.
    Notch { width: f32, height: f32 },
    /// The island: a cutout that **grows** when the system shows an activity in it. Do not
    /// place anything under it that must stay readable; it can expand without warning.
    DynamicIsland { width: f32, height: f32, top: f32 },
    /// A camera punch hole (Android): small, not necessarily centred.
    PunchHole { width: f32, height: f32 },
}

impl Cutout {
    pub fn id(self) -> &'static str {
        match self {
            Cutout::None => "none",
            Cutout::Notch { .. } => "notch",
            Cutout::DynamicIsland { .. } => "island",
            Cutout::PunchHole { .. } => "punch-hole",
        }
    }
}

/// Where the knowledge of the cutout comes from. A cutout **inferred** from a margin can be
/// wrong; an interface that depends heavily on it (animating around the island) should check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutoutSource {
    /// The exact device identifier is in the table.
    Model,
    /// Unknown identifier, placed by generation (a model newer than the table).
    Family,
    /// No identifier (browser): inferred from the top safe-area margin.
    Inset,
    /// Nothing says which.
    #[default]
    Unknown,
}

// ---- Platform dimensions (documented, not measured here) --------------------------------

/// The macOS traffic lights in an `Overlay` bar: about 70 px of buttons, plus margin.
/// The band the three window buttons occupy, measured — not recalled.
///
/// These numbers follow a norm that changes with the macOS version, exactly
/// like the window's corner radius. They said 78x28 (an older norm) until they
/// were measured on macOS 26.5.2 with AppKit's own frames:
///
/// ```text
/// close x=9 y=9 14x14 · minimize x=32 · zoom x=55
/// span of the three: 60 · inset 9 · title bar: 32 high
/// ```
///
/// so 9 + 60 = 69 wide, 32 tall. The measuring script lives in the examples
/// repo (`examples/desktop-modules/tools/measure-traffic-lights.swift`): run it
/// on the target machine before changing these numbers.
pub const MACOS_TRAFFIC_LIGHTS: Size = Size {
    width: 69.0,
    height: 32.0,
};
/// Minimise / maximise / close on Windows: three 46 px buttons.
pub const WINDOWS_CAPTION_BUTTONS: Size = Size {
    width: 138.0,
    height: 32.0,
};
/// Controls an app draws itself (`TitleBar::Hidden`): the room to keep for them.
pub const APP_DRAWN_CONTROLS: Size = Size {
    width: 120.0,
    height: 36.0,
};

/// The island at rest, in points (iPhone 14 Pro to 16 Pro Max): about 126 × 37, 11 from the top.
const ISLAND: Cutout = Cutout::DynamicIsland {
    width: 126.0,
    height: 37.0,
    top: 11.0,
};
/// Notch of the iPhone X to 12 (wide) and 13 to 16e (narrower).
const NOTCH_WIDE: Cutout = Cutout::Notch {
    width: 209.0,
    height: 30.0,
};
const NOTCH_NARROW: Cutout = Cutout::Notch {
    width: 162.0,
    height: 33.0,
};

/// The cutout of an iPhone from its machine identifier (`uname -m`, or
/// `SIMULATOR_MODEL_IDENTIFIER` in the simulator), for example `iPhone17,2`.
///
/// Exceptions first (an SE in a notch generation, a 16e in an island generation), then the
/// rule by generation. An identifier newer than the table gives [`CutoutSource::Family`]: we
/// assume the island, and we say we assume it.
pub fn ios_cutout(machine: &str) -> (Cutout, CutoutSource) {
    if machine.starts_with("iPad") || machine.starts_with("iPod") {
        // No iPad, even with Face ID, has a notch: the camera sits in the frame.
        return (Cutout::None, CutoutSource::Model);
    }
    let Some(rest) = machine.strip_prefix("iPhone") else {
        return (Cutout::None, CutoutSource::Unknown);
    };
    let mut parts = rest.split(',');
    let (Some(Ok(major)), Some(Ok(minor))) = (
        parts.next().map(str::parse::<u32>),
        parts.next().map(str::parse::<u32>),
    ) else {
        return (Cutout::None, CutoutSource::Unknown);
    };

    let exact = match (major, minor) {
        // iPhone 8 / 8 Plus in generation 10; X right next to them.
        (10, 1) | (10, 2) | (10, 4) | (10, 5) => Some(Cutout::None),
        (10, 3) | (10, 6) => Some(NOTCH_WIDE),
        (12, 8) | (14, 6) => Some(Cutout::None), // SE 2nd and 3rd generation
        (14, 7) | (14, 8) => Some(NOTCH_NARROW), // iPhone 14 / 14 Plus: notch, no island
        (15, 2) | (15, 3) => Some(ISLAND),       // 14 Pro / 14 Pro Max
        (17, 5) => Some(NOTCH_NARROW),           // iPhone 16e
        _ => None,
    };
    if let Some(cutout) = exact {
        return (cutout, CutoutSource::Model);
    }

    let (cutout, known) = match major {
        0..=9 => (Cutout::None, true),
        11..=13 => (NOTCH_WIDE, true),
        14 => (NOTCH_NARROW, true),
        15..=18 => (ISLAND, true),
        _ => (ISLAND, false),
    };
    (
        cutout,
        if known {
            CutoutSource::Model
        } else {
            CutoutSource::Family
        },
    )
}

/// Without a device identifier (an installed web page), the only trace of the cutout is the
/// margin the system imposes on the content, at the cutout side.
///
/// 20: status bar only; 40 to 53: notch; 54 and up: island (59 on 14 Pro, 62 on 16 Pro).
/// Inferred, hence [`CutoutSource::Inset`]. Only iOS is inferred this way.
pub fn cutout_from_inset(
    platform: Platform,
    safe: Insets,
    viewport: Size,
) -> (Cutout, CutoutSource) {
    if platform != Platform::Ios {
        return (Cutout::None, CutoutSource::Unknown);
    }
    let edge = match viewport.orientation() {
        Orientation::Portrait => safe.top,
        Orientation::Landscape => safe.left.max(safe.right),
    };
    let cutout = if edge >= 54.0 {
        ISLAND
    } else if edge >= 40.0 {
        NOTCH_NARROW
    } else {
        Cutout::None
    };
    (cutout, CutoutSource::Inset)
}

// ---- Input and output -------------------------------------------------------------------

/// Where the window controls are, and who draws them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Controls {
    pub side: Side,
    pub width: f32,
    pub height: f32,
    /// `true`: the system draws them, the app leaves a gap. `false`: the app draws them.
    pub system: bool,
}

/// What the shell knows about the chrome at a given moment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeEnv {
    pub platform: Platform,
    #[serde(default)]
    pub title_bar: TitleBar,
    #[serde(default)]
    pub window: WindowState,
    /// What the webview reports through `env(safe-area-inset-*)`.
    #[serde(default)]
    pub safe: Insets,
    #[serde(default)]
    pub viewport: Size,
    /// Machine identifier (`iPhone17,2`), when the native shell knows it.
    #[serde(default)]
    pub device_model: Option<String>,
    /// The real size of the controls, when the shell measured or moved them.
    #[serde(default)]
    pub controls: Option<Size>,
}

/// What the interface must reserve. Computed by [`layout`], never by hand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeLayout {
    pub platform: Platform,
    pub title_bar: TitleBar,
    pub fullscreen: bool,
    pub focused: bool,
    pub orientation: Orientation,
    /// The margin to leave the content on each side. On top: an integrated title bar, or
    /// the status bar and cutout. At the bottom: the home indicator.
    pub insets: Insets,
    /// Minimal header height so the controls fit, centred.
    pub header_min_height: f32,
    /// Room to leave at the start and end of the header (the window controls).
    pub header_lead: f32,
    pub header_trail: f32,
    pub controls: Option<Controls>,
    /// The header is the window's drag handle (integrated or absent bar).
    pub drag_region: bool,
    pub cutout: Cutout,
    pub cutout_source: CutoutSource,
    /// Where the cutout is in the viewport: place nothing there that must stay readable.
    pub cutout_rect: Option<Rect>,
}

/// Decides what to reserve, from what the shell knows.
pub fn layout(env: &ChromeEnv) -> ChromeLayout {
    let orientation = env.viewport.orientation();

    // ---- Window controls (desktop only) ----
    let controls = if env.platform.is_desktop() && !env.window.fullscreen {
        match env.title_bar {
            TitleBar::Native => None,
            TitleBar::Overlay => {
                let default = match env.platform {
                    Platform::Macos => MACOS_TRAFFIC_LIGHTS,
                    _ => WINDOWS_CAPTION_BUTTONS,
                };
                let size = env.controls.unwrap_or(default);
                Some(Controls {
                    side: match env.platform {
                        Platform::Macos => Side::Left,
                        _ => Side::Right,
                    },
                    width: size.width,
                    height: size.height,
                    system: true,
                })
            }
            TitleBar::Hidden => {
                let size = env.controls.unwrap_or(APP_DRAWN_CONTROLS);
                Some(Controls {
                    side: match env.platform {
                        Platform::Macos => Side::Left,
                        _ => Side::Right,
                    },
                    width: size.width,
                    height: size.height,
                    system: false,
                })
            }
        }
    } else {
        None
    };

    let (header_lead, header_trail) = match controls {
        Some(Controls {
            side: Side::Left,
            width,
            ..
        }) => (width, 0.0),
        Some(Controls {
            side: Side::Right,
            width,
            ..
        }) => (0.0, width),
        None => (0.0, 0.0),
    };
    let header_min_height = controls.map(|c| c.height).unwrap_or(0.0);

    // ---- Cutout (mobile) ----
    let (cutout, cutout_source) = match (env.platform, env.device_model.as_deref()) {
        (Platform::Ios, Some(model)) => ios_cutout(model),
        (Platform::Ios, None) => cutout_from_inset(env.platform, env.safe, env.viewport),
        _ => (Cutout::None, CutoutSource::Unknown),
    };
    let cutout_rect = cutout_rect(cutout, orientation, env.safe, env.viewport);

    // The system imposes its safe areas: they are kept as measured. An integrated desktop
    // bar is not in them (the webview reports 0): it is carried by `header_min_height`, which
    // falls to 0 in full screen with the lights.
    let insets = env.safe;

    let drag_region =
        env.platform.is_desktop() && env.title_bar != TitleBar::Native && !env.window.fullscreen;

    ChromeLayout {
        platform: env.platform,
        title_bar: env.title_bar,
        fullscreen: env.window.fullscreen,
        focused: env.window.focused,
        orientation,
        insets,
        header_min_height,
        header_lead,
        header_trail,
        controls,
        drag_region,
        cutout,
        cutout_source,
        cutout_rect,
    }
}

fn cutout_rect(
    cutout: Cutout,
    orientation: Orientation,
    safe: Insets,
    viewport: Size,
) -> Option<Rect> {
    let (w, h, top) = match cutout {
        Cutout::None => return None,
        Cutout::Notch { width, height } | Cutout::PunchHole { width, height } => {
            (width, height, 0.0)
        }
        Cutout::DynamicIsland { width, height, top } => (width, height, top),
    };
    Some(match orientation {
        Orientation::Portrait => Rect {
            x: (viewport.width - w) / 2.0,
            y: top,
            width: w,
            height: h,
        },
        // In landscape the cutout moves to the side with the larger margin.
        Orientation::Landscape => {
            let x = if safe.right > safe.left {
                viewport.width - top - h
            } else {
                top
            };
            Rect {
                x,
                y: (viewport.height - w) / 2.0,
                width: h,
                height: w,
            }
        }
    })
}

/// The CSS variables that the cutout occupies. Cleared when the cutout disappears (rotation,
/// change of screen), so that a stale rectangle never survives.
pub const CUTOUT_VARS: [&str; 4] = [
    "--chrome-cutout-x",
    "--chrome-cutout-y",
    "--chrome-cutout-w",
    "--chrome-cutout-h",
];

impl ChromeLayout {
    /// The CSS variables to set on the root element. A style reads `var(--chrome-top)`, never
    /// `env(safe-area-inset-top)`: the decision is made here, once.
    pub fn css_vars(&self) -> Vec<(&'static str, String)> {
        let px = |v: f32| format!("{}px", v.round() as i32);
        let mut vars = vec![
            ("--chrome-top", px(self.insets.top)),
            ("--chrome-right", px(self.insets.right)),
            ("--chrome-bottom", px(self.insets.bottom)),
            ("--chrome-left", px(self.insets.left)),
            ("--chrome-header-min", px(self.header_min_height)),
            ("--chrome-lead", px(self.header_lead)),
            ("--chrome-trail", px(self.header_trail)),
        ];
        if let Some(r) = self.cutout_rect {
            vars.push(("--chrome-cutout-x", px(r.x)));
            vars.push(("--chrome-cutout-y", px(r.y)));
            vars.push(("--chrome-cutout-w", px(r.width)));
            vars.push(("--chrome-cutout-h", px(r.height)));
        }
        vars
    }

    /// The attributes to set on the root element, for what a variable does not say (hiding a
    /// button, changing an animation around the island).
    pub fn attributes(&self) -> Vec<(&'static str, &'static str)> {
        let flag = |b: bool| if b { "true" } else { "false" };
        vec![
            ("data-platform", self.platform.id()),
            ("data-titlebar", self.title_bar.id()),
            ("data-fullscreen", flag(self.fullscreen)),
            ("data-focused", flag(self.focused)),
            ("data-drag", flag(self.drag_region)),
            ("data-cutout", self.cutout.id()),
            (
                "data-orientation",
                match self.orientation {
                    Orientation::Portrait => "portrait",
                    Orientation::Landscape => "landscape",
                },
            ),
        ]
    }

    /// One readable line: what the layout understood, and from where. For a settings screen
    /// or a bug report, not for deciding.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.platform.id().to_string()];
        if self.platform.is_desktop() {
            parts.push(format!("barre {}", self.title_bar.id()));
            if self.fullscreen {
                parts.push("plein écran".into());
            }
        }
        if self.platform.is_mobile() {
            let cutout = match self.cutout {
                Cutout::None => "sans découpe",
                Cutout::Notch { .. } => "encoche",
                Cutout::DynamicIsland { .. } => "Dynamic Island",
                Cutout::PunchHole { .. } => "poinçon",
            };
            let source = match self.cutout_source {
                CutoutSource::Model => "modèle",
                CutoutSource::Family => "génération supposée",
                CutoutSource::Inset => "déduit de la marge",
                CutoutSource::Unknown => "inconnu",
            };
            parts.push(format!("{cutout} ({source})"));
        }
        let i = self.insets;
        parts.push(format!(
            "marges {}/{}/{}/{}",
            i.top.round(),
            i.right.round(),
            i.bottom.round(),
            i.left.round()
        ));
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(title_bar: TitleBar, fullscreen: bool) -> ChromeEnv {
        ChromeEnv {
            platform: Platform::Macos,
            title_bar,
            window: WindowState {
                fullscreen,
                maximized: false,
                focused: true,
            },
            viewport: Size {
                width: 1180.0,
                height: 800.0,
            },
            ..Default::default()
        }
    }

    fn iphone(model: Option<&str>, safe: Insets, viewport: Size) -> ChromeEnv {
        ChromeEnv {
            platform: Platform::Ios,
            safe,
            viewport,
            device_model: model.map(str::to_owned),
            ..Default::default()
        }
    }

    const PORTRAIT: Size = Size {
        width: 440.0,
        height: 956.0,
    };
    const LANDSCAPE: Size = Size {
        width: 956.0,
        height: 440.0,
    };

    #[test]
    fn a_native_title_bar_reserves_nothing() {
        let l = layout(&mac(TitleBar::Native, false));
        assert_eq!(l.controls, None);
        assert_eq!(
            (l.header_lead, l.header_trail, l.header_min_height),
            (0.0, 0.0, 0.0)
        );
        assert!(!l.drag_region, "the system bar already moves the window");
    }

    #[test]
    fn an_integrated_mac_bar_leaves_room_for_the_traffic_lights() {
        let l = layout(&mac(TitleBar::Overlay, false));
        let c = l.controls.expect("the lights are there");
        assert_eq!(c.side, Side::Left);
        assert!(c.system, "macOS draws them");
        assert_eq!(l.header_lead, MACOS_TRAFFIC_LIGHTS.width);
        assert_eq!(l.header_trail, 0.0);
        assert_eq!(l.header_min_height, MACOS_TRAFFIC_LIGHTS.height);
        assert!(l.drag_region, "the header becomes the handle");
    }

    #[test]
    fn in_fullscreen_the_traffic_lights_leave_and_so_does_their_room() {
        let l = layout(&mac(TitleBar::Overlay, true));
        assert_eq!(l.controls, None);
        assert_eq!(l.header_lead, 0.0, "the header takes the lights' place");
        assert_eq!(l.header_min_height, 0.0);
        assert!(!l.drag_region, "a full-screen window is not dragged");
        assert!(l.fullscreen);
    }

    #[test]
    fn a_shell_that_moved_the_lights_says_so() {
        let mut env = mac(TitleBar::Overlay, false);
        env.controls = Some(Size {
            width: 92.0,
            height: 52.0,
        });
        let l = layout(&env);
        assert_eq!((l.header_lead, l.header_min_height), (92.0, 52.0));
    }

    #[test]
    fn windows_puts_its_buttons_on_the_right() {
        let env = ChromeEnv {
            platform: Platform::Windows,
            title_bar: TitleBar::Overlay,
            ..Default::default()
        };
        let l = layout(&env);
        assert_eq!(l.header_lead, 0.0);
        assert_eq!(l.header_trail, WINDOWS_CAPTION_BUTTONS.width);
    }

    #[test]
    fn a_frameless_window_asks_the_app_to_draw_its_controls() {
        let env = ChromeEnv {
            platform: Platform::Linux,
            title_bar: TitleBar::Hidden,
            ..Default::default()
        };
        let c = layout(&env).controls.unwrap();
        assert!(!c.system, "nobody else will draw them");
        assert_eq!(c.side, Side::Right);
    }

    #[test]
    fn a_title_bar_setting_means_nothing_on_a_phone() {
        let mut env = iphone(
            Some("iPhone17,2"),
            Insets::new(62.0, 0.0, 34.0, 0.0),
            PORTRAIT,
        );
        env.title_bar = TitleBar::Overlay;
        let l = layout(&env);
        assert_eq!(l.controls, None);
        assert!(!l.drag_region);
    }

    #[test]
    fn the_device_model_tells_island_from_notch_from_nothing() {
        let cases = [
            ("iPhone17,2", "island"),
            ("iPhone15,2", "island"),
            ("iPhone14,7", "notch"),
            ("iPhone17,5", "notch"),
            ("iPhone13,2", "notch"),
            ("iPhone10,3", "notch"),
            ("iPhone10,4", "none"),
            ("iPhone14,6", "none"),
            ("iPad13,4", "none"),
        ];
        for (model, want) in cases {
            let (cutout, source) = ios_cutout(model);
            assert_eq!(cutout.id(), want, "{model}");
            assert_eq!(source, CutoutSource::Model, "{model}");
        }
    }

    #[test]
    fn an_unknown_newer_iphone_is_assumed_island_and_says_it_assumes() {
        let (cutout, source) = ios_cutout("iPhone21,1");
        assert_eq!(cutout.id(), "island");
        assert_eq!(source, CutoutSource::Family);
        assert_eq!(ios_cutout("arm64").1, CutoutSource::Unknown);
    }

    #[test]
    fn without_a_model_the_top_inset_is_the_only_clue() {
        let island = iphone(None, Insets::new(59.0, 0.0, 34.0, 0.0), PORTRAIT);
        let notch = iphone(None, Insets::new(47.0, 0.0, 34.0, 0.0), PORTRAIT);
        let none = iphone(None, Insets::new(20.0, 0.0, 0.0, 0.0), PORTRAIT);
        for (env, want) in [(island, "island"), (notch, "notch"), (none, "none")] {
            let l = layout(&env);
            assert_eq!(l.cutout.id(), want);
            assert_eq!(
                l.cutout_source,
                CutoutSource::Inset,
                "inferred, and marked as such"
            );
        }
    }

    #[test]
    fn the_island_sits_centred_at_the_top_in_portrait() {
        let l = layout(&iphone(
            Some("iPhone17,2"),
            Insets::new(62.0, 0.0, 34.0, 0.0),
            PORTRAIT,
        ));
        let r = l.cutout_rect.unwrap();
        assert_eq!(r.x, (440.0 - 126.0) / 2.0);
        assert_eq!(r.y, 11.0);
        assert_eq!(
            l.insets.top, 62.0,
            "the system imposes its margin; we keep it"
        );
        assert_eq!(l.insets.bottom, 34.0, "home indicator");
    }

    #[test]
    fn in_landscape_the_island_moves_to_the_side_with_the_inset() {
        let l = layout(&iphone(
            Some("iPhone17,2"),
            Insets::new(0.0, 62.0, 21.0, 62.0),
            LANDSCAPE,
        ));
        assert_eq!(l.orientation, Orientation::Landscape);
        let r = l.cutout_rect.unwrap();
        assert!(r.height > r.width, "the island lies on its side");
        assert_eq!(l.insets.left, 62.0);
        assert_eq!(l.insets.top, 0.0, "nothing at the top");
    }

    #[test]
    fn the_wire_is_css_and_attributes_nothing_else() {
        let l = layout(&mac(TitleBar::Overlay, false));
        let vars: std::collections::HashMap<_, _> = l.css_vars().into_iter().collect();
        // Asserted against the constant, not against a number: the test's job is
        // "the band reaches the wire", not "the band is 78" — the numbers follow
        // the macOS version and are measured, so freezing them here would be a
        // second source of truth that drifts silently.
        assert_eq!(
            vars["--chrome-lead"],
            format!("{}px", MACOS_TRAFFIC_LIGHTS.width)
        );
        assert_eq!(
            vars["--chrome-header-min"],
            format!("{}px", MACOS_TRAFFIC_LIGHTS.height)
        );
        assert!(!vars.contains_key("--chrome-cutout-x"));
        let attrs: std::collections::HashMap<_, _> = l.attributes().into_iter().collect();
        assert_eq!(attrs["data-platform"], "macos");
        assert_eq!(attrs["data-drag"], "true");

        let full = layout(&mac(TitleBar::Overlay, true));
        let attrs: std::collections::HashMap<_, _> = full.attributes().into_iter().collect();
        assert_eq!(attrs["data-fullscreen"], "true");
        assert_eq!(attrs["data-drag"], "false");
    }

    #[test]
    fn the_summary_says_what_was_understood_and_from_where() {
        let l = layout(&iphone(None, Insets::new(59.0, 0.0, 34.0, 0.0), PORTRAIT));
        assert_eq!(
            l.summary(),
            "ios · Dynamic Island (déduit de la marge) · marges 59/0/34/0"
        );
        let l = layout(&mac(TitleBar::Overlay, true));
        assert_eq!(
            l.summary(),
            "macos · barre overlay · plein écran · marges 0/0/0/0"
        );
    }

    #[test]
    fn the_env_arrives_as_json_from_any_shell() {
        let env: ChromeEnv = serde_json::from_value(serde_json::json!({
            "platform": "macos",
            "titleBar": "overlay",
            "window": { "fullscreen": false, "maximized": false, "focused": true }
        }))
        .unwrap();
        assert_eq!(env.title_bar, TitleBar::Overlay);
        assert_eq!(env.device_model, None);
    }
}
