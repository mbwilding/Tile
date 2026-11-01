use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use tile_core::{LayoutKind, LayoutSettings, animation::Bezier, layout::CenterMode};

/// The whole config file. Missing sections fall back to the shipped defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub theme: Theme,
    pub cursor: Cursor,
    pub tray: Tray,
    pub shell: Shell,
    pub animations: Animations,
    pub dwindle: Dwindle,
    pub master: Master,
    pub scrolling: Scrolling,
    pub remap: Remap,
    pub launcher: Launcher,
    pub workspace_indicator: WorkspaceIndicator,
    /// `super+shift+q: killactive`, added to (or overriding) the default binds unless
    /// `general.default_binds` is off. An empty value unbinds a default.
    pub binds: BTreeMap<String, Option<String>>,
    pub rules: Vec<Rule>,
}

/// The program a command line starts: its first word, or the quoted path it starts with.
pub fn program(command: &str) -> &str {
    let command = command.trim();
    match command.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(quoted),
        None => command.split_whitespace().next().unwrap_or(command),
    }
}

/// The default binds, which `binds` add to unless `general.default_binds` is off.
const DEFAULT_BINDS: &[(&str, &str)] = &[
    ("super+b", "exec chrome.exe --new-window"),
    ("super+t", "exec wt.exe -w new"),
    ("super+q", "killactive"),
    ("super+enter", "swapwithmaster"),
    ("super+f", "togglefloating"),
    ("super+o", "fullscreen"),
    ("super+space", "cyclelayout"),
    ("super+n", "cyclenext"),
    ("super+z", "lock"),
    ("super+left", "movefocus l"),
    ("super+right", "movefocus r"),
    ("super+up", "movefocus u"),
    ("super+down", "movefocus d"),
    ("super+h", "movefocus l"),
    ("super+j", "movefocus d"),
    ("super+k", "movefocus u"),
    ("super+l", "movefocus r"), // needs `override_win_l` (the default)
    ("super+shift+left", "movewindow l"),
    ("super+shift+right", "movewindow r"),
    ("super+shift+up", "movewindow u"),
    ("super+shift+down", "movewindow d"),
    ("super+shift+h", "movewindow l"),
    ("super+shift+j", "movewindow d"),
    ("super+shift+k", "movewindow u"),
    ("super+shift+l", "movewindow r"),
    ("super+alt+left", "resizeactive -40 0"),
    ("super+alt+right", "resizeactive 40 0"),
    ("super+alt+up", "resizeactive 0 -40"),
    ("super+alt+down", "resizeactive 0 40"),
    ("super+alt+h", "resizeactive -40 0"),
    ("super+alt+j", "resizeactive 0 40"),
    ("super+alt+k", "resizeactive 0 -40"),
    ("super+alt+l", "resizeactive 40 0"),
    // Win+Ctrl+Left/Right, Win+Ctrl+D and Win+Tab are left to Windows' virtual desktops.
    ("super+ctrl+shift+left", "movetodesktop prev"),
    ("super+ctrl+shift+right", "movetodesktop next"),
    ("super+ctrl+p", "pin"),
    ("super+comma", "focusmonitor l"),
    ("super+period", "focusmonitor r"),
    ("super+s", "togglesplit"),
    ("super+r", "cycleproportion"),
    ("super+c", "centercolumn"),
    ("super+bracketleft", "consumewindow l"),
    ("super+bracketright", "consumewindow r"),
    ("super+backslash", "expelwindow"),
    ("super+shift+r", "reload"),
    ("super+shift+p", "togglepause"),
    ("super+shift+e", "exit"),
];

/// The default binds, by key.
pub fn default_binds() -> BTreeMap<String, String> {
    let mut binds: BTreeMap<String, String> = DEFAULT_BINDS
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    for n in 1..=9 {
        binds.insert(format!("super+{n}"), format!("workspace {n}"));
        binds.insert(format!("super+shift+{n}"), format!("movetoworkspace {n}"));
    }
    binds
}

/// Window rules used when the config has no `rules`.
fn default_rules() -> Vec<Rule> {
    vec![
        // Standard dialogs.
        Rule {
            class: Some("^#32770$".into()),
            float: Some(true),
            ..Rule::default()
        },
        Rule {
            class: Some(
                r"^(Windows\.UI\.Core\.CoreWindow|XamlExplorerHostIslandWindow|Shell_TrayWnd)$"
                    .into(),
            ),
            ignore: true,
            ..Rule::default()
        },
        Rule {
            exe: Some(r"^(PowerToys.*|ShareX)\.exe$".into()),
            ignore: true,
            ..Rule::default()
        },
    ]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            theme: Theme::default(),
            cursor: Cursor::default(),
            tray: Tray::default(),
            shell: Shell::default(),
            animations: Animations::default(),
            dwindle: Dwindle::default(),
            master: Master::default(),
            scrolling: Scrolling::default(),
            remap: Remap::default(),
            launcher: Launcher::default(),
            workspace_indicator: WorkspaceIndicator::default(),
            binds: BTreeMap::new(),
            rules: default_rules(),
        }
    }
}

impl Config {
    pub fn layout_settings(&self) -> LayoutSettings {
        LayoutSettings {
            gaps_in: self.general.gaps_in,
            gaps_out: self.general.gaps_out,
            dwindle_split_ratio: self.dwindle.split_ratio.clamp(0.1, 0.9),
            master_mfact: self.master.mfact.clamp(0.1, 0.9),
            master_count: self.master.count.max(1),
            master_new_is_master: self.master.new_is_master,
            scrolling_proportion: self.scrolling.proportion.clamp(0.1, 1.0),
            scrolling_presets: self.scrolling.proportion_presets.clone(),
            scrolling_center: self.scrolling.center_focused,
            independent_workspaces: self.general.independent_workspaces,
        }
    }

    /// The focused window's border colour.
    pub fn border_active(&self) -> Color {
        self.general.border_active.unwrap_or(self.theme.accent)
    }

    /// Every other tiled window's border colour.
    pub fn border_inactive(&self) -> Color {
        self.general.border_inactive.unwrap_or(self.theme.inactive)
    }

    /// The launcher's look, with `theme` filling in what it leaves unset.
    pub fn launcher_style(&self) -> LauncherStyle {
        let (theme, l) = (&self.theme, &self.launcher);
        LauncherStyle {
            font: l.font.clone().unwrap_or_else(|| theme.font.clone()),
            corners: l.corners.unwrap_or(theme.corners),
            background: l.background.unwrap_or(theme.background),
            border: l.border.unwrap_or(theme.accent),
            foreground: l.foreground.unwrap_or(theme.foreground),
            dimmed: l.dimmed.unwrap_or(theme.dimmed),
            selected: l.selected.unwrap_or(theme.accent),
            selected_foreground: l.selected_foreground.unwrap_or(theme.accent_foreground),
        }
    }

    /// The workspace indicator's look, with `theme` filling in what it leaves unset.
    pub fn indicator_style(&self) -> IndicatorStyle {
        let (theme, i) = (&self.theme, &self.workspace_indicator);
        IndicatorStyle {
            font: i.font.clone().unwrap_or_else(|| theme.font.clone()),
            corners: i.corners.unwrap_or(theme.corners),
            background: i.background.unwrap_or(theme.accent),
            foreground: i.foreground.unwrap_or(theme.accent_foreground),
        }
    }
}

/// The font, corners and colours shared by everything Tile draws, unless its own section
/// sets them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub font: String,
    pub corners: Corners,
    /// The launcher's window.
    pub background: Color,
    /// Text.
    pub foreground: Color,
    /// Secondary text.
    pub dimmed: Color,
    /// The focused window's border, the launcher's outline and selected row, and the workspace
    /// indicator.
    pub accent: Color,
    /// Text on `accent`.
    pub accent_foreground: Color,
    /// The border of tiled windows without focus.
    pub inactive: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            font: "Segoe UI".into(),
            corners: Corners::Round,
            background: Color::rgb(0x1e, 0x1e, 0x2e),
            foreground: Color::rgb(0xcd, 0xd6, 0xf4),
            dimmed: Color::rgb(0x7f, 0x84, 0x9c),
            accent: Color::rgb(0x89, 0xb4, 0xfa),
            accent_foreground: Color::rgb(0x1e, 0x1e, 0x2e),
            inactive: Color::rgb(0x45, 0x47, 0x5a),
        }
    }
}

/// The launcher's font, corners and colours, resolved against `theme`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherStyle {
    pub font: String,
    pub corners: Corners,
    pub background: Color,
    pub border: Color,
    pub foreground: Color,
    pub dimmed: Color,
    pub selected: Color,
    pub selected_foreground: Color,
}

/// The workspace indicator's font, corners and colours, resolved against `theme`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndicatorStyle {
    pub font: String,
    pub corners: Corners,
    pub background: Color,
    pub foreground: Color,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// Manage windows: layouts, `binds` and `rules`. Off, Tile leaves windows alone.
    pub tiling: bool,
    /// Start from the default binds, which `binds` add to. Off, only `binds` apply.
    pub default_binds: bool,
    /// Remap keys with the `remap` layers.
    pub remap: bool,
    /// Bind the launcher to `Win+R` (or `Win+D` if that is taken) when no bind opens it, with
    /// tiling on or off. In shell mode the launcher is bound whatever this says.
    pub launcher: bool,
    /// Space between neighbouring tiled windows.
    pub gaps_in: i32,
    /// Space between tiled windows and the monitor edge.
    pub gaps_out: i32,
    /// Each monitor has workspaces of its own, so `workspace 2` means workspace 2 on the
    /// monitor you are on. Off, the monitors are synced: switching workspace switches them all.
    pub independent_workspaces: bool,
    pub layout: LayoutKind,
    pub borders: bool,
    /// Unset follows `theme.accent`.
    pub border_active: Option<Color>,
    /// Unset follows `theme.inactive`.
    pub border_inactive: Option<Color>,
    pub focus_new_windows: bool,
    /// Keep title bars on tiled windows.
    pub decorations: bool,
    /// Disable Windows' Win+L lock shortcut while Tile runs (on by default), so it can be bound.
    /// Use the `lock` dispatcher to lock instead. Restored on exit, even after a crash.
    pub override_win_l: bool,
    /// Switch off Windows' Click to Do (Copilot+ PCs): holding Win would otherwise glow around
    /// the pointer. It stays off after Tile exits; turn it back on in Settings.
    pub disable_click_to_do: bool,
    /// Releasing Win on its own no longer opens the Start menu. Win combos and binds are
    /// unaffected.
    pub disable_start_menu: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            tiling: true,
            default_binds: true,
            remap: true,
            launcher: true,
            gaps_in: 10,
            gaps_out: 10,
            independent_workspaces: true,
            layout: LayoutKind::Master,
            borders: true,
            border_active: None,
            border_inactive: None,
            focus_new_windows: true,
            decorations: false,
            override_win_l: true,
            disable_click_to_do: true,
            disable_start_menu: false,
        }
    }
}

/// The tray's switches, saved here whenever they are changed so they survive a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tray {
    /// Window management is on.
    pub tile: bool,
    /// Key remapping is on. Off until switched on.
    pub remap: bool,
    /// Logging to the log file is on.
    pub log: bool,
    /// Tile is the shell: it hosts the notification area when no taskbar is running, keeps the
    /// launcher bound and lists the tray icons in it.
    pub shell: bool,
}

impl Default for Tray {
    fn default() -> Self {
        Self {
            tile: true,
            remap: false,
            log: false,
            shell: false,
        }
    }
}

/// Tile as the Windows shell, in place of Explorer's taskbar. Whether it is the shell is the
/// tray's Shell switch (`tray.shell`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shell {
    /// Run the `Run` and `RunOnce` registry keys and the Startup folders when Tile starts as
    /// the shell, as Explorer would.
    pub run_startup: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self { run_startup: true }
    }
}

/// The mouse pointer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cursor {
    /// Move the pointer onto the focused window (the middle of the monitor, if it is empty)
    /// whenever a keybind or dispatch moves focus, then hide it like typing does, until the
    /// mouse moves. It stays put if it is already over the window.
    pub warp: bool,
    /// Hide the pointer when a key is typed, until the mouse moves or clicks, or another
    /// monitor is focused. Modifiers on their own and keybinds don't count as typing.
    pub hide_on_key: bool,
    /// Hold Win and drag a window with the left button, from anywhere on it.
    pub drag: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            warp: true,
            hide_on_key: true,
            drag: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Animations {
    pub enabled: bool,
    pub duration_ms: u64,
    pub bezier: [f32; 4],
}

impl Default for Animations {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_ms: 180,
            bezier: [0.0, 0.75, 0.15, 1.0],
        }
    }
}

impl Animations {
    pub const fn curve(&self) -> Bezier {
        let [x1, y1, x2, y2] = self.bezier;
        Bezier::new(x1, y1, x2, y2)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dwindle {
    pub split_ratio: f32,
}

impl Default for Dwindle {
    fn default() -> Self {
        Self { split_ratio: 0.5 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Master {
    pub mfact: f32,
    pub count: usize,
    pub new_is_master: bool,
}

impl Default for Master {
    fn default() -> Self {
        Self {
            mfact: 0.5,
            count: 1,
            new_is_master: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scrolling {
    pub proportion: f32,
    pub proportion_presets: Vec<f32>,
    pub center_focused: CenterMode,
}

impl Default for Scrolling {
    fn default() -> Self {
        Self {
            proportion: 0.333,
            proportion_presets: vec![0.25, 0.333, 0.5, 1.0],
            center_focused: CenterMode::OnOverflow,
        }
    }
}

/// Key remapping, keyd style. Each `layers` entry is a layer named as a keyd section header:
/// `main`, `name`, `name:C` (a layer whose unbound keys get Control) or `a+b` (while both
/// are active). It maps keys to actions, e.g. `capslock: overload(nav, esc)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Remap {
    /// Tap a dummy key before a layer's lone Alt or Win is released, so it doesn't open a menu
    /// or Start (keyd's modifier guard).
    pub modifier_guard: bool,
    pub layers: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for Remap {
    fn default() -> Self {
        Self {
            modifier_guard: true,
            layers: BTreeMap::new(),
        }
    }
}

/// The app launcher opened by the `launcher` dispatcher: a search box over everything Start
/// lists, centred on the focused monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Launcher {
    /// Width in pixels at 100% scaling.
    pub width: u32,
    /// The most result rows shown at once. The window is as tall as its content, up to this;
    /// more results scroll.
    pub max_visible: usize,
    pub search_position: SearchPosition,
    /// Show each app's icon.
    pub show_icons: bool,
    /// Shorter rows, without the app id under the name.
    pub compact: bool,
    pub max_results: usize,
    /// Remember launches, for `sort_by_usage`.
    pub track_usage: bool,
    /// List recently launched apps first, and let often launched ones win close matches.
    pub sort_by_usage: bool,
    /// The mouse doesn't select or launch.
    pub ignore_mouse: bool,
    /// App names or ids listed first, in this order, before anything is typed.
    pub pinned: Vec<String>,
    /// Regexes (case-insensitive) on the app name or id; matching apps are never listed.
    pub hide: Vec<String>,
    /// Unset follows `theme.font`.
    pub font: Option<String>,
    /// Font size in points.
    pub font_size: u32,
    /// Unset follows `theme.corners`.
    pub corners: Option<Corners>,
    /// How opaque the whole window is, from 0.1 to 1.0.
    pub opacity: f32,
    /// Unset follows `theme.background`.
    pub background: Option<Color>,
    /// The window's outline (Windows 11). Unset follows `theme.accent`.
    pub border: Option<Color>,
    /// Unset follows `theme.foreground`.
    pub foreground: Option<Color>,
    /// The app id line and the placeholder. Unset follows `theme.dimmed`.
    pub dimmed: Option<Color>,
    /// The selected row. Unset follows `theme.accent`.
    pub selected: Option<Color>,
    /// Text on the selected row. Unset follows `theme.accent_foreground`.
    pub selected_foreground: Option<Color>,
    /// Shown in the empty search box. Empty shows `Search apps, equations and commands`,
    /// leaving out what is switched off.
    pub placeholder: String,
    /// Show the value of a search that reads as a calculation (`12 * 3`, `sqrt(2)`) first;
    /// Enter copies it.
    pub calculator: bool,
    /// Offer Tile's dispatchers (`workspace 3`) and running what is typed as a command line
    /// among the results.
    pub commands: bool,
    /// Offer the system items (volume, network, Wi-Fi, Bluetooth, battery, clock and power
    /// actions) and the notification area's tray icons among the results when they match what
    /// is typed.
    pub system_items: bool,
    /// Show the bar at the bottom with the time (with seconds), the date and the buttons for
    /// switching user, logging out, locking, restarting and shutting down.
    pub bar: bool,
    /// Some apps place the popup of their tray icon by finding the taskbar, which is not there
    /// in shell mode, and open it in the corner of a monitor. Move such a popup next to the
    /// pointer, in the moment after the click.
    pub fix_popups: bool,
}

impl Launcher {
    /// The text shown in the empty search box.
    pub fn placeholder_text(&self) -> String {
        if !self.placeholder.is_empty() {
            return self.placeholder.clone();
        }
        match (self.calculator, self.commands) {
            (true, true) => "Search apps, equations and commands",
            (true, false) => "Search apps or equations",
            (false, true) => "Search apps and commands",
            (false, false) => "Search apps",
        }
        .into()
    }
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            width: 560,
            max_visible: 6,
            search_position: SearchPosition::Top,
            show_icons: true,
            compact: false,
            max_results: 50,
            track_usage: true,
            sort_by_usage: true,
            ignore_mouse: false,
            pinned: Vec::new(),
            hide: Vec::new(),
            font: None,
            font_size: 11,
            corners: None,
            opacity: 1.0,
            background: None,
            border: None,
            foreground: None,
            dimmed: None,
            selected: None,
            selected_foreground: None,
            placeholder: String::new(),
            calculator: true,
            commands: true,
            system_items: true,
            bar: true,
            fix_popups: true,
        }
    }
}

/// Where the launcher's search box sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchPosition {
    #[default]
    Top,
    Bottom,
}

/// The workspace indicator: a small square showing the workspace number that flashes up on the
/// focused monitor when a `workspace` or `movetoworkspace` dispatcher runs, then fades out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceIndicator {
    pub enabled: bool,
    pub position: IndicatorPosition,
    /// Pixels between the indicator and the edges of the work area it sits against, at 100%
    /// scaling. Unused when `position` is `center`.
    pub gap: u32,
    /// Width and height in pixels at 100% scaling. It widens for a label that doesn't fit.
    pub size: u32,
    /// How long it stays fully shown, in milliseconds.
    pub duration_ms: u64,
    /// How long it takes to fade out after that, in milliseconds.
    pub fade_ms: u64,
    /// Unset follows `theme.font`.
    pub font: Option<String>,
    /// Font size in points.
    pub font_size: u32,
    pub bold: bool,
    /// Unset follows `theme.corners`.
    pub corners: Option<Corners>,
    /// How opaque it is when fully shown, from 0.1 to 1.0.
    pub opacity: f32,
    /// Unset follows `theme.accent`.
    pub background: Option<Color>,
    /// The workspace number. Unset follows `theme.accent_foreground`.
    pub foreground: Option<Color>,
}

impl Default for WorkspaceIndicator {
    fn default() -> Self {
        Self {
            enabled: true,
            position: IndicatorPosition::BottomRight,
            gap: 48,
            size: 96,
            duration_ms: 400,
            fade_ms: 250,
            font: None,
            font_size: 36,
            bold: true,
            corners: None,
            opacity: 0.95,
            background: None,
            foreground: None,
        }
    }
}

/// Where on the monitor's work area the workspace indicator sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorPosition {
    Center,
    Top,
    TopLeft,
    TopRight,
    Bottom,
    BottomLeft,
    #[default]
    BottomRight,
    Left,
    Right,
}

/// Window corner rounding (Windows 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corners {
    #[default]
    Round,
    Small,
    Square,
}

/// A window rule. All given matchers must match for the actions to apply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    pub exe: Option<String>,
    pub class: Option<String>,
    pub title: Option<String>,
    pub float: Option<bool>,
    pub workspace: Option<u8>,
    pub monitor: Option<usize>,
    /// Force the title bar on or off for matching windows, tiled or floating.
    pub decorations: Option<bool>,
    pub ignore: bool,
}

/// An sRGB color written as `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.trim().trim_start_matches('#');
        let channel = |i: usize| {
            hex.get(i..i + 2)
                .and_then(|c| u8::from_str_radix(c, 16).ok())
                .ok_or_else(|| format!("invalid color `{s}`, expected #rrggbb"))
        };
        if hex.len() != 6 {
            return Err(format!("invalid color `{s}`, expected #rrggbb"));
        }
        Ok(Self::rgb(channel(0)?, channel(2)?, channel(4)?))
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Color> for String {
    fn from(color: Color) -> Self {
        color.to_string()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_is_the_first_word_or_quoted_path() {
        assert_eq!(program("  notepad.exe file.txt"), "notepad.exe");
        assert_eq!(
            program(r#""C:\Program Files\App\app.exe" --flag"#),
            r"C:\Program Files\App\app.exe"
        );
        assert_eq!(program(r#""unterminated path"#), "unterminated path");
        assert_eq!(program(""), "");
    }

    #[test]
    fn layout_settings_clamp_out_of_range_values() {
        let mut config = Config::default();
        config.dwindle.split_ratio = 5.0;
        config.master.mfact = -1.0;
        config.master.count = 0;
        config.scrolling.proportion = 0.0;
        let settings = config.layout_settings();
        assert_eq!(settings.dwindle_split_ratio, 0.9);
        assert_eq!(settings.master_mfact, 0.1);
        assert_eq!(settings.master_count, 1);
        assert_eq!(settings.scrolling_proportion, 0.1);
    }

    #[test]
    fn borders_fall_back_to_the_theme() {
        let mut config = Config::default();
        assert_eq!(config.border_active(), config.theme.accent);
        assert_eq!(config.border_inactive(), config.theme.inactive);
        config.general.border_active = Some(Color::rgb(1, 2, 3));
        config.general.border_inactive = Some(Color::rgb(4, 5, 6));
        assert_eq!(config.border_active(), Color::rgb(1, 2, 3));
        assert_eq!(config.border_inactive(), Color::rgb(4, 5, 6));
    }

    #[test]
    fn colours_parse_with_or_without_a_hash() {
        assert_eq!("#0a1B2c".parse(), Ok(Color::rgb(10, 27, 44)));
        assert_eq!(" ff0000 ".parse(), Ok(Color::rgb(255, 0, 0)));
        for bad in ["", "#fff", "#12345g", "#1234567", "red"] {
            assert!(bad.parse::<Color>().is_err(), "{bad}");
        }
    }

    #[test]
    fn animation_curve_uses_the_configured_points() {
        let animations = Animations::default();
        let [x1, y1, x2, y2] = animations.bezier;
        assert_eq!(animations.curve(), Bezier::new(x1, y1, x2, y2));
    }
}
