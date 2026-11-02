//! What the launcher shows: the search, its results and what picking one does.

use std::{borrow::Cow, path::PathBuf};

use tile_shell::notify::{Click, IconImage, Registry, TrayItem};
use tokio::sync::mpsc::UnboundedSender;
use winsafe as w;

use super::{
    LauncherEvent, LauncherSettings,
    layout::{FontSizes, Layout},
    popups::MenuWatch,
};
use crate::{
    calc,
    input::Input,
    search::{self, App, CommandEntry, RankOptions, Usage},
    system::{self, Group},
};

/// A result row.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Row {
    /// An index into all the apps.
    App(usize),
    Command(CommandEntry),
    /// The value of the search as a calculation.
    Calc(f64),
    /// An index into the system items.
    System(usize),
    /// An index into the tray icons.
    Tray(usize),
}

/// A row's picture.
pub(super) enum RowIcon<'a> {
    /// The app with this `shell:AppsFolder` id.
    App(&'a str),
    Tray(&'a IconImage),
    /// A character of Windows' icon font, for a row with no picture of its own.
    Glyph(char),
}

/// The glyph of a row whose picture is missing: a generic app.
pub(super) const DEFAULT_GLYPH: char = '\u{ECAA}';
/// The glyphs of a command line to run, a Tile dispatcher and a calculation.
const RUN_GLYPH: char = '\u{E756}';
const DISPATCH_GLYPH: char = '\u{E765}';
const CALC_GLYPH: char = '\u{E8EF}';

/// What picking a row did.
pub(super) enum Picked {
    /// Close, then send this to the daemon (after, so a dispatcher acts on the window
    /// that had focus before the launcher).
    Done(LauncherEvent),
    /// Changed the search (completed a dispatcher name): redraw.
    Edited,
    /// Copy this to the clipboard, then close.
    Copy(String),
    Nothing,
}

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Hit {
    /// A tray icon of the strip.
    Tray(usize),
    /// A button of the bar.
    Bar(usize),
    /// A result row.
    Row(usize),
}

pub(super) struct State {
    pub(super) settings: LauncherSettings,
    pub(super) events: UnboundedSender<LauncherEvent>,
    /// Everything listed, before `hide`.
    pub(super) all: Vec<App>,
    /// Indices into `all` of the apps `hide` leaves.
    pub(super) shown: Vec<usize>,
    pub(super) loading: bool,
    pub(super) usage: Usage,
    pub(super) usage_path: PathBuf,
    pub(super) query: Input,
    /// A UTF-16 high surrogate waiting for its pair.
    pub(super) surrogate: Option<u16>,
    pub(super) results: Vec<Row>,
    pub(super) selected: usize,
    pub(super) scroll: usize,
    pub(super) dpi: u32,
    /// Where the pointer was first seen since opening; moves from there select rows.
    pub(super) mouse: Option<(i32, i32)>,
    /// The notification area's icons.
    pub(super) registry: Registry,
    /// The tray icons when the launcher opened.
    pub(super) tray: Vec<TrayItem>,
    /// The tray icon the pointer is over.
    pub(super) tray_hover: Option<usize>,
    /// The bar button the pointer is over.
    pub(super) bar_hover: Option<usize>,
    /// The group of controls shown in place of the results, while one is open.
    pub(super) panel: Option<Group>,
    /// What the system was doing when the launcher opened.
    pub(super) status: system::Status,
    /// The system rows matching the search.
    pub(super) system: Vec<system::Entry>,
    /// While a tray icon's menu is up: the launcher stays open behind it.
    pub(super) menu: Option<MenuWatch>,
}

impl State {
    pub(super) fn new(
        settings: LauncherSettings,
        events: UnboundedSender<LauncherEvent>,
        usage_path: PathBuf,
        registry: Registry,
    ) -> Self {
        Self {
            settings,
            events,
            all: Vec::new(),
            shown: Vec::new(),
            loading: true,
            usage: Usage::load(&usage_path),
            usage_path,
            query: Input::default(),
            surrogate: None,
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            dpi: 96,
            mouse: None,
            registry,
            tray: Vec::new(),
            tray_hover: None,
            bar_hover: None,
            panel: None,
            status: system::Status::default(),
            system: Vec::new(),
            menu: None,
        }
    }

    pub(super) fn config(&self) -> &tile_config::Launcher {
        &self.settings.config
    }

    pub(super) fn font_sizes(&self) -> FontSizes {
        FontSizes::new(self.config().font_size, self.dpi)
    }

    pub(super) fn filter(&mut self) {
        let hide = &self.settings.hide;
        self.shown = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, a)| !hide.is_match(&a.name) && !hide.is_match(&a.id))
            .map(|(i, _)| i)
            .collect();
        self.rank();
    }

    /// Back to what a freshly opened launcher shows.
    pub(super) fn reset(&mut self) {
        self.query.clear();
        self.surrogate = None;
        self.mouse = None;
        self.tray_hover = None;
        self.bar_hover = None;
        self.panel = None;
        self.menu = None;
        self.rank();
    }

    pub(super) fn rank(&mut self) {
        let config = &self.settings.config;
        let query = self.query.text();
        self.selected = 0;
        self.scroll = 0;
        if let Some(group) = self.panel {
            self.system = system::find_group(group, query, &self.status);
            self.results = (0..self.system.len()).map(Row::System).collect();
            return;
        }
        let options = RankOptions {
            pinned: &config.pinned,
            sort_by_usage: config.sort_by_usage,
            max_results: config.max_results,
        };
        let searching = !query.trim().is_empty();
        let sum = config
            .calculator
            .then(|| calc::calculation(query))
            .flatten();
        let mut results: Vec<Row> = sum
            .map(Row::Calc)
            .into_iter()
            .chain(
                search::rank(query, &self.all, &self.shown, &self.usage, &options)
                    .into_iter()
                    .map(Row::App),
            )
            .collect();
        self.system.clear();
        if searching && config.system_items {
            // An app that is listed already stands for its own system row or tray icon.
            let listed = |name: &str| {
                results.iter().any(|row| match row {
                    Row::App(i) => {
                        let app = self.all[*i].name.to_lowercase();
                        let name = name.to_lowercase();
                        name == app || name.starts_with(&format!("{app} "))
                    }
                    _ => false,
                })
            };
            let mut system = system::find(query, &self.status);
            system.retain(|e| !listed(&e.title));
            let tray: Vec<Row> = self
                .tray_rows(query)
                .filter(|row| match row {
                    Row::Tray(i) => {
                        let item = &self.tray[*i];
                        !listed(&item.name) && !self.runs_listed_app(&item.path, &results)
                    }
                    _ => true,
                })
                .collect();
            self.system = system;
            results.extend((0..self.system.len()).map(Row::System));
            results.extend(tray);
        }
        if searching && config.commands {
            let mut rows = search::commands(query);
            // Running the text as a command line is offered when it reads like one, or when
            // nothing else matched.
            let shell = rows.pop();
            results.extend(rows.into_iter().map(Row::Command));
            if let Some(shell) = shell {
                results.push(Row::Command(shell));
            }
        }
        self.results = results;
    }

    /// Whether one of the app `results` is the program at `path`.
    fn runs_listed_app(&self, path: &str, results: &[Row]) -> bool {
        !path.is_empty()
            && results
                .iter()
                .any(|row| matches!(row, Row::App(i) if self.all[*i].id.eq_ignore_ascii_case(path)))
    }

    /// The rows for the tray icons matching `query`, best first.
    fn tray_rows(&self, query: &str) -> impl Iterator<Item = Row> + use<> {
        let query = query.trim();
        let mut tray: Vec<(f64, usize)> = self
            .tray
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((search::fuzzy(query, &item.name)?, i)))
            .collect();
        tray.sort_by(|a, b| b.0.total_cmp(&a.0));
        tray.into_iter().map(|(_, i)| Row::Tray(i))
    }

    /// The most rows shown at once.
    pub(super) fn max_rows(&self) -> usize {
        self.config().max_visible.clamp(1, 30)
    }

    /// The height of a `width` pixel wide window that fits everything but the list exactly, and
    /// `rows` rows of it.
    pub(super) fn height_for(&self, width: i32, rows: usize) -> i32 {
        const TALL: i32 = 20_000;
        let layout = self.layout(w::RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: TALL,
        });
        let list = TALL - (layout.list.bottom - layout.list.top) + rows as i32 * layout.row;
        // The sidebar may be taller than the search column; the list then shows more rows.
        list.max(layout.side_end + layout.pad)
    }

    /// The height that fits the results now: as many rows as there are (at least one, for the
    /// message that there are none), up to the most shown.
    pub(super) fn wanted_height(&self, width: i32) -> i32 {
        let rows = self.results.len().clamp(1, self.max_rows());
        self.height_for(width, rows)
    }

    pub(super) fn layout(&self, client: w::RECT) -> Layout {
        Layout::new(
            self.config(),
            self.font_sizes(),
            self.dpi,
            self.tray.len(),
            client,
        )
    }

    /// What is at a point.
    pub(super) fn hit(&self, layout: &Layout, x: i32, y: i32) -> Option<Hit> {
        layout
            .tray_at(x, y)
            .map(Hit::Tray)
            .or_else(|| layout.bar_button_at(x, y).map(Hit::Bar))
            .or_else(|| {
                layout
                    .row_at(self.scroll, self.results.len(), x, y)
                    .map(Hit::Row)
            })
    }

    pub(super) fn select(&mut self, index: isize, visible: usize) {
        if self.results.is_empty() {
            return;
        }
        let last = self.results.len() as isize - 1;
        self.selected = index.clamp(0, last) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    pub(super) fn scroll_by(&mut self, rows: isize, visible: usize) {
        let max = self.results.len().saturating_sub(visible) as isize;
        self.scroll = (self.scroll as isize + rows).clamp(0, max) as usize;
    }

    /// Picks the selected row: what to ask the daemon (noting an app launch), or completes a
    /// dispatcher's name. `secondary` (Shift+Enter, the right button's menu) is a right click on a
    /// tray icon, or starting an app as administrator.
    pub(super) fn pick(&mut self, secondary: bool) -> Picked {
        let event = match self.results.get(self.selected) {
            None => return Picked::Nothing,
            Some(Row::Tray(i)) => {
                let Some(item) = self.tray.get(*i) else {
                    return Picked::Nothing;
                };
                let click = if secondary { Click::Right } else { Click::Left };
                LauncherEvent::Tray {
                    key: item.key,
                    click,
                    at: cursor(),
                }
            }
            Some(Row::System(i)) => {
                let Some(entry) = self.system.get(*i) else {
                    return Picked::Nothing;
                };
                LauncherEvent::System(entry.action.clone())
            }
            Some(Row::Calc(value)) => return Picked::Copy(calc::format(*value)),
            Some(Row::App(i)) => {
                let id = self.all[*i].id.clone();
                if self.config().track_usage {
                    self.usage.record(&id);
                    self.usage.save(&self.usage_path);
                }
                if secondary {
                    LauncherEvent::LaunchAsAdmin(id)
                } else {
                    LauncherEvent::Launch(id)
                }
            }
            Some(Row::Command(CommandEntry::Dispatch(command))) => {
                LauncherEvent::Dispatch(command.clone())
            }
            Some(Row::Command(CommandEntry::Name(name))) => match name.parse() {
                Ok(command) => LauncherEvent::Dispatch(command),
                Err(_) => return Picked::Nothing,
            },
            Some(Row::Command(CommandEntry::Invalid { .. })) => return Picked::Nothing,
            Some(Row::Command(CommandEntry::Shell(line))) => LauncherEvent::Run(line.clone()),
            Some(Row::Command(CommandEntry::Complete { name, .. })) => {
                let text = format!("{name} ");
                self.query.set(text);
                self.rank();
                return Picked::Edited;
            }
        };
        Picked::Done(event)
    }

    /// The row's title, subtitle and icon.
    pub(super) fn describe<'a>(
        &'a self,
        row: &'a Row,
    ) -> (Cow<'a, str>, Cow<'a, str>, Option<RowIcon<'a>>) {
        match row {
            Row::App(i) => {
                let app = &self.all[*i];
                (
                    app.name.as_str().into(),
                    app.id.as_str().into(),
                    Some(RowIcon::App(&app.id)),
                )
            }
            Row::Tray(i) => {
                let item = &self.tray[*i];
                (
                    item.name.as_str().into(),
                    "Tray icon. Enter clicks it, Shift+Enter or right click opens its menu".into(),
                    Some(
                        item.icon
                            .as_ref()
                            .map_or(RowIcon::Glyph(DEFAULT_GLYPH), RowIcon::Tray),
                    ),
                )
            }
            Row::System(i) => {
                let entry = &self.system[*i];
                (
                    entry.title.as_str().into(),
                    entry.subtitle.as_str().into(),
                    Some(RowIcon::Glyph(entry.action.glyph())),
                )
            }
            Row::Command(CommandEntry::Dispatch(command)) => (
                command.to_string().into(),
                "Tile dispatcher".into(),
                Some(RowIcon::Glyph(DISPATCH_GLYPH)),
            ),
            Row::Command(CommandEntry::Name(name)) => (
                (*name).into(),
                "Tile dispatcher".into(),
                Some(RowIcon::Glyph(DISPATCH_GLYPH)),
            ),
            Row::Command(CommandEntry::Complete { name, args }) => (
                format!("{name} {args}").into(),
                "Tile dispatcher: Enter to add the argument".into(),
                Some(RowIcon::Glyph(DISPATCH_GLYPH)),
            ),
            Row::Command(CommandEntry::Invalid { name, args, error }) => (
                format!("{name} {args}").trim_end().to_owned().into(),
                format!("Tile dispatcher: {error}").into(),
                Some(RowIcon::Glyph(DISPATCH_GLYPH)),
            ),
            Row::Command(CommandEntry::Shell(line)) => (
                line.as_str().into(),
                "Run command".into(),
                Some(RowIcon::Glyph(RUN_GLYPH)),
            ),
            Row::Calc(value) => (
                format!("= {}", calc::format(*value)).into(),
                "Enter to copy".into(),
                Some(RowIcon::Glyph(CALC_GLYPH)),
            ),
        }
    }
}

/// Where the pointer is, in screen pixels.
pub(super) fn cursor() -> (i32, i32) {
    w::GetCursorPos().map_or((0, 0), |p| (p.x, p.y))
}
