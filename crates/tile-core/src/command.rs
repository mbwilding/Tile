use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{geometry::Direction, layout::LayoutKind};

/// An action, written like `workspace 3`. Shared by keybinds, IPC and the `tile` command line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Start a program from a command line, a new instance every time.
    Exec(String),
    /// Run a command line with no window at all, for a console program that has nothing to show.
    ExecHidden(String),
    /// Focus a window of the command's program, or run the command if there is none.
    FocusOrExec(String),
    /// Close the focused window, like Alt+F4.
    KillActive,
    /// Float the focused window, or tile it again.
    ToggleFloating,
    /// Make the focused window fill its monitor, or stop.
    Fullscreen,
    /// Focus the nearest window that way, or the next monitor at the edge.
    MoveFocus(Direction),
    /// Move the focused window that way, to the next monitor at the edge.
    MoveWindow(Direction),
    /// Grow (or shrink, if negative) the active window by pixels.
    ResizeActive(i32, i32),
    /// Swap the focused window with the main one (or the main one with the next).
    SwapWithMaster,
    /// Focus the next / previous window of the workspace, in layout order.
    CycleNext,
    /// See [`CycleNext`](Self::CycleNext).
    CyclePrev,
    /// Dwindle: flip the direction of the focused window's split.
    ToggleSplit,
    /// Switch the focused monitor (or every monitor, when synced) to this workspace.
    Workspace(u8),
    /// Move the focused window to this workspace and follow it.
    MoveToWorkspace(u8),
    /// Move the focused window to this workspace and stay.
    MoveToWorkspaceSilent(u8),
    /// Focus the next monitor that way.
    FocusMonitor(Direction),
    /// Set the workspace's layout.
    Layout(LayoutKind),
    /// Step the workspace through the layouts.
    CycleLayout,
    /// Rebuild the workspace's layout as new, undoing every resize and split change.
    ResetLayout,
    /// Scrolling layout: set the focused column's width as a fraction of the monitor.
    ScrollerProportion(f32),
    /// Scrolling layout: step the focused column through the width presets.
    CycleProportion,
    /// Scrolling layout: move the focused window into the column that way.
    ConsumeWindow(Direction),
    /// Scrolling layout: move the focused window into a column of its own.
    ExpelWindow,
    /// Scrolling layout: centre the focused column.
    CenterColumn,
    /// Switch native virtual desktop.
    Desktop(DesktopTarget),
    /// Send the active window to another native virtual desktop.
    MoveToDesktop(DesktopTarget),
    /// Toggle "show this window on all desktops".
    Pin,
    /// Lock the workstation (works even while Win+L is taken over).
    Lock,
    /// Open (or close) the app launcher.
    Launcher,
    /// Reload the config.
    Reload,
    /// Pause both tiling and remapping, or resume both.
    TogglePause,
    /// Pause or resume the window manager, like the tray's Tile item.
    ToggleTile,
    /// Pause or resume the key remapper, like the tray's Remap item.
    ToggleRemap,
    /// Make Tile the shell, or stop, like the tray's Shell item.
    ToggleShell,
    /// Switch logging on or off, like the tray's Log item.
    ToggleLog,
    /// Mute notification popups, or let them show again. Doesn't touch Windows' own notification
    /// settings; Action Center and Focus Assist are unaffected.
    ToggleDnd,
    /// Change the wallpaper Tile paints in shell mode.
    Wallpaper(WallpaperAction),
    /// Open Tile's config file, its folder or its logs.
    Open(OpenTarget),
    /// Quit Tile.
    Exit,
}

/// What `wallpaper` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WallpaperAction {
    /// The next image, in the cycle's order.
    Next,
    /// The image shown before this one.
    Prev,
    /// Any image not yet shown in this round, whatever the cycle's order.
    Random,
    /// Stop the timed cycle, or start it again.
    Pause,
    /// This image file.
    Set(String),
}

impl fmt::Display for WallpaperAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Next => f.write_str("next"),
            Self::Prev => f.write_str("prev"),
            Self::Random => f.write_str("random"),
            Self::Pause => f.write_str("pause"),
            Self::Set(path) => f.write_str(path),
        }
    }
}

/// What `open` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenTarget {
    /// The config file, in your editor.
    Config,
    /// The folder the config file is in.
    Folder,
    /// The folder of Tile's logs.
    Logs,
}

impl fmt::Display for OpenTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Config => "config",
            Self::Folder => "folder",
            Self::Logs => "logs",
        })
    }
}

/// A native virtual desktop: zero-based index, or relative to the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopTarget {
    /// The desktop at this zero-based index.
    Index(u32),
    /// The one after the current desktop.
    Next,
    /// The one before the current desktop.
    Previous,
}

fn parse_desktop(name: &str, arg: Option<&str>) -> Result<DesktopTarget, ParseError> {
    const EXPECTED: &str = "1-n|next|prev";
    let arg = arg.ok_or_else(|| ParseError::MissingArg(name.into(), EXPECTED))?;
    match arg.to_ascii_lowercase().as_str() {
        "next" | "+1" | "r" => Ok(DesktopTarget::Next),
        "prev" | "previous" | "-1" | "l" => Ok(DesktopTarget::Previous),
        n => match n.parse::<u32>() {
            Ok(n) if n > 0 => Ok(DesktopTarget::Index(n - 1)),
            _ => Err(ParseError::BadArg(name.into(), arg.into(), EXPECTED)),
        },
    }
}

impl fmt::Display for DesktopTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Index(i) => write!(f, "{}", i + 1),
            Self::Next => f.write_str("next"),
            Self::Previous => f.write_str("prev"),
        }
    }
}

/// Every dispatcher, with a hint for its argument (empty if it takes none).
pub const DISPATCHERS: &[(&str, &str)] = &[
    ("exec", "<command line>"),
    ("exechidden", "<command line>"),
    ("focusorexec", "<command line>"),
    ("killactive", ""),
    ("togglefloating", ""),
    ("fullscreen", ""),
    ("movefocus", "<l|r|u|d>"),
    ("movewindow", "<l|r|u|d>"),
    ("resizeactive", "<dx> <dy>"),
    ("swapwithmaster", ""),
    ("cyclenext", ""),
    ("cycleprev", ""),
    ("togglesplit", ""),
    ("workspace", "<1-255>"),
    ("movetoworkspace", "<1-255>"),
    ("movetoworkspacesilent", "<1-255>"),
    ("focusmonitor", "<l|r|u|d>"),
    ("layout", "<dwindle|master|monocle|scrolling|even>"),
    ("cyclelayout", ""),
    ("resetlayout", ""),
    ("scroller_proportion", "<0.05-1.0>"),
    ("cycleproportion", ""),
    ("consumewindow", "<l|r>"),
    ("expelwindow", ""),
    ("centercolumn", ""),
    ("desktop", "<n|next|prev>"),
    ("movetodesktop", "<n|next|prev>"),
    ("pin", ""),
    ("lock", ""),
    ("launcher", ""),
    ("reload", ""),
    ("togglepause", ""),
    ("toggletile", ""),
    ("toggleremap", ""),
    ("toggleshell", ""),
    ("togglelog", ""),
    ("togglednd", ""),
    ("wallpaper", "<next|prev|random|pause|image path>"),
    ("open", "<config|folder|logs>"),
    ("exit", ""),
];

impl Command {
    /// Works with tiling off or paused, since it doesn't need the window manager.
    pub const fn works_while_paused(&self) -> bool {
        matches!(
            self,
            Self::Exec(_)
                | Self::ExecHidden(_)
                | Self::FocusOrExec(_)
                | Self::KillActive
                | Self::Desktop(_)
                | Self::Lock
                | Self::Launcher
                | Self::Reload
                | Self::TogglePause
                | Self::ToggleTile
                | Self::ToggleRemap
                | Self::ToggleShell
                | Self::ToggleLog
                | Self::ToggleDnd
                | Self::Wallpaper(_)
                | Self::Open(_)
                | Self::Exit
        )
    }
}

/// Why a command didn't parse.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    /// Nothing was written.
    #[error("empty command")]
    Empty,
    /// No dispatcher has this name.
    #[error("unknown dispatcher `{0}`")]
    Unknown(String),
    /// The dispatcher needs an argument; the second field lists what it accepts.
    #[error("`{0}` expects an argument: {1}")]
    MissingArg(String, &'static str),
    /// The argument isn't one the dispatcher accepts; the third field lists what it does.
    #[error("invalid argument `{1}` for `{0}`: expected {2}")]
    BadArg(String, String, &'static str),
}

fn parse_direction(name: &str, arg: Option<&str>) -> Result<Direction, ParseError> {
    let arg = arg.ok_or_else(|| ParseError::MissingArg(name.into(), "l|r|u|d"))?;
    match arg.to_ascii_lowercase().as_str() {
        "l" | "left" => Ok(Direction::Left),
        "r" | "right" => Ok(Direction::Right),
        "u" | "up" | "t" | "top" => Ok(Direction::Up),
        "d" | "down" | "b" | "bottom" => Ok(Direction::Down),
        _ => Err(ParseError::BadArg(name.into(), arg.into(), "l|r|u|d")),
    }
}

fn parse_workspace(name: &str, arg: Option<&str>) -> Result<u8, ParseError> {
    let arg = arg.ok_or_else(|| ParseError::MissingArg(name.into(), "1-255"))?;
    match arg.parse::<u8>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(ParseError::BadArg(name.into(), arg.into(), "1-255")),
    }
}

impl FromStr for Command {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (name, rest) = match s.split_once(char::is_whitespace) {
            Some((name, rest)) => (name, Some(rest.trim()).filter(|r| !r.is_empty())),
            None => (s, None),
        };
        if name.is_empty() {
            return Err(ParseError::Empty);
        }
        let name = name.to_ascii_lowercase();
        let n = name.as_str();
        Ok(match n {
            "exec" => Self::Exec(
                rest.ok_or_else(|| ParseError::MissingArg(name.clone(), "a command line"))?
                    .to_string(),
            ),
            "exechidden" => Self::ExecHidden(
                rest.ok_or_else(|| ParseError::MissingArg(name.clone(), "a command line"))?
                    .to_string(),
            ),
            "focusorexec" => Self::FocusOrExec(
                rest.ok_or_else(|| ParseError::MissingArg(name.clone(), "a command line"))?
                    .to_string(),
            ),
            "killactive" => Self::KillActive,
            "togglefloating" => Self::ToggleFloating,
            "fullscreen" => Self::Fullscreen,
            "movefocus" => Self::MoveFocus(parse_direction(n, rest)?),
            "movewindow" => Self::MoveWindow(parse_direction(n, rest)?),
            "resizeactive" => {
                let rest = rest.ok_or_else(|| ParseError::MissingArg(name.clone(), "dx dy"))?;
                let bad = || ParseError::BadArg(name.clone(), rest.into(), "two integers");
                let mut parts = rest.split_whitespace().map(str::parse::<i32>);
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(Ok(dx)), Some(Ok(dy)), None) => Self::ResizeActive(dx, dy),
                    _ => return Err(bad()),
                }
            }
            "swapwithmaster" => Self::SwapWithMaster,
            "cyclenext" => Self::CycleNext,
            "cycleprev" => Self::CyclePrev,
            "togglesplit" => Self::ToggleSplit,
            "workspace" => Self::Workspace(parse_workspace(n, rest)?),
            "movetoworkspace" => Self::MoveToWorkspace(parse_workspace(n, rest)?),
            "movetoworkspacesilent" => Self::MoveToWorkspaceSilent(parse_workspace(n, rest)?),
            "focusmonitor" => Self::FocusMonitor(parse_direction(n, rest)?),
            "layout" => {
                let arg = rest.ok_or_else(|| {
                    ParseError::MissingArg(name.clone(), "dwindle|master|monocle|scrolling|even")
                })?;
                Self::Layout(arg.parse().map_err(|()| {
                    ParseError::BadArg(
                        name.clone(),
                        arg.into(),
                        "dwindle|master|monocle|scrolling|even",
                    )
                })?)
            }
            "cyclelayout" => Self::CycleLayout,
            "scroller_proportion" | "scrollerproportion" => {
                let arg = rest.ok_or_else(|| ParseError::MissingArg(name.clone(), "0.05-1.0"))?;
                match arg.parse::<f32>() {
                    Ok(p) if (0.05..=1.0).contains(&p) => Self::ScrollerProportion(p),
                    _ => return Err(ParseError::BadArg(name, arg.into(), "0.05-1.0")),
                }
            }
            "resetlayout" => Self::ResetLayout,
            "cycleproportion" => Self::CycleProportion,
            "consumewindow" => Self::ConsumeWindow(parse_direction(n, rest)?),
            "expelwindow" => Self::ExpelWindow,
            "centercolumn" => Self::CenterColumn,
            "desktop" => Self::Desktop(parse_desktop(n, rest)?),
            "movetodesktop" => Self::MoveToDesktop(parse_desktop(n, rest)?),
            "pin" => Self::Pin,
            "lock" => Self::Lock,
            "launcher" => Self::Launcher,
            "reload" => Self::Reload,
            "togglepause" => Self::TogglePause,
            "toggletile" => Self::ToggleTile,
            "toggleremap" => Self::ToggleRemap,
            "toggleshell" => Self::ToggleShell,
            "togglelog" => Self::ToggleLog,
            "togglednd" => Self::ToggleDnd,
            "wallpaper" => {
                let arg = rest.ok_or_else(|| {
                    ParseError::MissingArg(name.clone(), "next|prev|random|pause|image path")
                })?;
                Self::Wallpaper(match arg.to_ascii_lowercase().as_str() {
                    "next" => WallpaperAction::Next,
                    "prev" | "previous" => WallpaperAction::Prev,
                    "random" => WallpaperAction::Random,
                    "pause" | "resume" => WallpaperAction::Pause,
                    _ => WallpaperAction::Set(arg.trim_matches('"').to_string()),
                })
            }
            "open" => {
                const EXPECTED: &str = "config|folder|logs";
                let arg = rest.ok_or_else(|| ParseError::MissingArg(name.clone(), EXPECTED))?;
                Self::Open(match arg.to_ascii_lowercase().as_str() {
                    "config" => OpenTarget::Config,
                    "folder" => OpenTarget::Folder,
                    "logs" | "log" => OpenTarget::Logs,
                    _ => return Err(ParseError::BadArg(name, arg.into(), EXPECTED)),
                })
            }
            "exit" => Self::Exit,
            _ => return Err(ParseError::Unknown(name)),
        })
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Left => "l",
            Self::Right => "r",
            Self::Up => "u",
            Self::Down => "d",
        })
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exec(cmd) => write!(f, "exec {cmd}"),
            Self::ExecHidden(cmd) => write!(f, "exechidden {cmd}"),
            Self::FocusOrExec(cmd) => write!(f, "focusorexec {cmd}"),
            Self::KillActive => f.write_str("killactive"),
            Self::ToggleFloating => f.write_str("togglefloating"),
            Self::Fullscreen => f.write_str("fullscreen"),
            Self::MoveFocus(d) => write!(f, "movefocus {d}"),
            Self::MoveWindow(d) => write!(f, "movewindow {d}"),
            Self::ResizeActive(dx, dy) => write!(f, "resizeactive {dx} {dy}"),
            Self::SwapWithMaster => f.write_str("swapwithmaster"),
            Self::CycleNext => f.write_str("cyclenext"),
            Self::CyclePrev => f.write_str("cycleprev"),
            Self::ToggleSplit => f.write_str("togglesplit"),
            Self::Workspace(n) => write!(f, "workspace {n}"),
            Self::MoveToWorkspace(n) => write!(f, "movetoworkspace {n}"),
            Self::MoveToWorkspaceSilent(n) => write!(f, "movetoworkspacesilent {n}"),
            Self::FocusMonitor(d) => write!(f, "focusmonitor {d}"),
            Self::Layout(kind) => write!(f, "layout {kind}"),
            Self::CycleLayout => f.write_str("cyclelayout"),
            Self::ResetLayout => f.write_str("resetlayout"),
            Self::ScrollerProportion(p) => write!(f, "scroller_proportion {p}"),
            Self::CycleProportion => f.write_str("cycleproportion"),
            Self::ConsumeWindow(d) => write!(f, "consumewindow {d}"),
            Self::ExpelWindow => f.write_str("expelwindow"),
            Self::CenterColumn => f.write_str("centercolumn"),
            Self::Desktop(t) => write!(f, "desktop {t}"),
            Self::MoveToDesktop(t) => write!(f, "movetodesktop {t}"),
            Self::Pin => f.write_str("pin"),
            Self::Lock => f.write_str("lock"),
            Self::Launcher => f.write_str("launcher"),
            Self::Reload => f.write_str("reload"),
            Self::TogglePause => f.write_str("togglepause"),
            Self::ToggleTile => f.write_str("toggletile"),
            Self::ToggleRemap => f.write_str("toggleremap"),
            Self::ToggleShell => f.write_str("toggleshell"),
            Self::ToggleLog => f.write_str("togglelog"),
            Self::ToggleDnd => f.write_str("togglednd"),
            Self::Wallpaper(action) => write!(f, "wallpaper {action}"),
            Self::Open(target) => write!(f, "open {target}"),
            Self::Exit => f.write_str("exit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        for input in [
            "exec wt.exe -p \"PowerShell\"",
            "exechidden ecc toggle",
            "focusorexec code.cmd",
            "killactive",
            "movefocus l",
            "resizeactive -40 0",
            "workspace 3",
            "movetoworkspacesilent 9",
            "layout scrolling",
            "scroller_proportion 0.5",
            "consumewindow r",
            "desktop 2",
            "movetodesktop next",
            "resetlayout",
            "pin",
            "lock",
            "launcher",
            "cyclenext",
            "togglednd",
            "wallpaper next",
            "wallpaper prev",
            "wallpaper pause",
            r"wallpaper C:\Pictures\pic ture.png",
            "open logs",
        ] {
            let cmd: Command = input.parse().unwrap();
            assert_eq!(cmd.to_string(), input);
        }
    }

    #[test]
    fn dispatcher_list_matches_the_parser() {
        for (name, args) in DISPATCHERS {
            let parsed = name.parse::<Command>();
            if args.is_empty() {
                assert!(parsed.is_ok(), "{name}");
            } else {
                assert!(matches!(parsed, Err(ParseError::MissingArg(..))), "{name}");
            }
        }
    }

    #[test]
    fn accepts_long_directions_and_case() {
        assert_eq!(
            "MoveFocus Left".parse(),
            Ok(Command::MoveFocus(Direction::Left))
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!("".parse::<Command>(), Err(ParseError::Empty)));
        assert!(matches!(
            "nope".parse::<Command>(),
            Err(ParseError::Unknown(_))
        ));
        assert!(matches!(
            "workspace 0".parse::<Command>(),
            Err(ParseError::BadArg(..))
        ));
        assert!(matches!(
            "movefocus".parse::<Command>(),
            Err(ParseError::MissingArg(..))
        ));
        assert!(matches!(
            "resizeactive 1".parse::<Command>(),
            Err(ParseError::BadArg(..))
        ));
    }
}
