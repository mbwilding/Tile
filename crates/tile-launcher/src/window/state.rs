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
    system::{self, Action, Group, Power},
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
}

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
    /// A power action that needs Enter pressed again.
    pub(super) armed: Option<Power>,
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
            armed: None,
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

    pub(super) fn rank(&mut self) {
        let config = &self.settings.config;
        let query = self.query.text();
        self.armed = None;
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
        let is_sum = sum.is_some();
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
            self.system = system::find(query, &self.status);
            results.extend((0..self.system.len()).map(Row::System));
            results.extend(self.tray_rows(query));
        }
        if searching && config.commands {
            let mut rows = search::commands(query);
            // Running the text as a command line is offered when it reads like one, or when
            // nothing else matched.
            let shell = rows.pop();
            results.extend(rows.into_iter().map(Row::Command));
            if let Some(shell) = shell
                && (results.is_empty() || !is_sum && query.trim().contains(char::is_whitespace))
            {
                results.push(Row::Command(shell));
            }
        }
        self.results = results;
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

    /// The window height that fits everything but the list exactly, and `rows` rows of it.
    pub(super) fn height_for(&self, rows: usize) -> i32 {
        const TALL: i32 = 20_000;
        let layout = self.layout(w::RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: TALL,
        });
        TALL - (layout.list.bottom - layout.list.top) + rows as i32 * layout.row
    }

    /// The height that fits the results now: as many rows as there are (at least one, for the
    /// message that there are none), up to the most shown.
    pub(super) fn wanted_height(&self) -> i32 {
        let rows = self.results.len().clamp(1, self.max_rows());
        self.height_for(rows)
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
            .tray_at(self.tray.len(), x, y)
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
        self.armed = None;
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
                if let Action::Power(power) = entry.action
                    && power.needs_confirmation()
                    && self.armed != Some(power)
                {
                    self.armed = Some(power);
                    return Picked::Edited;
                }
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
                    item.icon.as_ref().map(RowIcon::Tray),
                )
            }
            Row::System(i) => {
                let entry = &self.system[*i];
                let confirming = matches!(entry.action, Action::Power(p) if self.armed == Some(p));
                let subtitle: &str = if confirming {
                    "Press Enter again to confirm"
                } else {
                    &entry.subtitle
                };
                (entry.title.as_str().into(), subtitle.into(), None)
            }
            Row::Command(CommandEntry::Dispatch(command)) => {
                (command.to_string().into(), "Tile dispatcher".into(), None)
            }
            Row::Command(CommandEntry::Name(name)) => {
                ((*name).into(), "Tile dispatcher".into(), None)
            }
            Row::Command(CommandEntry::Complete { name, args }) => (
                format!("{name} {args}").into(),
                "Tile dispatcher: Enter to add the argument".into(),
                None,
            ),
            Row::Command(CommandEntry::Invalid { name, args, error }) => (
                format!("{name} {args}").trim_end().to_owned().into(),
                format!("Tile dispatcher: {error}").into(),
                None,
            ),
            Row::Command(CommandEntry::Shell(line)) => {
                (line.as_str().into(), "Run command".into(), None)
            }
            Row::Calc(value) => (
                format!("= {}", calc::format(*value)).into(),
                "Enter to copy".into(),
                None,
            ),
        }
    }
}

/// Where the pointer is, in screen pixels.
pub(super) fn cursor() -> (i32, i32) {
    w::GetCursorPos().map_or((0, 0), |p| (p.x, p.y))
}
