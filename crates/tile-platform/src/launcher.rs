//! The app launcher: a search box over everything Start lists (`shell:AppsFolder`), centred
//! on a monitor. It runs on its own thread, and its window is created hidden up front so it
//! opens at once.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc as std_mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use regex::RegexSet;
use tile_config::{Color, Corners, Launcher as Config, LauncherStyle, SearchPosition};
use tile_core::{
    Command, Rect, calc,
    input::Input,
    search::{self, App, CommandEntry, RankOptions, Usage},
    system::{self, Action, Group, Power, format_date, format_time},
};
use tile_hook::notify::{Click, IconImage, ItemKey, Registry, TrayItem};
use tokio::sync::mpsc::UnboundedSender;
use winsafe::{self as w, co, guard::DeleteObjectGuard, gui, prelude::*};

use crate::monitors::rect;

/// What the launcher asks the daemon to do.
#[derive(Debug, Clone, PartialEq)]
pub enum LauncherEvent {
    /// Start the app with this `shell:AppsFolder` id.
    Launch(String),
    /// Start the app with this `shell:AppsFolder` id as administrator.
    LaunchAsAdmin(String),
    /// Run a command line, like `exec`.
    Run(String),
    /// Run a Tile dispatcher.
    Dispatch(Command),
    /// Click a tray icon of the notification area.
    Tray {
        key: ItemKey,
        click: Click,
        /// Where the pointer was, for the menu the app may show.
        at: (i32, i32),
    },
    /// Carry out a system item's action.
    System(Action),
}

/// A result row.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    /// An index into the apps.
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
enum RowIcon {
    App(String),
    Tray(IconImage),
}

/// What picking a row did.
enum Picked {
    /// Close, then send this to the daemon (after, so a dispatcher acts on the window
    /// that had focus before the launcher).
    Done(LauncherEvent),
    /// Changed the search (completed a dispatcher name): redraw.
    Edited,
    /// Copy this to the clipboard, then close.
    Copy(String),
    Nothing,
}

/// The `launcher` config section with its `hide` patterns compiled and its look resolved.
#[derive(Debug, Clone)]
pub struct LauncherSettings {
    pub config: Config,
    pub hide: RegexSet,
    pub style: LauncherStyle,
}

enum Request {
    Toggle(Option<Rect>),
    Settings(Box<LauncherSettings>),
    Apps(Vec<App>),
    System(system::Status),
    Quit,
}

/// The launcher window, on its own thread.
pub struct Launcher {
    requests: std_mpsc::Sender<Request>,
    hwnd: Arc<AtomicIsize>,
    thread: Option<JoinHandle<()>>,
}

impl Launcher {
    /// Starts the launcher thread. Launch history is kept in `usage_path`.
    pub fn spawn(
        events: UnboundedSender<LauncherEvent>,
        settings: LauncherSettings,
        usage_path: PathBuf,
        tray: Registry,
    ) -> Self {
        let (tx, rx) = std_mpsc::channel();
        let hwnd = Arc::new(AtomicIsize::new(0));
        let thread = thread::Builder::new()
            .name("launcher".into())
            .spawn({
                let (tx, hwnd) = (tx.clone(), hwnd.clone());
                move || run(events, settings, usage_path, tray, rx, tx, hwnd)
            })
            .inspect_err(|e| tracing::error!("failed to start the launcher thread: {e}"))
            .ok();
        Self {
            requests: tx,
            hwnd,
            thread,
        }
    }

    fn send(&self, request: Request) {
        let _ = self.requests.send(request);
        wake(&self.hwnd);
    }

    /// Opens the launcher on `fallback` (Tile's focused monitor's work area, which is right
    /// even when that monitor has no windows), or else on the focused window's monitor, or
    /// else on the monitor under the pointer. Closes it if it is open.
    pub fn toggle(&self, fallback: Option<Rect>) {
        self.send(Request::Toggle(fallback));
    }

    pub fn set_settings(&self, settings: LauncherSettings) {
        self.send(Request::Settings(Box::new(settings)));
    }

    /// Closes the window and stops the thread.
    pub fn shutdown(&mut self) {
        self.send(Request::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Has the window in `hwnd` (once made) read its requests.
pub(crate) fn wake(hwnd: &AtomicIsize) {
    let hwnd = hwnd.load(Ordering::Acquire);
    if hwnd != 0 {
        tile_hook::post_app(hwnd, 0);
    }
}

/// Everything Start lists, by name. Runs COM on the calling thread.
fn list_apps() -> Vec<App> {
    let _com = w::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE);
    let list = || -> w::HrResult<Vec<App>> {
        let folder: w::IShellItem =
            w::SHCreateItemFromParsingName("shell:AppsFolder", None::<&w::IBindCtx>)?;
        let items: w::IEnumShellItems =
            folder.BindToHandler(None::<&w::IBindCtx>, &co::BHID::EnumItems)?;
        let mut apps = Vec::new();
        for item in items.iter() {
            let item = item?;
            let (Ok(name), Ok(id)) = (
                item.GetDisplayName(co::SIGDN::NORMALDISPLAY),
                item.GetDisplayName(co::SIGDN::PARENTRELATIVEPARSING),
            ) else {
                continue;
            };
            if !name.trim().is_empty() && !id.is_empty() {
                apps.push(App { name, id });
            }
        }
        Ok(apps)
    };
    let mut apps = list().unwrap_or_else(|e| {
        tracing::warn!("couldn't list apps: {e}");
        Vec::new()
    });
    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.id.clone()));
    apps.dedup();
    apps
}

pub(crate) const fn colorref(c: Color) -> w::COLORREF {
    w::COLORREF::from_rgb(c.r, c.g, c.b)
}

/// `a` moved `amount` (0-255) of the way towards `b`.
fn mix(a: Color, b: Color, amount: u16) -> Color {
    let ch = |x: u8, y: u8| ((u16::from(x) * (255 - amount) + u16::from(y) * amount) / 255) as u8;
    Color::rgb(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b))
}

struct Fonts {
    main: DeleteObjectGuard<w::HFONT>,
    small: DeleteObjectGuard<w::HFONT>,
    /// The main font in bold, for the bar's time.
    bold: DeleteObjectGuard<w::HFONT>,
    /// Between the two, for the bar's date.
    date: DeleteObjectGuard<w::HFONT>,
    /// Windows' icon font, for the bar's buttons.
    icons: DeleteObjectGuard<w::HFONT>,
    /// Pixel heights.
    main_px: i32,
    small_px: i32,
    date_px: i32,
}

/// The icon font this Windows has: Fluent Icons on Windows 11, else the older MDL2 Assets.
fn icon_face() -> &'static str {
    let fonts = std::env::var_os("WINDIR").map_or_else(
        || PathBuf::from(r"C:\Windows\Fonts"),
        |dir| PathBuf::from(dir).join("Fonts"),
    );
    if fonts.join("SegoeIcons.ttf").exists() {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

/// Where things go, in client pixels.
struct Layout {
    search: w::RECT,
    /// The row of tray icons, empty without any.
    strip: w::RECT,
    /// One tray icon's cell width, and the icon's size inside it.
    cell: i32,
    tray_icon: i32,
    /// Where the first tray icon's cell starts, so the row is centred.
    tray_x: i32,
    /// The bar with the clock and the power buttons, empty when it is off.
    bar: w::RECT,
    /// The bar's buttons, in the order of [`BAR_BUTTONS`].
    buttons: Vec<w::RECT>,
    list: w::RECT,
    row: i32,
    icon: i32,
    visible: usize,
}

struct State {
    settings: LauncherSettings,
    events: UnboundedSender<LauncherEvent>,
    /// Everything listed, before `hide`.
    all: Vec<App>,
    apps: Vec<App>,
    loading: bool,
    usage: Usage,
    usage_path: PathBuf,
    query: Input,
    /// A UTF-16 high surrogate waiting for its pair.
    surrogate: Option<u16>,
    results: Vec<Row>,
    selected: usize,
    scroll: usize,
    dpi: u32,
    fonts: Option<Fonts>,
    icons: HashMap<String, Option<tile_hook::Icon>>,
    /// Where the pointer was first seen since opening; moves from there select rows.
    mouse: Option<(i32, i32)>,
    /// The notification area's icons.
    registry: Registry,
    /// The tray icons when the launcher opened.
    tray: Vec<TrayItem>,
    /// The tray icon the pointer is over.
    tray_hover: Option<usize>,
    /// The bar button the pointer is over.
    bar_hover: Option<usize>,
    /// The group of controls shown in place of the results, while one is open.
    panel: Option<Group>,
    /// What the system was doing when the launcher opened.
    status: system::Status,
    /// The system rows matching the search.
    system: Vec<system::Entry>,
    /// A power action that needs Enter pressed again.
    armed: Option<Power>,
    /// While a tray icon's menu is up: the launcher stays open behind it.
    menu: Option<MenuWatch>,
}

/// Waiting on what a click on a tray icon opens: for its menu to close, and for a popup the app
/// puts in a monitor's corner (because it can't find the taskbar) to be moved to the pointer.
struct MenuWatch {
    since: Instant,
    /// The menu has been seen on screen.
    seen: bool,
    /// The launcher stays open behind the menu (a right click); otherwise it has closed.
    keep_open: bool,
    /// The app's process and where the pointer was, to place its popup by.
    pid: Option<u32>,
    at: (i32, i32),
    /// The app's windows that were showing at the click, which are not its popup.
    baseline: HashSet<isize>,
    /// Checking every [`POPUP_POLL_MS`] still, rather than every [`MENU_POLL_MS`].
    fast: bool,
}

const MENU_TIMER: usize = 2;
/// Repaints the bar's clock while the launcher is open.
const CLOCK_TIMER: usize = 3;
/// What a bar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarAction {
    /// Lists a group of controls in place of the results.
    Panel(Group),
    /// Ends the session or puts the machine away.
    Power(Power),
}

/// The buttons of the bar, left to right: an icon in Windows' icon font and what it does. The
/// first [`QUICK_BUTTONS`] open panels, the rest are power buttons.
const BAR_BUTTONS: [(char, BarAction); 9] = [
    ('\u{E767}', BarAction::Panel(Group::Output)),
    ('\u{E720}', BarAction::Panel(Group::Input)),
    ('\u{E701}', BarAction::Panel(Group::Wifi)),
    ('\u{E702}', BarAction::Panel(Group::Bluetooth)),
    ('\u{E748}', BarAction::Power(Power::SwitchUser)),
    ('\u{F3B1}', BarAction::Power(Power::SignOut)),
    ('\u{E72E}', BarAction::Power(Power::Lock)),
    ('\u{E777}', BarAction::Power(Power::Restart)),
    ('\u{E7E8}', BarAction::Power(Power::ShutDown)),
];
/// How many of the bar's buttons open panels.
const QUICK_BUTTONS: usize = 4;
/// Refreshes what the panel shows a moment after something in it was changed.
const STATUS_TIMER: usize = 4;

/// The power action of a bar button, if it is one.
fn power_at(index: usize) -> Option<Power> {
    match BAR_BUTTONS[index].1 {
        BarAction::Power(power) => Some(power),
        BarAction::Panel(_) => None,
    }
}
/// How long a menu may take to appear before it is taken to have never come.
const MENU_GRACE: Duration = Duration::from_millis(1500);
/// How long after a click a popup in a corner is moved, and how often it is looked for then.
const FIX_WINDOW: Duration = Duration::from_secs(2);
const POPUP_POLL_MS: u32 = 50;
/// How often the menu is looked for once the popups are no longer.
const MENU_POLL_MS: u32 = 250;

/// The process that owns the window `raw`.
fn pid_of(raw: isize) -> Option<u32> {
    crate::windows::enumerate(|h| h.ptr() as isize == raw)
        .first()
        .map(|h| h.GetWindowThreadProcessId().1)
}

/// The windows of the process `pid` that are showing.
fn visible_windows(pid: u32) -> HashSet<isize> {
    crate::windows::enumerate(|h| h.GetWindowThreadProcessId().1 == pid && h.IsWindowVisible())
        .iter()
        .map(|h| h.ptr() as isize)
        .collect()
}

/// Some apps place the popup of their tray icon by finding the taskbar, which is not there, and
/// open it in the corner of a monitor. This moves such a popup, a small borderless window of the
/// app that wasn't showing before the click and sits exactly at a monitor's top left corner, to
/// the pointer: above it, or below if there isn't room, kept on that monitor.
fn move_corner_popups(pid: u32, baseline: &HashSet<isize>, at: (i32, i32)) {
    let caption = co::WS::CAPTION.raw() as isize;
    let candidates = crate::windows::enumerate(|h| {
        h.GetWindowThreadProcessId().1 == pid
            && h.IsWindowVisible()
            && !baseline.contains(&(h.ptr() as isize))
            && h.GetWindowLongPtr(co::GWLP::STYLE) & caption != caption
    });
    if candidates.is_empty() {
        return;
    }
    let monitors = crate::monitors::monitors();
    for hwnd in candidates {
        let Ok(r) = hwnd.GetWindowRect() else {
            continue;
        };
        let (width, height) = (r.right - r.left, r.bottom - r.top);
        if !(40..=800).contains(&width) || !(20..=900).contains(&height) {
            continue;
        }
        let Some(corner) = monitors
            .iter()
            .find(|m| (r.left - m.rect.x).abs() <= 2 && (r.top - m.rect.y).abs() <= 2)
        else {
            continue;
        };
        let area = monitors
            .iter()
            .find(|m| m.rect.contains_point(at.0, at.1))
            .unwrap_or(corner)
            .work_area;
        let x = at.0.clamp(area.x, (area.right() - width).max(area.x));
        let above = at.1 - height;
        let y = if above >= area.y {
            above
        } else {
            (at.1 + 8).min((area.bottom() - height).max(area.y))
        };
        let moved = hwnd.SetWindowPos(
            w::HwndPlace::None,
            w::POINT::with(x, y),
            w::SIZE::default(),
            co::SWP::NOSIZE | co::SWP::NOZORDER | co::SWP::NOACTIVATE,
        );
        if let Err(e) = moved {
            tracing::debug!("couldn't move a tray popup: {e}");
        }
    }
}

/// Starts watching what a click on the tray icon of the window `owner` opens. With `keep_open`
/// the launcher stays behind the menu; otherwise it is closing.
fn watch_menu(wnd: &gui::WindowMain, s: &mut State, owner: isize, at: (i32, i32), keep_open: bool) {
    let pid = pid_of(owner);
    s.menu = Some(MenuWatch {
        since: Instant::now(),
        seen: false,
        keep_open,
        pid,
        at,
        baseline: pid.map(visible_windows).unwrap_or_default(),
        fast: true,
    });
    let _ = wnd.hwnd().SetTimer(MENU_TIMER, POPUP_POLL_MS, None);
}

/// Makes the open window as tall as its content, keeping its top where it is.
fn fit_window(wnd: &gui::WindowMain, s: &mut State) {
    let hwnd = wnd.hwnd();
    if !hwnd.IsWindowVisible() {
        return;
    }
    let wanted = s.wanted_height();
    let Ok(rect) = hwnd.GetWindowRect() else {
        return;
    };
    if rect.bottom - rect.top != wanted {
        let resized = hwnd.SetWindowPos(
            w::HwndPlace::None,
            w::POINT::default(),
            w::SIZE::with(rect.right - rect.left, wanted),
            co::SWP::NOMOVE | co::SWP::NOZORDER | co::SWP::NOACTIVATE,
        );
        if let Err(e) = resized {
            tracing::debug!("couldn't resize the launcher: {e}");
        }
    }
}

impl State {
    fn scale(&self, px: i32) -> i32 {
        px * self.dpi as i32 / 96
    }

    fn config(&self) -> &Config {
        &self.settings.config
    }

    fn filter(&mut self) {
        let hide = &self.settings.hide;
        self.apps = self
            .all
            .iter()
            .filter(|a| !hide.is_match(&a.name) && !hide.is_match(&a.id))
            .cloned()
            .collect();
        self.rank();
    }

    fn rank(&mut self) {
        let config = &self.settings.config;
        let options = RankOptions {
            pinned: config.pinned.clone(),
            sort_by_usage: config.sort_by_usage,
            max_results: config.max_results,
        };
        let commands = config.commands;
        let calculator = config.calculator;
        let system_items = config.system_items;
        let query = self.query.text().to_owned();
        self.armed = None;
        self.system.clear();
        if let Some(group) = self.panel {
            self.system = system::find_group(group, &query, &self.status);
            self.results = (0..self.system.len()).map(Row::System).collect();
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let searching = !query.trim().is_empty();
        let sum = calculator.then(|| calc::calculation(&query)).flatten();
        let is_sum = sum.is_some();
        let mut results: Vec<Row> = sum
            .map(Row::Calc)
            .into_iter()
            .chain(
                search::rank(&query, &self.apps, &self.usage, &options)
                    .into_iter()
                    .map(Row::App),
            )
            .collect();
        if searching && system_items {
            results.extend(self.system_rows(&query));
        }
        if searching && commands {
            let mut rows = search::commands(&query);
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
        self.selected = 0;
        self.scroll = 0;
    }

    /// The rows for the system items and tray icons matching `query`, system items first.
    fn system_rows(&mut self, query: &str) -> Vec<Row> {
        self.system = system::find(query, &self.status);
        let query = query.trim();
        let mut tray: Vec<(f64, usize)> = self
            .tray
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((search::fuzzy(query, &item.name)?, i)))
            .collect();
        tray.sort_by(|a, b| b.0.total_cmp(&a.0));
        (0..self.system.len())
            .map(Row::System)
            .chain(tray.into_iter().map(|(_, i)| Row::Tray(i)))
            .collect()
    }

    fn fonts(&mut self) -> &Fonts {
        if self.fonts.is_none() {
            let config = &self.settings.config;
            let main_px = (config.font_size.clamp(6, 48) * self.dpi / 72) as i32;
            let small_px = main_px * 4 / 5;
            let date_px = main_px * 9 / 10;
            let face = &self.settings.style.font;
            let font = |face: &str, px: i32, bold: bool| {
                let mut lf = w::LOGFONT::new_face(-px, face);
                lf.lfQuality = co::QUALITY::CLEARTYPE;
                if bold {
                    lf.lfWeight = co::FW::BOLD;
                }
                w::HFONT::CreateFontIndirect(&lf)
            };
            let made = (|| {
                Ok::<_, co::ERROR>(Fonts {
                    main: font(face, main_px, false)?,
                    small: font(face, small_px, false)?,
                    bold: font(face, main_px, true)?,
                    date: font(face, date_px, false)?,
                    icons: font(icon_face(), main_px * 12 / 10, false)?,
                    main_px,
                    small_px,
                    date_px,
                })
            })();
            match made {
                Ok(fonts) => self.fonts = Some(fonts),
                Err(e) => tracing::warn!("launcher font `{face}`: {e}"),
            }
        }
        self.fonts.get_or_insert_with(|| {
            let plain = || {
                w::HFONT::CreateFontIndirect(&w::LOGFONT::default())
                    .expect("the default font exists")
            };
            Fonts {
                main: plain(),
                small: plain(),
                bold: plain(),
                date: plain(),
                icons: plain(),
                main_px: 16,
                small_px: 13,
                date_px: 14,
            }
        })
    }

    /// The most rows shown at once.
    fn max_rows(&self) -> usize {
        self.config().max_visible.clamp(1, 30)
    }

    /// The window height that fits everything but the list exactly, and `rows` rows of it.
    fn height_for(&mut self, rows: usize) -> i32 {
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
    fn wanted_height(&mut self) -> i32 {
        let rows = self.results.len().clamp(1, self.max_rows());
        self.height_for(rows)
    }

    fn layout(&mut self, client: w::RECT) -> Layout {
        self.fonts();
        let fonts = self.fonts.take().expect("fonts were just made");
        let layout = self.layout_with(&fonts, client);
        self.fonts = Some(fonts);
        layout
    }

    fn layout_with(&self, fonts: &Fonts, client: w::RECT) -> Layout {
        let (main_px, small_px) = (fonts.main_px, fonts.small_px);
        let config = &self.settings.config;
        let pad = self.scale(12);
        let gap = self.scale(8);
        let search_h = main_px + self.scale(24);
        let (row, icon) = if config.compact {
            (main_px + self.scale(16), self.scale(20))
        } else {
            (main_px + small_px + self.scale(22), self.scale(32))
        };
        let icon = if config.show_icons { icon } else { 0 };
        let (left, right) = (client.left + pad, client.right - pad);
        let tray_icon = self.scale(24);
        let cell = tray_icon + self.scale(12);
        let strip_h = if self.tray.is_empty() { 0 } else { cell };
        let tray_x = left + ((right - left - self.tray.len() as i32 * cell) / 2).max(0);
        // Top to bottom: the bar, the tray icons, then the search box and the results.
        let bar_h = if config.bar {
            main_px + self.scale(16)
        } else {
            0
        };
        let bar = w::RECT {
            left,
            top: client.top + pad,
            right,
            bottom: client.top + pad + bar_h,
        };
        let below_bar = if bar_h > 0 {
            bar.bottom + gap
        } else {
            bar.bottom
        };
        let strip = w::RECT {
            left,
            top: below_bar,
            right,
            bottom: below_bar + strip_h,
        };
        let content = if strip_h > 0 {
            strip.bottom + gap
        } else {
            below_bar
        };
        // Square buttons from the right edge.
        let mut buttons = Vec::new();
        if bar_h > 0 {
            let mut x = bar.right;
            for i in (0..BAR_BUTTONS.len()).rev() {
                // A wider gap between the panels' buttons and the power buttons.
                if i + 1 == QUICK_BUTTONS {
                    x -= self.scale(14);
                }
                buttons.push(w::RECT {
                    left: x - bar_h,
                    top: bar.top,
                    right: x,
                    bottom: bar.bottom,
                });
                x -= bar_h + self.scale(6);
            }
            buttons.reverse();
        }
        let (search, list) = match config.search_position {
            SearchPosition::Top => {
                let search = w::RECT {
                    left,
                    top: content,
                    right,
                    bottom: content + search_h,
                };
                let list = w::RECT {
                    left,
                    top: search.bottom + gap,
                    right,
                    bottom: client.bottom - pad,
                };
                (search, list)
            }
            SearchPosition::Bottom => {
                let search = w::RECT {
                    left,
                    top: client.bottom - pad - search_h,
                    right,
                    bottom: client.bottom - pad,
                };
                let list = w::RECT {
                    left,
                    top: content,
                    right,
                    bottom: search.top - gap,
                };
                (search, list)
            }
        };
        let visible = ((list.bottom - list.top) / row.max(1)).max(1) as usize;
        Layout {
            search,
            strip,
            cell,
            tray_icon,
            tray_x,
            bar,
            buttons,
            list,
            row,
            icon,
            visible,
        }
    }

    fn select(&mut self, index: isize, visible: usize) {
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

    fn scroll_by(&mut self, rows: isize, visible: usize) {
        let max = self.results.len().saturating_sub(visible) as isize;
        self.scroll = (self.scroll as isize + rows).clamp(0, max) as usize;
    }

    /// The width the bar needs for its clock, date and buttons.
    fn bar_width(&self, main_px: i32, date_px: i32) -> i32 {
        let button = main_px + self.scale(16);
        let buttons = BAR_BUTTONS.len() as i32 * (button + self.scale(6)) + self.scale(14);
        let clock = (12 * main_px + 24 * date_px) * 11 / 20;
        2 * self.scale(12) + clock + self.scale(16) + buttons
    }

    /// The bar button at a point.
    fn bar_button_at(&self, layout: &Layout, x: i32, y: i32) -> Option<usize> {
        layout
            .buttons
            .iter()
            .position(|b| x >= b.left && x < b.right && y >= b.top && y < b.bottom)
    }

    /// The tray icon at a point of the strip.
    fn tray_at(&self, layout: &Layout, x: i32, y: i32) -> Option<usize> {
        let strip = layout.strip;
        if strip.bottom <= strip.top
            || x < layout.tray_x
            || x >= strip.right
            || y < strip.top
            || y >= strip.bottom
        {
            return None;
        }
        let index = ((x - layout.tray_x) / layout.cell.max(1)) as usize;
        (index < self.tray.len()).then_some(index)
    }

    fn row_at(&self, layout: &Layout, x: i32, y: i32) -> Option<usize> {
        let list = layout.list;
        if x < list.left || x >= list.right || y < list.top || y >= list.bottom {
            return None;
        }
        let index = self.scroll + ((y - list.top) / layout.row) as usize;
        (index < self.results.len() && index < self.scroll + layout.visible).then_some(index)
    }

    /// Picks the selected row: what to ask the daemon (noting an app launch), or completes a
    /// dispatcher's name. `secondary` (Shift+Enter, the right button's menu) is a right click on a
    /// tray icon, or starting an app as administrator.
    fn pick(&mut self, secondary: bool) -> Picked {
        let event = match self.results.get(self.selected).cloned() {
            None => return Picked::Nothing,
            Some(Row::Tray(i)) => {
                let Some(item) = self.tray.get(i) else {
                    return Picked::Nothing;
                };
                let at = w::GetCursorPos().map_or((0, 0), |p| (p.x, p.y));
                let click = if secondary { Click::Right } else { Click::Left };
                LauncherEvent::Tray {
                    key: item.key,
                    click,
                    at,
                }
            }
            Some(Row::System(i)) => {
                let Some(entry) = self.system.get(i) else {
                    return Picked::Nothing;
                };
                let action = entry.action.clone();
                if let Action::Power(power) = action
                    && power.needs_confirmation()
                    && self.armed != Some(power)
                {
                    self.armed = Some(power);
                    return Picked::Edited;
                }
                LauncherEvent::System(action)
            }
            Some(Row::Calc(value)) => return Picked::Copy(calc::format(value)),
            Some(Row::App(i)) => {
                let id = self.apps[i].id.clone();
                if self.config().track_usage {
                    self.usage.record(&id);
                    save_usage(&self.usage_path, &self.usage);
                }
                if secondary {
                    LauncherEvent::LaunchAsAdmin(id)
                } else {
                    LauncherEvent::Launch(id)
                }
            }
            Some(Row::Command(CommandEntry::Dispatch(command))) => LauncherEvent::Dispatch(command),
            Some(Row::Command(CommandEntry::Name(name))) => match name.parse() {
                Ok(command) => LauncherEvent::Dispatch(command),
                Err(_) => return Picked::Nothing,
            },
            Some(Row::Command(CommandEntry::Invalid { .. })) => return Picked::Nothing,
            Some(Row::Command(CommandEntry::Shell(line))) => LauncherEvent::Run(line),
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
    fn describe(&self, row: &Row) -> (String, String, Option<RowIcon>) {
        match row {
            Row::App(i) => {
                let app = &self.apps[*i];
                (
                    app.name.clone(),
                    app.id.clone(),
                    Some(RowIcon::App(app.id.clone())),
                )
            }
            Row::Tray(i) => {
                let item = &self.tray[*i];
                (
                    item.name.clone(),
                    "Tray icon. Enter clicks it, Shift+Enter or right click opens its menu".into(),
                    item.icon.clone().map(RowIcon::Tray),
                )
            }
            Row::System(i) => {
                let entry = &self.system[*i];
                let confirming = matches!(entry.action, Action::Power(p) if self.armed == Some(p));
                let subtitle = if confirming {
                    "Press Enter again to confirm".to_owned()
                } else {
                    entry.subtitle.clone()
                };
                (entry.title.clone(), subtitle, None)
            }
            Row::Command(CommandEntry::Dispatch(command)) => {
                (command.to_string(), "Tile dispatcher".into(), None)
            }
            Row::Command(CommandEntry::Name(name)) => {
                ((*name).to_owned(), "Tile dispatcher".into(), None)
            }
            Row::Command(CommandEntry::Complete { name, args }) => (
                format!("{name} {args}"),
                "Tile dispatcher: Enter to add the argument".into(),
                None,
            ),
            Row::Command(CommandEntry::Invalid { name, args, error }) => (
                format!("{name} {args}").trim_end().to_owned(),
                format!("Tile dispatcher: {error}"),
                None,
            ),
            Row::Command(CommandEntry::Shell(line)) => (line.clone(), "Run command".into(), None),
            Row::Calc(value) => (
                format!("= {}", calc::format(*value)),
                "Enter to copy".into(),
                None,
            ),
        }
    }

    /// A tray icon drawn at exactly `size` pixels.
    fn tray_icon_at(&mut self, image: &IconImage, size: i32) -> Option<&tile_hook::Icon> {
        self.icons
            .entry(format!("hicon:{}:{size}", image.handle()))
            .or_insert_with(|| image.render(size))
            .as_ref()
    }

    /// The icon of a program file, at `size` pixels.
    fn file_icon(&mut self, path: &str, size: i32) -> Option<&tile_hook::Icon> {
        self.icons
            .entry(format!("file:{path}"))
            .or_insert_with(|| tile_hook::shell_icon(path, size))
            .as_ref()
    }

    fn icon(&mut self, id: &str, size: i32) -> Option<&tile_hook::Icon> {
        self.icons
            .entry(id.to_owned())
            .or_insert_with(|| tile_hook::shell_icon(&format!("shell:AppsFolder\\{id}"), size))
            .as_ref()
    }
}

fn load_usage(path: &PathBuf) -> Usage {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_usage(path: &PathBuf, usage: &Usage) {
    let result = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| {
            fs::write(
                path,
                serde_json::to_string_pretty(usage).unwrap_or_default(),
            )
        });
    if let Err(e) = result {
        tracing::warn!("couldn't save {}: {e}", path.display());
    }
}

/// The work area to open in: `preferred` (the monitor Tile has focused, empty or not), then the
/// focused window's monitor, then the pointer's monitor.
fn open_area(preferred: Option<Rect>) -> Option<Rect> {
    let work_area = |monitor: w::HMONITOR| monitor.GetMonitorInfo().ok().map(|i| rect(i.rcWork));
    // The desktop and taskbar have focus on an empty monitor, but say nothing about which, and
    // Windows may leave focus on a window of another monitor.
    let focused = || {
        w::HWND::GetForegroundWindow()
            .filter(|hwnd| !crate::is_shell_window(hwnd.ptr() as isize))
            .and_then(|hwnd| work_area(hwnd.MonitorFromWindow(co::MONITOR::DEFAULTTONEAREST)))
    };
    preferred.or_else(focused).or_else(|| {
        work_area(w::HMONITOR::MonitorFromPoint(
            w::GetCursorPos().unwrap_or_default(),
            co::MONITOR::DEFAULTTONEAREST,
        ))
    })
}

pub(crate) const fn corner_preference(corners: Corners) -> co::DWMWCP {
    match corners {
        Corners::Round => co::DWMWCP::ROUND,
        Corners::Small => co::DWMWCP::ROUNDSMALL,
        Corners::Square => co::DWMWCP::DONOTROUND,
    }
}

fn style_window(hwnd: &w::HWND, settings: &LauncherSettings) {
    let (config, style) = (&settings.config, &settings.style);
    let corners = corner_preference(style.corners);
    let _ = hwnd.DwmSetWindowAttribute(w::DwmAttr::WindowCornerPreference(corners));
    let _ = hwnd.DwmSetWindowAttribute(w::DwmAttr::BorderColor(colorref(style.border)));
    let alpha = (config.opacity.clamp(0.1, 1.0) * 255.0).round() as u8;
    if let Err(e) =
        hwnd.SetLayeredWindowAttributes(w::COLORREF::from_rgb(0, 0, 0), alpha, co::LWA::ALPHA)
    {
        tracing::warn!("launcher opacity: {e}");
    }
}

fn run(
    events: UnboundedSender<LauncherEvent>,
    settings: LauncherSettings,
    usage_path: PathBuf,
    registry: Registry,
    requests: std_mpsc::Receiver<Request>,
    requests_tx: std_mpsc::Sender<Request>,
    hwnd_slot: Arc<AtomicIsize>,
) {
    // For the icons, which come from the shell.
    let _com = w::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE);
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Tile launcher",
        class_name: "tile-launcher",
        class_bg_brush: gui::Brush::None,
        size: (settings.config.width as i32, 100),
        style: co::WS::POPUP | co::WS::CLIPCHILDREN,
        ex_style: co::WS_EX::TOOLWINDOW | co::WS_EX::TOPMOST | co::WS_EX::LAYERED,
        // Dialog handling would swallow typed characters (and Tab) as mnemonics.
        process_dlg_msgs: false,
        ..Default::default()
    });
    let usage = load_usage(&usage_path);
    let state = Rc::new(RefCell::new(State {
        settings,
        events,
        all: Vec::new(),
        apps: Vec::new(),
        loading: true,
        usage,
        usage_path,
        query: Input::default(),
        surrogate: None,
        results: Vec::new(),
        selected: 0,
        scroll: 0,
        dpi: 96,
        fonts: None,
        icons: HashMap::new(),
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
    }));
    let listing = Arc::new(AtomicBool::new(false));
    let system_tx = requests_tx.clone();
    let refresh = {
        let (tx, hwnd_slot, listing) = (requests_tx, hwnd_slot.clone(), listing);
        move || {
            if listing.swap(true, Ordering::AcqRel) {
                return;
            }
            let (tx, hwnd_slot, listing) = (tx.clone(), hwnd_slot.clone(), listing.clone());
            let spawned = thread::Builder::new()
                .name("launcher apps".into())
                .spawn(move || {
                    let apps = list_apps();
                    listing.store(false, Ordering::Release);
                    let _ = tx.send(Request::Apps(apps));
                    wake(&hwnd_slot);
                });
            if let Err(e) = spawned {
                tracing::warn!("couldn't list apps: {e}");
            }
        }
    };

    // Reading the system waits on Windows, so it happens on a thread of its own.
    let reading = Arc::new(AtomicBool::new(false));
    let refresh_system = {
        let hwnd_slot = hwnd_slot.clone();
        move || {
            if reading.swap(true, Ordering::AcqRel) {
                return;
            }
            let (tx, hwnd_slot, reading) = (system_tx.clone(), hwnd_slot.clone(), reading.clone());
            let spawned = thread::Builder::new()
                .name("launcher system".into())
                .spawn(move || {
                    let status = crate::system::status();
                    reading.store(false, Ordering::Release);
                    let _ = tx.send(Request::System(status));
                    wake(&hwnd_slot);
                });
            if let Err(e) = spawned {
                tracing::warn!("couldn't read the system: {e}");
            }
        }
    };

    wnd.on().wm_create({
        let (wnd, state, refresh, refresh_system, hwnd_slot) = (
            wnd.clone(),
            state.clone(),
            refresh.clone(),
            refresh_system.clone(),
            hwnd_slot.clone(),
        );
        move |_| {
            style_window(wnd.hwnd(), &state.borrow().settings);
            hwnd_slot.store(wnd.hwnd().ptr() as isize, Ordering::Release);
            refresh();
            // So the first time it opens already knows the system.
            refresh_system();
            // Requests sent before the window existed.
            tile_hook::post_app(wnd.hwnd().ptr() as isize, 0);
            Ok(0)
        }
    });

    let hide = {
        let wnd = wnd.clone();
        move || {
            wnd.hwnd().ShowWindow(co::SW::HIDE);
        }
    };

    let status_refresh = refresh_system.clone();

    let toggle = {
        let (wnd, state, hide, refresh) = (wnd.clone(), state.clone(), hide.clone(), refresh);
        move |fallback: Option<Rect>| {
            let hwnd = wnd.hwnd();
            if hwnd.IsWindowVisible() {
                hide();
                return;
            }
            let Some(area) = open_area(fallback) else {
                return;
            };
            let mut s = state.borrow_mut();
            let (cx, cy) = area.center();
            let dpi = tile_hook::monitor_dpi(cx, cy);
            if dpi != s.dpi {
                s.dpi = dpi;
                s.fonts = None;
                s.icons.clear();
            }
            // The icons first, since they decide how wide the window must be to hold them.
            s.tray = s.registry.items();
            // The icon handles of the last time may have been reused for other images.
            s.icons.retain(|key, _| !key.starts_with("hicon:"));
            let tray_width = s.tray.len() as i32 * s.scale(36) + 2 * s.scale(12);
            let (main_px, date_px) = {
                let fonts = s.fonts();
                (fonts.main_px, fonts.date_px)
            };
            let bar_width = if s.config().bar {
                s.bar_width(main_px, date_px)
            } else {
                0
            };
            let w = s
                .scale(s.config().width.clamp(200, 4000) as i32)
                .max(tray_width)
                .max(bar_width)
                .min(area.w);
            s.query.clear();
            s.surrogate = None;
            s.mouse = None;
            s.tray_hover = None;
            s.bar_hover = None;
            s.panel = None;
            s.menu = None;
            s.rank();
            // Centred for the full number of rows, so it keeps its top as it shrinks.
            let full = s.max_rows();
            let top = cy - s.height_for(full).min(area.h) / 2;
            let h = s.wanted_height().min(area.h);
            drop(s);
            let placed = hwnd.SetWindowPos(
                w::HwndPlace::Place(co::HWND_PLACE::TOPMOST),
                w::POINT::with(cx - w / 2, top),
                w::SIZE::with(w, h),
                co::SWP::SHOWWINDOW,
            );
            if let Err(e) = placed {
                tracing::warn!("couldn't show the launcher: {e}");
                return;
            }
            let _ = hwnd.InvalidateRect(None, false);
            let _ = hwnd.UpdateWindow();
            // The bar's clock shows seconds.
            let _ = hwnd.SetTimer(CLOCK_TIMER, 1000, None);
            if !crate::windows::focus_hwnd(hwnd) {
                tracing::debug!("the launcher didn't get focus");
            }
            refresh();
            refresh_system();
        }
    };

    wnd.on().wm(co::WM::APP, {
        let (wnd, state) = (wnd.clone(), state.clone());
        move |_| {
            while let Ok(request) = requests.try_recv() {
                match request {
                    Request::Toggle(fallback) => toggle(fallback),
                    Request::Settings(settings) => {
                        let mut s = state.borrow_mut();
                        s.settings = *settings;
                        s.fonts = None;
                        s.icons.clear();
                        s.filter();
                        style_window(wnd.hwnd(), &s.settings);
                        fit_window(&wnd, &mut s);
                        let _ = wnd.hwnd().InvalidateRect(None, false);
                    }
                    Request::Apps(apps) => {
                        let mut s = state.borrow_mut();
                        s.loading = false;
                        if apps != s.all {
                            s.all = apps;
                            s.filter();
                            fit_window(&wnd, &mut s);
                            let _ = wnd.hwnd().InvalidateRect(None, false);
                        }
                    }
                    Request::System(status) => {
                        let mut s = state.borrow_mut();
                        if status != s.status {
                            s.status = status;
                            // Something typed may match rows that weren't known when it was
                            // typed. Re-ranking resets the selection, so it is put back.
                            if !s.query.text().trim().is_empty() || s.panel.is_some() {
                                let (selected, scroll) = (s.selected, s.scroll);
                                s.rank();
                                s.selected = selected.min(s.results.len().saturating_sub(1));
                                s.scroll = scroll;
                                fit_window(&wnd, &mut s);
                            }
                            let _ = wnd.hwnd().InvalidateRect(None, false);
                        }
                    }
                    Request::Quit => {
                        state.borrow_mut().icons.clear();
                        w::PostQuitMessage(0);
                    }
                }
            }
            if wnd.hwnd().IsWindowVisible() {
                let _ = wnd.hwnd().UpdateWindow();
            }
            Ok(0)
        }
    });

    wnd.on().wm_timer(STATUS_TIMER, {
        let (wnd, status_refresh) = (wnd.clone(), status_refresh.clone());
        move || {
            let _ = wnd.hwnd().KillTimer(STATUS_TIMER);
            status_refresh();
            Ok(())
        }
    });

    wnd.on().wm_timer(CLOCK_TIMER, {
        let wnd = wnd.clone();
        move || {
            if wnd.hwnd().IsWindowVisible() {
                let _ = wnd.hwnd().InvalidateRect(None, false);
            } else {
                let _ = wnd.hwnd().KillTimer(CLOCK_TIMER);
            }
            Ok(())
        }
    });

    wnd.on().wm_activate({
        let (hide, state) = (hide.clone(), state.clone());
        move |p| {
            // The menu of a tray icon takes the focus: the launcher stays until it closes.
            if p.event == co::WA::INACTIVE
                && !state.borrow().menu.as_ref().is_some_and(|m| m.keep_open)
            {
                hide();
            }
            Ok(())
        }
    });

    // Once the tray icon's menu has closed, back to the launcher, unless the choice brought
    // another window forward.
    wnd.on().wm_timer(MENU_TIMER, {
        let (wnd, state, hide) = (wnd.clone(), state.clone(), hide.clone());
        move || {
            let mut s = state.borrow_mut();
            let hwnd = wnd.hwnd();
            let fix = s.config().fix_popups;
            let Some(watch) = s.menu.as_mut() else {
                let _ = hwnd.KillTimer(MENU_TIMER);
                return Ok(());
            };
            let fixing = fix && watch.since.elapsed() < FIX_WINDOW;
            if fixing && let Some(pid) = watch.pid {
                move_corner_popups(pid, &watch.baseline, watch.at);
            }
            if watch.fast && !fixing {
                watch.fast = false;
                let _ = hwnd.SetTimer(MENU_TIMER, MENU_POLL_MS, None);
            }
            if !watch.keep_open {
                // The launcher has closed: only the popup is left to watch.
                if !fixing {
                    s.menu = None;
                    let _ = hwnd.KillTimer(MENU_TIMER);
                }
                return Ok(());
            }
            if !hwnd.IsWindowVisible() {
                s.menu = None;
                let _ = hwnd.KillTimer(MENU_TIMER);
                return Ok(());
            }
            let open = !crate::windows::enumerate(|h| {
                h.IsWindowVisible() && h.GetClassName().is_ok_and(|c| c == "#32768")
            })
            .is_empty();
            if open {
                watch.seen = true;
                return Ok(());
            }
            if !watch.seen && watch.since.elapsed() < MENU_GRACE {
                return Ok(());
            }
            s.menu = None;
            drop(s);
            let _ = hwnd.KillTimer(MENU_TIMER);
            let elsewhere = w::HWND::GetForegroundWindow().is_some_and(|f| {
                f != *hwnd
                    && f.IsWindowVisible()
                    && f.GetWindowText().is_ok_and(|t| !t.is_empty())
                    && !crate::is_shell_window(f.ptr() as isize)
            });
            if elsewhere {
                hide();
            } else {
                crate::windows::focus_hwnd(hwnd);
                let _ = hwnd.InvalidateRect(None, false);
            }
            Ok(())
        }
    });

    wnd.on().wm_paint({
        let (wnd, state) = (wnd.clone(), state.clone());
        move || {
            let hwnd = wnd.hwnd();
            let hdc = hwnd.BeginPaint()?;
            let client = hwnd.GetClientRect()?;
            let (w, h) = (client.right - client.left, client.bottom - client.top);
            let mem = hdc.CreateCompatibleDC()?;
            let bitmap = hdc.CreateCompatibleBitmap(w, h)?;
            let _bitmap = mem.SelectObject(&*bitmap)?;
            paint(&mut state.borrow_mut(), &mem, client)?;
            hdc.BitBlt(
                w::POINT::default(),
                w::SIZE::with(w, h),
                &mem,
                w::POINT::default(),
                co::ROP::SRCCOPY,
            )?;
            Ok(())
        }
    });

    // Does what picking the selected row asks for.
    let act = {
        let (wnd, hide) = (wnd.clone(), hide.clone());
        move |state: &Rc<RefCell<State>>, secondary: bool| {
            let mut s = state.borrow_mut();
            match s.pick(secondary) {
                Picked::Done(event) => {
                    let events = s.events.clone();
                    let in_panel = s.panel.is_some();
                    drop(s);
                    // In a panel the launcher stays, to change more; what it shows catches up
                    // a moment later. Opening Settings or ending the session closes it.
                    let stays = in_panel
                        && matches!(
                            &event,
                            LauncherEvent::System(a)
                                if !matches!(a, Action::Open(_) | Action::Power(_))
                        );
                    if stays {
                        let _ = events.send(event);
                        let _ = wnd.hwnd().SetTimer(STATUS_TIMER, 400, None);
                        return;
                    }
                    if let LauncherEvent::Tray { key, click, at } = &event {
                        // So the app can show its window or menu in front.
                        tile_hook::notify::allow_foreground();
                        let keep_open = *click == Click::Right;
                        watch_menu(&wnd, &mut state.borrow_mut(), key.hwnd, *at, keep_open);
                        if keep_open {
                            let _ = events.send(event);
                            return;
                        }
                    }
                    hide();
                    let _ = events.send(event);
                }
                Picked::Copy(text) => {
                    drop(s);
                    if let Err(e) = copy(wnd.hwnd(), &text) {
                        tracing::warn!("couldn't copy `{text}`: {e}");
                    }
                    hide();
                }
                Picked::Edited => {
                    drop(s);
                    let _ = wnd.hwnd().InvalidateRect(None, false);
                }
                Picked::Nothing => {}
            }
        }
    };

    // Clicks a tray icon of the strip once the launcher has closed.
    let click_tray = {
        let (wnd, hide) = (wnd.clone(), hide.clone());
        move |state: &Rc<RefCell<State>>, index: usize, click: Click| {
            let s = state.borrow();
            let Some(item) = s.tray.get(index) else {
                return;
            };
            let at = w::GetCursorPos().map_or((0, 0), |p| (p.x, p.y));
            let key = item.key;
            let event = LauncherEvent::Tray { key, click, at };
            let events = s.events.clone();
            drop(s);
            // So the app can show its window or menu in front.
            tile_hook::notify::allow_foreground();
            // A right click's menu opens over the launcher, which stays until it closes.
            let keep_open = click == Click::Right;
            watch_menu(&wnd, &mut state.borrow_mut(), key.hwnd, at, keep_open);
            if !keep_open {
                hide();
            }
            let _ = events.send(event);
        }
    };

    // A button of the bar. Those that lose unsaved work ask for a second click first.
    let click_bar = {
        let (wnd, hide, status_refresh) = (wnd.clone(), hide.clone(), status_refresh.clone());
        move |state: &Rc<RefCell<State>>, index: usize, right: bool| {
            let mut s = state.borrow_mut();
            let power = match BAR_BUTTONS[index].1 {
                BarAction::Panel(group) if !right => {
                    // A left click flips the switch; what is shown follows at once.
                    if let Some(action) = group.toggle(&s.status) {
                        s.status.expect(&action);
                        let events = s.events.clone();
                        if s.panel.is_some() {
                            s.rank();
                        }
                        drop(s);
                        let _ = events.send(LauncherEvent::System(action));
                        let _ = wnd.hwnd().SetTimer(STATUS_TIMER, 400, None);
                        let _ = wnd.hwnd().InvalidateRect(None, false);
                    }
                    return;
                }
                BarAction::Panel(group) => {
                    // The same button again goes back to the results.
                    s.panel = if s.panel == Some(group) {
                        None
                    } else {
                        Some(group)
                    };
                    s.query.clear();
                    s.rank();
                    fit_window(&wnd, &mut s);
                    drop(s);
                    let _ = wnd.hwnd().InvalidateRect(None, false);
                    let _ = wnd.hwnd().UpdateWindow();
                    // So it shows what things are now.
                    status_refresh();
                    return;
                }
                BarAction::Power(power) => power,
            };
            if power.needs_confirmation() && s.armed != Some(power) {
                s.armed = Some(power);
                drop(s);
                let _ = wnd.hwnd().InvalidateRect(None, false);
                return;
            }
            s.armed = None;
            let events = s.events.clone();
            drop(s);
            hide();
            let _ = events.send(LauncherEvent::System(Action::Power(power)));
        }
    };

    wnd.on().wm_key_down({
        let (wnd, state, hide, act) = (wnd.clone(), state.clone(), hide.clone(), act.clone());
        move |p| {
            let ctrl = w::GetKeyState(co::VK::CONTROL).0;
            let shift = w::GetKeyState(co::VK::SHIFT).0;
            let client = wnd.hwnd().GetClientRect()?;
            let mut s = state.borrow_mut();
            let visible = s.layout(client).visible;
            let page = visible as isize;
            let selected = s.selected as isize;
            let mut edited = false;
            match p.vkey_code {
                co::VK::ESCAPE => {
                    if s.panel.take().is_some() {
                        s.query.clear();
                        s.rank();
                        fit_window(&wnd, &mut s);
                        drop(s);
                        let _ = wnd.hwnd().InvalidateRect(None, false);
                        let _ = wnd.hwnd().UpdateWindow();
                        return Ok(());
                    }
                    drop(s);
                    hide();
                    return Ok(());
                }
                co::VK::RETURN => {
                    drop(s);
                    act(&state, shift);
                    return Ok(());
                }
                co::VK::UP => s.select(selected - 1, visible),
                co::VK::DOWN => s.select(selected + 1, visible),
                co::VK::TAB if shift => s.select(selected - 1, visible),
                co::VK::TAB => s.select(selected + 1, visible),
                co::VK::PRIOR => s.select(selected - page, visible),
                co::VK::NEXT => s.select(selected + page, visible),
                co::VK::CHAR_K | co::VK::CHAR_P if ctrl => s.select(selected - 1, visible),
                co::VK::CHAR_J | co::VK::CHAR_N if ctrl => s.select(selected + 1, visible),
                co::VK::LEFT if ctrl => _ = s.query.word_left(),
                co::VK::RIGHT if ctrl => _ = s.query.word_right(),
                co::VK::LEFT => _ = s.query.left(),
                co::VK::RIGHT => _ = s.query.right(),
                co::VK::CHAR_H if ctrl => _ = s.query.left(),
                co::VK::CHAR_L if ctrl => _ = s.query.right(),
                co::VK::HOME => _ = s.query.home(),
                co::VK::END => _ = s.query.end(),
                co::VK::BACK | co::VK::CHAR_W if ctrl => edited = s.query.delete_word(),
                co::VK::CHAR_U if ctrl => edited = s.query.delete_to_start(),
                co::VK::BACK => edited = s.query.backspace(),
                co::VK::DELETE => edited = s.query.delete(),
                _ => return Ok(()),
            }
            if edited {
                s.rank();
                fit_window(&wnd, &mut s);
            }
            drop(s);
            let _ = wnd.hwnd().InvalidateRect(None, false);
            // Painted now, not when Windows next gets to it, so a resize and its new contents
            // reach the screen together.
            let _ = wnd.hwnd().UpdateWindow();
            Ok(())
        }
    });

    wnd.on().wm_char({
        let (wnd, state) = (wnd.clone(), state.clone());
        move |p| {
            let code = p.char_code;
            let mut s = state.borrow_mut();
            let c = match code {
                0xD800..=0xDBFF => {
                    s.surrogate = Some(code);
                    return Ok(());
                }
                0xDC00..=0xDFFF => {
                    let Some(high) = s.surrogate.take() else {
                        return Ok(());
                    };
                    char::decode_utf16([high, code]).next().and_then(Result::ok)
                }
                // Control characters: their keys are handled in `wm_key_down`.
                ..0x20 | 0x7F => None,
                _ => char::from_u32(u32::from(code)),
            };
            let Some(c) = c else {
                return Ok(());
            };
            s.query.insert(c);
            s.rank();
            fit_window(&wnd, &mut s);
            drop(s);
            let _ = wnd.hwnd().InvalidateRect(None, false);
            let _ = wnd.hwnd().UpdateWindow();
            Ok(())
        }
    });

    wnd.on().wm_mouse_move({
        let (wnd, state) = (wnd.clone(), state.clone());
        move |p| {
            let mut s = state.borrow_mut();
            if s.config().ignore_mouse {
                return Ok(());
            }
            let at = (p.coords.x, p.coords.y);
            // Windows sends a move when the window opens under a still pointer; only real
            // moves select.
            match s.mouse {
                None => {
                    s.mouse = Some(at);
                    return Ok(());
                }
                Some(first) if first == at => return Ok(()),
                Some(_) => {}
            }
            let layout = s.layout(wnd.hwnd().GetClientRect()?);
            let hover = s.tray_at(&layout, at.0, at.1);
            if hover != s.tray_hover {
                s.tray_hover = hover;
                let _ = wnd.hwnd().InvalidateRect(None, false);
            }
            let bar_hover = s.bar_button_at(&layout, at.0, at.1);
            if bar_hover != s.bar_hover {
                s.bar_hover = bar_hover;
                // Moving off a button that was asked to confirm takes the question back.
                if s.armed.is_some() && bar_hover.and_then(power_at) != s.armed {
                    s.armed = None;
                }
                let _ = wnd.hwnd().InvalidateRect(None, false);
            }
            if let Some(row) = s.row_at(&layout, at.0, at.1)
                && row != s.selected
            {
                s.selected = row;
                s.armed = None;
                drop(s);
                let _ = wnd.hwnd().InvalidateRect(None, false);
            }
            Ok(())
        }
    });

    wnd.on().wm_l_button_down({
        let (wnd, state, act, click_tray, click_bar) = (
            wnd.clone(),
            state.clone(),
            act.clone(),
            click_tray.clone(),
            click_bar.clone(),
        );
        move |p| {
            let mut s = state.borrow_mut();
            if s.config().ignore_mouse {
                return Ok(());
            }
            let layout = s.layout(wnd.hwnd().GetClientRect()?);
            if let Some(index) = s.tray_at(&layout, p.coords.x, p.coords.y) {
                drop(s);
                click_tray(&state, index, Click::Left);
                return Ok(());
            }
            if let Some(index) = s.bar_button_at(&layout, p.coords.x, p.coords.y) {
                drop(s);
                click_bar(&state, index, false);
                return Ok(());
            }
            if let Some(row) = s.row_at(&layout, p.coords.x, p.coords.y) {
                s.selected = row;
                drop(s);
                act(&state, false);
            }
            Ok(())
        }
    });

    // The right button opens a tray icon's menu.
    wnd.on().wm_r_button_down({
        let (wnd, state) = (wnd.clone(), state.clone());
        move |p| {
            let mut s = state.borrow_mut();
            if s.config().ignore_mouse {
                return Ok(());
            }
            let layout = s.layout(wnd.hwnd().GetClientRect()?);
            if let Some(index) = s.tray_at(&layout, p.coords.x, p.coords.y) {
                drop(s);
                click_tray(&state, index, Click::Right);
                return Ok(());
            }
            if let Some(index) = s.bar_button_at(&layout, p.coords.x, p.coords.y) {
                drop(s);
                click_bar(&state, index, true);
                return Ok(());
            }
            if let Some(row) = s.row_at(&layout, p.coords.x, p.coords.y) {
                match s.results[row] {
                    Row::Tray(_) => {
                        s.selected = row;
                        drop(s);
                        act(&state, true);
                    }
                    Row::App(_) => {
                        s.selected = row;
                        drop(s);
                        let _ = wnd.hwnd().InvalidateRect(None, false);
                        let _ = wnd.hwnd().UpdateWindow();
                        // The menu runs its own message loop, so the state must not be borrowed.
                        match app_menu(wnd.hwnd()) {
                            Some(APP_OPEN) => act(&state, false),
                            Some(APP_ADMIN) => act(&state, true),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
    });

    wnd.on().wm_mouse_wheel({
        let (wnd, state) = (wnd.clone(), state.clone());
        move |p| {
            let mut s = state.borrow_mut();
            if s.config().ignore_mouse {
                return Ok(());
            }
            let visible = s.layout(wnd.hwnd().GetClientRect()?).visible;
            let rows = -isize::from(p.wheel_distance) / 120 * 3;
            s.scroll_by(rows, visible);
            drop(s);
            let _ = wnd.hwnd().InvalidateRect(None, false);
            Ok(())
        }
    });

    if let Err(e) = wnd.run_main(Some(co::SW::HIDE)) {
        tracing::error!("launcher message loop failed: {e}");
    }
    hwnd_slot.store(0, Ordering::Release);
}

/// Menu ids of an app's right click menu.
const APP_OPEN: i32 = 1;
const APP_ADMIN: i32 = 2;

/// Shows the menu of an app row at the pointer and returns what was chosen.
fn app_menu(hwnd: &w::HWND) -> Option<i32> {
    let mut menu = w::HMENU::CreatePopupMenu().ok()?;
    let built = menu.append_item(&[
        w::MenuItem::Entry {
            cmd_id: APP_OPEN as u16,
            text: "Open",
        },
        w::MenuItem::Entry {
            cmd_id: APP_ADMIN as u16,
            text: "Run as administrator",
        },
    ]);
    let chosen = built.ok().and_then(|()| {
        let at = w::GetCursorPos().ok()?;
        menu.TrackPopupMenu(co::TPM::LEFTBUTTON | co::TPM::RETURNCMD, at, hwnd)
            .ok()
            .flatten()
    });
    let _ = menu.DestroyMenu();
    chosen
}

/// Puts `text` on the clipboard.
fn copy(hwnd: &w::HWND, text: &str) -> w::SysResult<()> {
    let clipboard = hwnd.OpenClipboard()?;
    clipboard.EmptyClipboard()?;
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    clipboard.SetClipboardData(co::CF::UNICODETEXT, &bytes)
}

fn fill_round(dc: &w::HDC, bounds: w::RECT, radius: i32, color: Color) -> w::SysResult<()> {
    let brush = w::HBRUSH::CreateSolidBrush(colorref(color))?;
    let pen = w::HPEN::GetStockObject(co::STOCK_PEN::NULL)?;
    let _brush = dc.SelectObject(&*brush)?;
    let _pen = dc.SelectObject(&pen)?;
    // RoundRect leaves out the bottom and right edges with a null pen.
    let bounds = w::RECT {
        right: bounds.right + 1,
        bottom: bounds.bottom + 1,
        ..bounds
    };
    dc.RoundRect(bounds, w::SIZE::with(radius, radius))
}

fn text(
    dc: &w::HDC,
    font: &w::HFONT,
    color: Color,
    s: &str,
    mut bounds: w::RECT,
    flags: co::DT,
) -> w::SysResult<()> {
    let _font = dc.SelectObject(font)?;
    dc.SetTextColor(colorref(color))?;
    dc.DrawText(
        s,
        &mut bounds,
        flags | co::DT::SINGLELINE | co::DT::NOPREFIX | co::DT::END_ELLIPSIS,
    )?;
    Ok(())
}

fn paint(s: &mut State, dc: &w::HDC, client: w::RECT) -> w::SysResult<()> {
    s.fonts();
    let fonts = s.fonts.take().expect("fonts were just made");
    let result = paint_with(s, &fonts, dc, client);
    s.fonts = Some(fonts);
    result
}

fn paint_with(s: &mut State, fonts: &Fonts, dc: &w::HDC, client: w::RECT) -> w::SysResult<()> {
    let config = s.config().clone();
    let style = s.settings.style.clone();
    let layout = s.layout_with(fonts, client);
    let radius = s.scale(16);
    let pad = s.scale(12);
    let background = w::HBRUSH::CreateSolidBrush(colorref(style.background))?;
    dc.FillRect(client, &background)?;
    dc.SetBkMode(co::BKMODE::TRANSPARENT)?;

    // The search box.
    let field = mix(style.background, style.foreground, 20);
    fill_round(dc, layout.search, radius, field)?;
    let mut inner = layout.search;
    inner.left += pad;
    inner.right -= pad;
    let (main, small) = (&*fonts.main, &*fonts.small);
    let left = co::DT::LEFT | co::DT::VCENTER;
    if s.query.text().is_empty() {
        let placeholder = s
            .panel
            .map_or_else(|| config.placeholder_text(), |g| g.title().to_owned());
        text(dc, main, style.dimmed, &placeholder, inner, left)?;
    } else {
        text(dc, main, style.foreground, s.query.text(), inner, left)?;
    }
    let typed = {
        let _font = dc.SelectObject(main)?;
        dc.GetTextExtentPoint32(s.query.before_caret())?.cx
    };
    let caret_x = (inner.left + typed + 1).min(inner.right);
    let caret_h = fonts.main_px;
    let mid = (inner.top + inner.bottom) / 2;
    let caret = w::RECT {
        left: caret_x,
        top: mid - caret_h / 2,
        right: caret_x + s.scale(2).max(1),
        bottom: mid + caret_h / 2,
    };
    let caret_brush = w::HBRUSH::CreateSolidBrush(colorref(style.selected))?;
    dc.FillRect(caret, &caret_brush)?;

    // The tray icons: a row of their own, at the bottom.
    if layout.strip.bottom > layout.strip.top {
        let cell = layout.cell;
        let hover = mix(style.background, style.foreground, 25);
        let mut end = layout.tray_x;
        let tray = s.tray.clone();
        for (i, item) in tray.iter().enumerate() {
            let x = layout.tray_x + i as i32 * cell;
            if x + cell > layout.strip.right {
                break;
            }
            end = x + cell;
            let slot = w::RECT {
                left: x,
                right: x + cell,
                ..layout.strip
            };
            if s.tray_hover == Some(i) {
                fill_round(dc, slot, s.scale(10), hover)?;
            }
            let size = layout.tray_icon;
            let (ix, iy) = (
                x + (cell - size) / 2,
                slot.top + (slot.bottom - slot.top - size) / 2,
            );
            // A tray icon is usually 16 or 32 pixels, small for the size shown. One big enough is
            // shrunk smoothly; a smaller one gives way to the program's own icon, which Windows
            // has at larger sizes, and failing that is grown smoothly rather than stretched.
            let hdc = dc.ptr() as isize;
            let big_enough = item.icon.as_ref().filter(|_| item.icon_size >= size);
            if let Some(icon) = big_enough.and_then(|image| s.tray_icon_at(image, size)) {
                icon.draw(hdc, ix, iy, size);
            } else if let Some(icon) = (!item.path.is_empty())
                .then(|| s.file_icon(&item.path, size))
                .flatten()
            {
                icon.draw(hdc, ix, iy, size);
            } else if let Some(icon) = item.icon.as_ref().and_then(|i| s.tray_icon_at(i, size)) {
                icon.draw(hdc, ix, iy, size);
            } else {
                let initial: String = item.name.chars().take(1).collect();
                let centred = co::DT::CENTER | co::DT::VCENTER;
                text(dc, small, style.dimmed, &initial, slot, centred)?;
            }
        }
        if let Some(item) = s.tray_hover.and_then(|i| s.tray.get(i)) {
            let bounds = w::RECT {
                left: end + pad,
                ..layout.strip
            };
            text(dc, small, style.dimmed, &item.name, bounds, left)?;
        }
    }

    // The bar: the clock and date, and the power buttons.
    if layout.bar.bottom > layout.bar.top {
        let t = w::GetLocalTime();
        let time = format_time(t.wHour, t.wMinute, t.wSecond);
        let date = format_date(t.wYear, t.wMonth, t.wDay, t.wDayOfWeek);
        let bar = layout.bar;
        let inset = s.scale(6);
        let bold = &*fonts.bold;
        let time_width = {
            let _font = dc.SelectObject(bold)?;
            dc.GetTextExtentPoint32(&time)?.cx
        };
        let time_area = w::RECT {
            left: bar.left + inset,
            ..bar
        };
        text(dc, bold, style.foreground, &time, time_area, left)?;
        let buttons_left = layout.buttons.first().map_or(bar.right, |b| b.left);
        let date_area = w::RECT {
            left: bar.left + inset + time_width + pad,
            right: buttons_left - pad,
            ..bar
        };
        text(dc, &fonts.date, style.dimmed, &date, date_area, left)?;
        let idle = mix(style.background, style.foreground, 20);
        let hovered = mix(style.background, style.foreground, 40);
        for (i, ((glyph, action), rect)) in BAR_BUTTONS.iter().zip(&layout.buttons).enumerate() {
            // A switch is lit while on and dim while off or missing; a power button is lit
            // while it waits to be confirmed.
            let active = match action {
                BarAction::Panel(group) => group.is_on(&s.status) == Some(true),
                BarAction::Power(power) => s.armed == Some(*power),
            };
            let off = matches!(action, BarAction::Panel(_)) && !active;
            let (fill, colour) = if active {
                (style.selected, style.selected_foreground)
            } else if s.bar_hover == Some(i) {
                (hovered, if off { style.dimmed } else { style.foreground })
            } else {
                (idle, if off { style.dimmed } else { style.foreground })
            };
            fill_round(dc, *rect, s.scale(10), fill)?;
            let centred = co::DT::CENTER | co::DT::VCENTER;
            text(dc, &fonts.icons, colour, &glyph.to_string(), *rect, centred)?;
            // A line under the button whose panel is open.
            if matches!(action, BarAction::Panel(group) if s.panel == Some(*group)) {
                let inset = (rect.right - rect.left) / 4;
                let line = w::RECT {
                    left: rect.left + inset,
                    top: rect.bottom - s.scale(4),
                    right: rect.right - inset,
                    bottom: rect.bottom - s.scale(2),
                };
                fill_round(dc, line, s.scale(2), colour)?;
            }
        }
    }

    // The results.
    if s.results.is_empty() {
        let message = if s.loading {
            "Loading apps…"
        } else {
            "No matches"
        };
        text(
            dc,
            main,
            style.dimmed,
            message,
            layout.list,
            co::DT::CENTER | co::DT::VCENTER,
        )?;
        return Ok(());
    }
    let end = (s.scroll + layout.visible).min(s.results.len());
    for (slot, index) in (s.scroll..end).enumerate() {
        let (title, subtitle, icon) = s.describe(&s.results[index]);
        let top = layout.list.top + slot as i32 * layout.row;
        let row = w::RECT {
            left: layout.list.left,
            top,
            right: layout.list.right,
            bottom: top + layout.row,
        };
        let selected = index == s.selected;
        let (fg, dim) = if selected {
            fill_round(dc, row, radius, style.selected)?;
            (style.selected_foreground, style.selected_foreground)
        } else {
            (style.foreground, style.dimmed)
        };
        let mut x = row.left + pad;
        if layout.icon > 0 {
            let y = top + (layout.row - layout.icon) / 2;
            match icon {
                Some(RowIcon::App(id)) => {
                    if let Some(icon) = s.icon(&id, layout.icon) {
                        icon.draw(dc.ptr() as isize, x, y, layout.icon);
                    }
                }
                Some(RowIcon::Tray(image)) => {
                    image.draw(dc.ptr() as isize, x, y, layout.icon);
                }
                None => {}
            }
            x += layout.icon + pad;
        }
        let bounds = w::RECT {
            left: x,
            right: row.right - pad,
            ..row
        };
        if config.compact {
            text(dc, main, fg, &title, bounds, left)?;
        } else {
            let (main_px, small_px) = (fonts.main_px, fonts.small_px);
            let spacing = s.scale(2);
            let block = main_px + spacing + small_px;
            let name_top = top + (layout.row - block) / 2;
            let name = w::RECT {
                top: name_top,
                bottom: name_top + main_px + spacing,
                ..bounds
            };
            let id = w::RECT {
                top: name.bottom,
                bottom: name.bottom + small_px + spacing,
                ..bounds
            };
            text(dc, main, fg, &title, name, co::DT::LEFT | co::DT::TOP)?;
            text(dc, small, dim, &subtitle, id, co::DT::LEFT | co::DT::TOP)?;
        }
    }
    Ok(())
}
