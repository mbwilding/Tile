//! YAML configuration for Tile: schema, keybind parsing, window rules, key remapping and hot
//! reload.

mod keys;
mod notification_filter;
mod rules;
mod schema;
mod watch;

use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub use keys::{Keybind, Modifiers};
pub use notification_filter::{FilterOutcome, NotificationFilters};
pub use rules::{RuleOutcome, Rules};
pub(crate) use schema::default_binds;
pub use schema::{
    Animations, Color, Config, Corners, Cursor, Cycle, CycleOrder, Features, FillMode, General,
    IndicatorPosition, IndicatorStyle, Launcher, LauncherStyle, NotificationFilter,
    NotificationStyle, Notifications, Remap, Rule, SearchPosition, Theme, Wallpaper,
    WorkspaceIndicator,
};
pub use watch::watch;

use tile_core::Command;

/// Why a config didn't load.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file couldn't be read.
    #[error("failed to read {0}: {1}")]
    Io(PathBuf, io::Error),
    /// The YAML is malformed or doesn't match the schema.
    #[error("invalid config: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    /// A bind's keys or command is invalid.
    #[error("bind `{0}`: {1}")]
    Bind(String, String),
    /// A window rule's pattern is invalid.
    #[error("rule #{0}: {1}")]
    Rule(usize, String),
    /// A `notifications.filter` entry's pattern is invalid.
    #[error("notifications.filter #{0}: {1}")]
    NotificationFilter(usize, String),
    /// The remap layers have errors, every one listed.
    #[error("remap: {}", .0.join("; "))]
    Remap(Vec<String>),
    /// A `launcher.hide` pattern is invalid.
    #[error("launcher.hide: {0}")]
    LauncherHide(String),
}

impl ConfigError {
    /// A one-line form for the features: serde's list of every expected field is cut, and the line
    /// number is kept.
    pub fn summary(&self) -> String {
        match self {
            Self::Yaml(e) => {
                let text = e.to_string();
                let message = text.split(", expected").next().unwrap_or(&text);
                let message = message
                    .rsplit_once(" at line ")
                    .map_or(message, |(message, _)| message);
                match e.location() {
                    Some(at) => format!("{message} (line {})", at.line()),
                    None => message.to_owned(),
                }
            }
            Self::Remap(errors) => match errors.as_slice() {
                [only] => format!("remap: {only}"),
                [first, rest @ ..] => format!("remap: {first} (+{} more)", rest.len()),
                [] => "remap".to_owned(),
            },
            other => other.to_string(),
        }
    }
}

/// A validated configuration, ready to use.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The settings, as written with the defaults filled in.
    pub config: Config,
    /// The default binds (unless switched off) with `binds` applied over them.
    pub binds: Vec<(Keybind, Command)>,
    /// `rules`, compiled.
    pub rules: Rules,
    /// `notifications.filter`, compiled.
    pub notification_filters: NotificationFilters,
    /// `remap`, compiled for the remapping engine.
    pub remap: tile_remap::Config,
    /// `launcher.hide`, compiled.
    pub launcher_hide: regex::RegexSet,
}

impl Loaded {
    /// Parses and validates config text. An empty document is the defaults.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        // An empty document (or one of only comments) is YAML's null, not an empty map.
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)?;
        if value.is_null() {
            return Self::from_config(Config::default());
        }
        Self::from_config(serde_yaml_ng::from_str(text)?)
    }

    /// The built-in defaults, used when no config file exists.
    pub fn defaults() -> Self {
        Self::from_config(Config::default()).expect("built-in defaults are valid")
    }

    /// Loads `path`, or the built-in defaults when the file doesn't exist.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::defaults())
        }
    }

    fn from_config(config: Config) -> Result<Self, ConfigError> {
        let defaults = if config.general.default_binds {
            default_binds()
                .into_iter()
                .map(|(k, v)| (k, Some(v)))
                .collect()
        } else {
            Vec::new()
        };
        let mut binds: Vec<(Keybind, Command)> = Vec::new();
        // Matched by the parsed key, so `win+h` overrides the default `super+h`.
        for (keys, action) in defaults.into_iter().chain(config.binds.clone()) {
            let bind = keys
                .parse::<Keybind>()
                .map_err(|e| ConfigError::Bind(keys.clone(), e))?;
            binds.retain(|(b, _)| *b != bind);
            let Some(action) = action.filter(|a| !a.trim().is_empty()) else {
                continue;
            };
            let command = action
                .parse::<Command>()
                .map_err(|e| ConfigError::Bind(keys.clone(), e.to_string()))?;
            binds.push((bind, command));
        }
        let rules = Rules::compile(&config.rules)?;
        let notification_filters = NotificationFilters::compile(&config.notifications.filter)?;
        let remap = tile_remap::Config::compile(&config.remap.layers, config.remap.modifier_guard)
            .map_err(ConfigError::Remap)?;
        let launcher_hide = regex::RegexSetBuilder::new(&config.launcher.hide)
            .case_insensitive(true)
            .build()
            .map_err(|e| ConfigError::LauncherHide(e.to_string()))?;
        Ok(Self {
            config,
            binds,
            rules,
            notification_filters,
            remap,
            launcher_hide,
        })
    }

    /// Loads and validates the config file at `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|e| ConfigError::Io(path.into(), e))?;
        Self::parse(&text)
    }
}

/// `%APPDATA%\Tile\config.yml`, or `config.yaml` when only that exists. Created on first run
/// with only the feature switches in it (see [`create_if_missing`]); everything else comes from the
/// built-in defaults.
pub fn default_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Tile");
    let yml = dir.join("config.yml");
    let yaml = dir.join("config.yaml");
    if !yml.exists() && yaml.exists() {
        yaml
    } else {
        yml
    }
}

const NEW_FILE_HEADER: &str = "\
---
# Tile config. Only the feature switches are saved here; everything else uses the built-in
# defaults. To change one, add its section and key. docs/configuration.md lists them all,
# with the whole default config to copy. Saved changes are applied live.

";

/// Sets `key: value` for each pair in the section at `path` of `text` (like `["general",
/// "layout", "monitors"]`), keeping everything else (comments, other keys, order). Missing keys
/// are added, and so are missing sections.
fn set_values<V: std::fmt::Display>(text: &str, path: &[&str], pairs: &[(&str, V)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    // Comments at the left edge head whatever section follows, so they don't count.
    let is_text = |l: &str| !l.trim().is_empty() && !l.starts_with('#');
    let content = |l: &str| l.split('#').next().unwrap_or("").trim_end().to_owned();
    // Where a new line goes in `lo..hi`: after the last line of text, or at the start.
    let after_text = |lines: &[String], lo: usize, hi: usize| {
        (lo..hi)
            .rev()
            .find(|&i| is_text(&lines[i]))
            .map_or(lo, |i| i + 1)
    };
    // The indentation of the lines in `lo..hi`, or two more than `parent` if there are none.
    let indent_in = |lines: &[String], lo: usize, hi: usize, parent: &str| {
        (lo..hi)
            .map(|i| &lines[i])
            .find(|l| is_text(l))
            .map_or_else(|| format!("{parent}  "), |l| l[..indent_of(l)].to_owned())
    };

    let (mut lo, mut hi) = (0, lines.len());
    let mut indent = String::new();
    for (depth, name) in path.iter().enumerate() {
        let header = format!("{indent}{name}:");
        let found = (lo..hi).find(|&i| {
            is_text(&lines[i])
                && indent_of(&lines[i]) == indent.len()
                && content(&lines[i]) == header
        });
        let at = if let Some(i) = found {
            i
        } else {
            let mut pos = if depth == 0 {
                lines.len()
            } else {
                after_text(&lines, lo, hi)
            };
            if depth == 0 && lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
                pos += 1;
            }
            lines.insert(pos, header);
            hi = if depth == 0 { lines.len() } else { hi + 1 };
            pos
        };
        let base = indent_of(&lines[at]);
        let end = (at + 1..hi)
            .find(|&i| is_text(&lines[i]) && indent_of(&lines[i]) <= base)
            .unwrap_or(hi);
        lo = at + 1;
        hi = end;
        indent = indent_in(&lines, lo, hi, &indent);
    }
    for (key, value) in pairs {
        let existing = (lo..hi).find(|&i| {
            indent_of(&lines[i]) == indent.len()
                && lines[i]
                    .trim_start()
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.starts_with(':'))
        });
        match existing {
            Some(i) => {
                // Keep a trailing comment.
                let comment = lines[i].find(" #").map(|c| lines[i][c + 1..].to_owned());
                lines[i] = match comment {
                    Some(c) => format!("{indent}{key}: {value}  {c}"),
                    None => format!("{indent}{key}: {value}"),
                };
            }
            None => {
                let at = after_text(&lines, lo, hi);
                lines.insert(at, format!("{indent}{key}: {value}"));
                hi += 1;
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn feature_pairs(features: &Features) -> [(&'static str, bool); 4] {
    [
        ("tile", features.tile),
        ("remap", features.remap),
        ("shell", features.shell),
        ("log", features.log),
    ]
}

/// Creates the config file, and its folder, if there is none, holding only the feature switches at
/// their defaults: everything else comes from the built-in defaults.
pub fn create_if_missing(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = set_values(
        NEW_FILE_HEADER,
        &["features"],
        &feature_pairs(&Features::default()),
    );
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        // Don't leave a partial file behind: it would stop the file being written again.
        Ok(mut file) => io::Write::write_all(&mut file, text.as_bytes()).inspect_err(|_| {
            drop(file);
            let _ = fs::remove_file(path);
        }),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// The name a monitor has under `general.layout.monitors:`, from its hardware path (see
/// `tile_platform::monitor_paths`): its model and the port it is on, like `DEL4146-4352`. The
/// model stays with the monitor and the port tells identical ones apart, unlike Windows'
/// `DISPLAYn` numbers, which change. `None` for a path that isn't a monitor's.
pub fn monitor_key(path: &str) -> Option<String> {
    let mut parts = path.trim_start_matches(['\\', '?']).split('#');
    if !parts.next()?.eq_ignore_ascii_case("DISPLAY") {
        return None;
    }
    let model = parts.next().filter(|m| !m.is_empty())?;
    let port = parts
        .next()
        .and_then(|instance| instance.rsplit_once("UID"))
        .map(|(_, uid)| uid.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|uid| !uid.is_empty());
    Some(match port {
        Some(port) => format!("{model}-{port}"),
        None => model.to_owned(),
    })
}

/// Saves `layout` as the last used on `monitor` (a [`monitor_key`]) in `general.layout.monitors:` of the config
/// file, leaving the rest of it as it is.
pub fn write_layout(path: &Path, monitor: &str, layout: tile_core::LayoutKind) -> io::Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::from(NEW_FILE_HEADER),
        Err(e) => return Err(e),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(
        path,
        set_values(
            &text,
            &["general", "layout", "monitors"],
            &[(monitor, layout)],
        ),
    )
}

/// Saves the feature switches that differ between `before` and `after` into `features:` of the config
/// file, leaving the rest of it, the other switches included, as it is. That keeps the file's
/// switches even when it doesn't load and `before` holds the defaults.
pub fn write_features(path: &Path, before: &Features, after: &Features) -> io::Result<()> {
    let changed: Vec<_> = feature_pairs(after)
        .into_iter()
        .zip(feature_pairs(before))
        .filter(|(new, old)| new != old)
        .map(|(new, _)| new)
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::from(NEW_FILE_HEADER),
        Err(e) => return Err(e),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, set_values(&text, &["features"], &changed))
}

#[cfg(test)]
mod tests {
    #[test]
    fn yaml_error_summary_is_one_short_line() {
        let Err(e) = Loaded::parse(
            "general:
  tilin: true
",
        ) else {
            panic!("an unknown field should not load");
        };
        let summary = e.summary();
        assert!(summary.contains("unknown field `tilin`"), "{summary}");
        assert!(summary.ends_with("(line 2)"), "{summary}");
        assert!(!summary.contains("expected"), "{summary}");
        assert!(summary.len() < 80, "{summary}");
    }

    use super::*;

    #[test]
    fn placeholder_follows_the_settings() {
        let mut launcher = Launcher::default();
        let mut check = |actions: bool, calculator: bool, commands: bool, expected: &str| {
            launcher.system_items = actions;
            launcher.calculator = calculator;
            launcher.commands = commands;
            assert_eq!(launcher.placeholder_text(), expected);
        };
        check(
            true,
            true,
            true,
            "Search apps, actions, equations, conversions and commands",
        );
        check(
            true,
            true,
            false,
            "Search apps, actions, equations and conversions",
        );
        check(false, true, false, "Search apps, equations and conversions");
        check(false, false, true, "Search apps and commands");
        check(true, false, false, "Search apps and actions");
        check(false, false, false, "Search apps");

        let custom = Launcher {
            placeholder: "Find".into(),
            ..Launcher::default()
        };
        assert_eq!(custom.placeholder_text(), "Find");
    }

    #[test]
    fn built_in_defaults_are_valid() {
        let loaded = Loaded::defaults();
        assert!(loaded.binds.len() > 40);
        assert_eq!(loaded.config, Config::default());
    }

    #[test]
    fn empty_config_uses_defaults() {
        for text in ["", "# only a comment\n"] {
            assert_eq!(Loaded::parse(text).unwrap().config, Config::default());
        }
    }

    #[test]
    fn missing_file_uses_defaults() {
        let loaded = Loaded::load_or_default(Path::new("does/not/exist.yml")).unwrap();
        assert_eq!(loaded.config, Config::default());
    }

    #[test]
    fn new_file_holds_only_the_features_and_runs_on_defaults() {
        let dir = std::env::temp_dir().join(format!("tile-config-{}", std::process::id()));
        let path = dir.join("config.yml");
        create_if_missing(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("features:") && text.contains("  remap: false"));
        assert_eq!(Loaded::parse(&text).unwrap().config, Config::default());
        let features = Features {
            tile: true,
            ..Features::default()
        };
        write_features(&path, &Features::default(), &features).unwrap();
        assert_eq!(Loaded::load(&path).unwrap().config.features, features);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn writing_a_switch_leaves_the_others_as_the_file_has_them() {
        let dir = std::env::temp_dir().join(format!("tile-config-tray-{}", std::process::id()));
        let path = dir.join("config.yml");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &path,
            "general:\n  tilin: true\nfeatures:\n  tile: true\n  log: true\n",
        )
        .unwrap();
        let before = Features::default();
        let after = Features {
            remap: true,
            ..before.clone()
        };
        write_features(&path, &before, &after).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("  tile: true\n  log: true\n  remap: true"),
            "{text}"
        );
        assert!(text.contains("tilin"), "{text}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_key_goes_before_the_comments_of_the_next_section() {
        let text = "monitors:
  A: even

# theme:
#   font: X

binds: {}
";
        let out = set_values(text, &["monitors"], &[("B", "master")]);
        assert_eq!(
            out,
            "monitors:
  A: even
  B: master

# theme:
#   font: X

binds: {}
"
        );
    }

    #[test]
    fn nested_sections_are_found_or_made() {
        let text = "general:
  gaps_in: 2
  layout:
    default: master  # mine
    monitors:
      A: even
  gaps_out: 3

binds: {}
";
        let path = &["general", "layout", "monitors"];
        let out = set_values(text, path, &[("A", "scrolling"), ("B", "master")]);
        assert_eq!(
            out,
            "general:
  gaps_in: 2
  layout:
    default: master  # mine
    monitors:
      A: scrolling
      B: master
  gaps_out: 3

binds: {}
"
        );
        let out = set_values(
            "general:
  gaps_in: 2

binds: {}
",
            path,
            &[("A", "even")],
        );
        assert_eq!(
            out,
            "general:
  gaps_in: 2
  layout:
    monitors:
      A: even

binds: {}
"
        );
        let out = set_values("", path, &[("A", "even")]);
        assert_eq!(
            out,
            "general:
  layout:
    monitors:
      A: even
"
        );
    }

    #[test]
    fn set_values_edits_in_place() {
        let text = "general:\n  gaps_in: 2\n\nfeatures:  # switches\n    tile: false  # on\n\ncursor:\n  warp: false\n";
        let out = set_values(text, &["features"], &[("tile", true), ("remap", true)]);
        assert_eq!(
            out,
            "general:\n  gaps_in: 2\n\nfeatures:  # switches\n    tile: true  # on\n    remap: true\n\ncursor:\n  warp: false\n"
        );
        let out = set_values(
            "general:\n  tiling: true\n",
            &["features"],
            &[("tile", true)],
        );
        assert_eq!(out, "general:\n  tiling: true\n\nfeatures:\n  tile: true\n");
    }

    #[test]
    fn a_monitor_keeps_its_last_layout_in_the_file() {
        assert_eq!(
            monitor_key(
                r"\\?\DISPLAY#DEL4146#5&2a3c91f&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            ),
            Some("DEL4146-4352".into())
        );
        assert_eq!(
            monitor_key(r"\\?\DISPLAY#GSM5B09#4&1e2f&0&UID0#{x}"),
            Some("GSM5B09-0".into())
        );
        assert_eq!(
            monitor_key(r"\\?\DISPLAY#AOC2790#5&1&0&0#{x}"),
            Some("AOC2790".into())
        );
        assert_eq!(monitor_key(r"\\.\DISPLAY1"), None);
        assert_eq!(monitor_key(""), None);
        let dir = std::env::temp_dir().join(format!("tile-config-monitors-{}", std::process::id()));
        let path = dir.join("config.yml");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &path,
            "# mine
general:
  gaps_in: 2
",
        )
        .unwrap();
        write_layout(&path, "DEL4146-4352", tile_core::LayoutKind::Scrolling).unwrap();
        write_layout(&path, "DEL4146-4352", tile_core::LayoutKind::Even).unwrap();
        write_layout(&path, "GSM5B09-0", tile_core::LayoutKind::Master).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(
                "# mine
general:
  gaps_in: 2
"
            ),
            "{text}"
        );
        let config = Loaded::parse(&text).unwrap().config;
        assert_eq!(
            config.general.layout.monitors["DEL4146-4352"],
            tile_core::LayoutKind::Even
        );
        assert_eq!(
            config.general.layout.monitors["GSM5B09-0"],
            tile_core::LayoutKind::Master
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_section_keeps_other_defaults() {
        let loaded = Loaded::parse("general:\n  gaps_in: 2\n").unwrap();
        assert_eq!(loaded.config.general.gaps_in, 2);
        assert_eq!(loaded.config.general.gaps_out, General::default().gaps_out);
    }

    #[test]
    fn theme_fills_in_what_sections_leave_unset() {
        let text = "theme:\n  font: Iosevka\n  accent: \"#ff0000\"\n\
                    launcher:\n  font: Consolas\n\
                    workspace_indicator:\n  background: \"#00ff00\"\n";
        let config = Loaded::parse(text).unwrap().config;
        let red = Color::rgb(0xff, 0, 0);
        assert_eq!(config.border_active(), red);
        let launcher = config.launcher_style();
        assert_eq!(launcher.font, "Consolas");
        assert_eq!((launcher.border, launcher.selected), (red, red));
        let indicator = config.indicator_style();
        assert_eq!(indicator.font, "Iosevka");
        assert_eq!(indicator.background, Color::rgb(0, 0xff, 0));
    }

    #[test]
    fn reports_bad_binds() {
        let err = Loaded::parse("binds:\n  super+q: explode\n").unwrap_err();
        assert!(err.to_string().contains("super+q"));
        assert!(Loaded::parse("binds:\n  hyper+q: killactive\n").is_err());
    }

    #[test]
    fn no_built_in_remap_layers() {
        let loaded = Loaded::defaults();
        assert!(loaded.config.general.tiling && loaded.config.general.remap);
        assert!(loaded.config.remap.layers.is_empty());
    }

    #[test]
    fn remap_layers_replace_the_defaults() {
        assert!(Loaded::defaults().remap.is_empty());
        let loaded = Loaded::parse("remap:\n  layers:\n    main:\n      capslock: esc\n").unwrap();
        assert!(!loaded.remap.is_empty());
        let err = Loaded::parse("remap:\n  layers:\n    main:\n      capslock: layer(nav)\n")
            .unwrap_err();
        assert!(err.to_string().contains("unknown layer `nav`"), "{err}");
    }

    #[test]
    fn win_l_bind_loads_without_override() {
        // The daemon skips it instead, so the default binds stay valid either way.
        let config = "general:\n  override_win_l: false\n  default_binds: false\nbinds:\n  super+l: movefocus r\n";
        assert!(Loaded::parse(config).unwrap().binds[0].0.is_win_l());
    }

    fn command(loaded: &Loaded, keys: &str) -> Option<String> {
        let bind = keys.parse::<Keybind>().unwrap();
        loaded
            .binds
            .iter()
            .find(|(b, _)| *b == bind)
            .map(|(_, c)| c.to_string())
    }

    #[test]
    fn binds_merge_over_the_defaults() {
        let defaults = Loaded::defaults();
        assert_eq!(defaults.binds.len(), default_binds().len());
        assert_eq!(command(&defaults, "super+q").as_deref(), Some("killactive"));

        let config = "binds:\n  win+t: exec wtd.exe -w new\n  super+h: ''\n  super+x: focusorexec code.cmd\n";
        let loaded = Loaded::parse(config).unwrap();
        assert_eq!(
            command(&loaded, "super+t").as_deref(),
            Some("exec wtd.exe -w new")
        );
        assert_eq!(command(&loaded, "super+h"), None);
        assert_eq!(
            command(&loaded, "super+x").as_deref(),
            Some("focusorexec code.cmd")
        );
        assert_eq!(command(&loaded, "super+q").as_deref(), Some("killactive"));
        assert_eq!(loaded.binds.len(), default_binds().len());

        let unbound = Loaded::parse("binds:\n  super+q: ~\n").unwrap();
        assert_eq!(command(&unbound, "super+q"), None);
    }

    #[test]
    fn default_binds_can_be_switched_off() {
        let config = "general:\n  default_binds: false\nbinds:\n  super+q: killactive\n";
        let loaded = Loaded::parse(config).unwrap();
        assert_eq!(loaded.binds.len(), 1);
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(Loaded::parse("general:\n  gap: 2\n").is_err());
    }
}
