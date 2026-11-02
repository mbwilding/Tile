//! The app launcher: a search box over everything Start lists (`shell:AppsFolder`), centred
//! on a monitor. It runs on its own thread, and its window is created hidden up front so it
//! opens at once.

mod layout;
mod paint;
mod popups;
mod state;

use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use regex::RegexSet;
use tile_config::{Launcher as Config, LauncherStyle};
use tile_core::{Command, Rect};
use tile_platform::{
    Mailbox, WindowThread, focus_hwnd, init_com, is_shell_window, list_apps, paint_buffered, rect,
    scale, set_alpha, style_popup,
};
use tile_shell::notify::{Click, ItemKey, Registry};
use tokio::sync::mpsc::UnboundedSender;
use winsafe::{self as w, co, gui, prelude::*};

use self::{
    layout::{BAR_BUTTONS, BarAction, Layout, bar_width},
    paint::{Resources, paint},
    popups::{
        FIX_WINDOW, MENU_GRACE, MENU_POLL_MS, MENU_TIMER, menu_active, menu_open,
        move_corner_popups,
    },
    state::{Hit, Picked, Row, State, cursor},
};
use crate::{
    search::App,
    status,
    system::{self, Action, Group},
};

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
        /// Which icon.
        key: ItemKey,
        /// Which button.
        click: Click,
        /// Where the pointer was, for the menu the app may show.
        at: (i32, i32),
    },
    /// Carry out a system item's action.
    System(Action),
    /// The launcher is about to close itself (not because focus went elsewhere), so Windows is
    /// about to hand focus to some other window.
    Closing,
}

/// The `launcher` config section with its `hide` patterns compiled and its look resolved.
#[derive(Debug, Clone)]
pub struct LauncherSettings {
    /// The section as written.
    pub config: Config,
    /// The `hide` patterns: an app whose name or id matches one isn't listed.
    pub hide: RegexSet,
    /// The font, corners and colours, resolved against the theme.
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
pub struct Launcher(WindowThread<Request>);

impl Launcher {
    /// Starts the launcher thread. Launch history is kept in `usage_path`.
    pub fn spawn(
        events: UnboundedSender<LauncherEvent>,
        settings: LauncherSettings,
        usage_path: PathBuf,
        tray: Registry,
    ) -> Self {
        Self(WindowThread::spawn("launcher", move |requests, mailbox| {
            run(events, settings, usage_path, tray, requests, mailbox);
        }))
    }

    /// Opens the launcher on `fallback` (Tile's focused monitor's work area, which is right
    /// even when that monitor has no windows), or else on the focused window's monitor, or
    /// else on the monitor under the pointer. Closes it if it is open.
    pub fn toggle(&self, fallback: Option<Rect>) {
        self.0.send(Request::Toggle(fallback));
    }

    /// Applies new settings, to the open launcher too.
    pub fn set_settings(&self, settings: LauncherSettings) {
        self.0.send(Request::Settings(Box::new(settings)));
    }

    /// Closes the window and stops the thread.
    pub fn shutdown(&mut self) {
        self.0.shutdown(Request::Quit);
    }
}

/// Repaints the bar's clock while the launcher is open.
const CLOCK_TIMER: usize = 3;
const CLOCK_MS: u32 = 1000;
/// Reads the system again a moment after something in a panel was changed, so what it shows
/// catches up with what the change did.
const STATUS_TIMER: usize = 4;
const STATUS_DELAY_MS: u32 = 400;
/// A wheel notch, and the rows it scrolls.
const WHEEL_DELTA: isize = 120;
const WHEEL_ROWS: isize = 3;

/// Menu ids of an app's right click menu.
const APP_OPEN: i32 = 1;
const APP_ADMIN: i32 = 2;

/// Has the window repainted: at once with `now`, so a resize and its new contents reach the
/// screen together, else when Windows next gets to it.
fn redraw(hwnd: &w::HWND, now: bool) {
    let _ = hwnd.InvalidateRect(None, false);
    if now {
        let _ = hwnd.UpdateWindow();
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
            .filter(|hwnd| !is_shell_window(hwnd.ptr() as isize))
            .and_then(|hwnd| work_area(hwnd.MonitorFromWindow(co::MONITOR::DEFAULTTONEAREST)))
    };
    preferred.or_else(focused).or_else(|| {
        work_area(w::HMONITOR::MonitorFromPoint(
            w::GetCursorPos().unwrap_or_default(),
            co::MONITOR::DEFAULTTONEAREST,
        ))
    })
}

fn style_window(hwnd: &w::HWND, settings: &LauncherSettings) {
    let (config, style) = (&settings.config, &settings.style);
    style_popup(hwnd, style.corners, style.border);
    let alpha = (config.opacity.clamp(0.1, 1.0) * 255.0).round() as u8;
    if let Err(e) = set_alpha(hwnd, alpha) {
        tracing::warn!("launcher opacity: {e}");
    }
}

/// Makes the open window as tall as its content, keeping its top where it is.
fn fit_window(wnd: &gui::WindowMain, s: &State) {
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

/// The layout for handling a mouse message, or `None` while the mouse is ignored.
fn mouse_layout(s: &State, hwnd: &w::HWND) -> Option<Layout> {
    if s.config().ignore_mouse {
        return None;
    }
    hwnd.GetClientRect().ok().map(|client| s.layout(client))
}

/// Returns a function that calls `read` on a thread called `name` and sends the window what it
/// returns. Reading waits on Windows, so it is kept off the window's thread; a call while the
/// last read is still going does nothing.
fn background_reader(
    name: &'static str,
    mailbox: Mailbox<Request>,
    read: fn() -> Request,
) -> impl Fn() + Clone + 'static {
    let busy = Arc::new(AtomicBool::new(false));
    move || {
        if busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (mailbox, done) = (mailbox.clone(), busy.clone());
        let spawned = thread::Builder::new().name(name.into()).spawn(move || {
            let request = read();
            done.store(false, Ordering::Release);
            mailbox.send(request);
        });
        if let Err(e) = spawned {
            busy.store(false, Ordering::Release);
            tracing::warn!("couldn't start the {name} thread: {e}");
        }
    }
}

fn run(
    events: UnboundedSender<LauncherEvent>,
    settings: LauncherSettings,
    usage_path: PathBuf,
    registry: Registry,
    requests: mpsc::Receiver<Request>,
    mailbox: Mailbox<Request>,
) {
    // For the icons, which come from the shell.
    let _com = init_com();
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
    let closing = events.clone();
    let state = Rc::new(RefCell::new(State::new(
        settings, events, usage_path, registry,
    )));
    let resources = Rc::new(RefCell::new(Resources::default()));
    let refresh_apps = background_reader("launcher apps", mailbox.clone(), || {
        Request::Apps(list_apps())
    });
    let refresh_status = background_reader("launcher system", mailbox.clone(), || {
        Request::System(status::status())
    });

    wnd.on().wm_create({
        let (wnd, state, mailbox) = (wnd.clone(), state.clone(), mailbox.clone());
        let (refresh_apps, refresh_status) = (refresh_apps.clone(), refresh_status.clone());
        move |_| {
            style_window(wnd.hwnd(), &state.borrow().settings);
            mailbox.attach(wnd.hwnd());
            refresh_apps();
            // So the first time it opens already knows the system.
            refresh_status();
            Ok(0)
        }
    });

    // `announce` is false when focus already went elsewhere, which is the user's doing.
    let close = {
        let (wnd, state) = (wnd.clone(), state.clone());
        Rc::new(move |announce: bool| {
            // Leave a fresh frame behind, or the next open flashes this session's selection
            // before it paints.
            if !wnd.hwnd().IsWindowVisible() {
                return;
            }
            let reset = state.try_borrow_mut().map(|mut s| s.reset()).is_ok();
            if reset {
                redraw(wnd.hwnd(), true);
            }
            // Before hiding, so the daemon knows by the time focus moves.
            if announce {
                let _ = closing.send(LauncherEvent::Closing);
            }
            wnd.hwnd().ShowWindow(co::SW::HIDE);
        })
    };
    let hide = {
        let close = close.clone();
        move || close(true)
    };

    let toggle = {
        let (wnd, state, resources, hide) =
            (wnd.clone(), state.clone(), resources.clone(), hide.clone());
        let (refresh_apps, refresh_status) = (refresh_apps.clone(), refresh_status.clone());
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
            let dpi = tile_shell::monitor_dpi(cx, cy);
            let mut res = resources.borrow_mut();
            if dpi != s.dpi {
                s.dpi = dpi;
                res.icons.clear();
            }
            // The icons first, since they decide how wide the window must be to hold them.
            s.tray = s.registry.items();
            res.icons.forget_tray();
            drop(res);
            let tray_width = s.layout(w::RECT::default()).tray_width(s.tray.len());
            let bar_width = if s.config().bar {
                bar_width(s.font_sizes(), dpi)
            } else {
                0
            };
            let w = scale(s.config().width.clamp(200, 4000) as i32, dpi)
                .max(tray_width)
                .max(bar_width)
                .min(area.w);
            s.reset();
            // Centred for the full number of rows, so it keeps its top as it shrinks.
            let top = cy - s.height_for(s.max_rows()).min(area.h) / 2;
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
            redraw(hwnd, true);
            // The bar's clock shows seconds.
            let _ = hwnd.SetTimer(CLOCK_TIMER, CLOCK_MS, None);
            if !focus_hwnd(hwnd) {
                tracing::debug!("the launcher didn't get focus");
            }
            refresh_apps();
            refresh_status();
        }
    };

    wnd.on().wm(co::WM::APP, {
        let (wnd, state, resources) = (wnd.clone(), state.clone(), resources.clone());
        move |_| {
            let mut changed = false;
            while let Ok(request) = requests.try_recv() {
                match request {
                    Request::Toggle(fallback) => toggle(fallback),
                    Request::Settings(settings) => {
                        let mut s = state.borrow_mut();
                        s.settings = *settings;
                        resources.borrow_mut().icons.clear();
                        s.filter();
                        style_window(wnd.hwnd(), &s.settings);
                        fit_window(&wnd, &s);
                        changed = true;
                    }
                    Request::Apps(apps) => {
                        let mut s = state.borrow_mut();
                        s.loading = false;
                        if apps != s.all {
                            s.all = apps;
                            s.filter();
                            fit_window(&wnd, &s);
                            changed = true;
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
                                fit_window(&wnd, &s);
                            }
                            changed = true;
                        }
                    }
                    Request::Quit => {
                        resources.borrow_mut().icons.clear();
                        w::PostQuitMessage(0);
                    }
                }
            }
            if changed {
                redraw(wnd.hwnd(), wnd.hwnd().IsWindowVisible());
            }
            Ok(0)
        }
    });

    wnd.on().wm_timer(STATUS_TIMER, {
        let (wnd, refresh_status) = (wnd.clone(), refresh_status.clone());
        move || {
            let _ = wnd.hwnd().KillTimer(STATUS_TIMER);
            refresh_status();
            Ok(())
        }
    });

    wnd.on().wm_timer(CLOCK_TIMER, {
        let wnd = wnd.clone();
        move || {
            if wnd.hwnd().IsWindowVisible() {
                redraw(wnd.hwnd(), false);
            } else {
                let _ = wnd.hwnd().KillTimer(CLOCK_TIMER);
            }
            Ok(())
        }
    });

    wnd.on().wm_activate({
        let (close, state) = (close.clone(), state.clone());
        move |p| {
            // The menu of a tray icon takes the focus: the launcher stays until it closes.
            if p.event == co::WA::INACTIVE
                && !state.borrow().menu.as_ref().is_some_and(|m| m.keep_open)
            {
                close(false);
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
            if menu_open(&watch.before) {
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
                    && !is_shell_window(f.ptr() as isize)
            });
            if elsewhere {
                hide();
            } else if !menu_active() {
                // A menu neither it nor `menu_open` noticed may still be open; reclaiming focus
                // would close it, so only do that when nothing looks like a menu is running.
                focus_hwnd(hwnd);
                redraw(hwnd, false);
            }
            Ok(())
        }
    });

    wnd.on().wm_paint({
        let (wnd, state, resources) = (wnd.clone(), state.clone(), resources.clone());
        move || {
            paint_buffered(wnd.hwnd(), |dc, client| {
                paint(&state.borrow(), &mut resources.borrow_mut(), dc, client)
            })?;
            Ok(())
        }
    });

    // Clicks a tray icon: the app may show a window, a menu or a popup. A right click's menu
    // opens over the launcher, which stays until it closes; otherwise the launcher closes.
    let send_tray_click = {
        let (wnd, state, hide) = (wnd.clone(), state.clone(), hide.clone());
        move |key: ItemKey, click: Click, at: (i32, i32)| {
            // So the app can show its window or menu in front.
            tile_shell::notify::allow_foreground();
            let keep_open = click == Click::Right;
            let mut s = state.borrow_mut();
            s.menu = Some(popups::watch_menu(&wnd, key.hwnd, at, keep_open));
            let events = s.events.clone();
            drop(s);
            if !keep_open {
                hide();
            }
            let _ = events.send(LauncherEvent::Tray { key, click, at });
        }
    };

    // Clicks the tray icon at `index` of the strip.
    let click_tray = {
        let (state, send_tray_click) = (state.clone(), send_tray_click.clone());
        move |index: usize, click: Click| {
            let key = state.borrow().tray.get(index).map(|item| item.key);
            if let Some(key) = key {
                send_tray_click(key, click, cursor());
            }
        }
    };

    // Opens a panel in place of the results, or with `None` goes back to them.
    let set_panel = {
        let (wnd, state) = (wnd.clone(), state.clone());
        move |panel: Option<Group>| {
            let mut s = state.borrow_mut();
            s.panel = panel;
            s.query.clear();
            s.rank();
            fit_window(&wnd, &s);
            drop(s);
            redraw(wnd.hwnd(), true);
        }
    };

    // Does what picking the selected row asks for.
    let act = {
        let (wnd, state, hide) = (wnd.clone(), state.clone(), hide.clone());
        move |secondary: bool| {
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
                        let _ = wnd.hwnd().SetTimer(STATUS_TIMER, STATUS_DELAY_MS, None);
                        return;
                    }
                    if let LauncherEvent::Tray { key, click, at } = event {
                        send_tray_click(key, click, at);
                        return;
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
                    redraw(wnd.hwnd(), false);
                }
                Picked::Nothing => {}
            }
        }
    };

    // A button of the bar. A click on a labelled button is deliberate, so power buttons act at
    // once; only the keyboard rows ask for Enter twice.
    let click_bar = {
        let (wnd, state, hide, set_panel) =
            (wnd.clone(), state.clone(), hide.clone(), set_panel.clone());
        let refresh_status = refresh_status.clone();
        move |index: usize, right: bool| {
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
                        let _ = wnd.hwnd().SetTimer(STATUS_TIMER, STATUS_DELAY_MS, None);
                        redraw(wnd.hwnd(), false);
                    }
                    return;
                }
                BarAction::Panel(group) => {
                    // The same button again goes back to the results.
                    let panel = (s.panel != Some(group)).then_some(group);
                    drop(s);
                    set_panel(panel);
                    // So it shows what things are now.
                    refresh_status();
                    return;
                }
                BarAction::Power(power) => power,
            };
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
                    let in_panel = s.panel.is_some();
                    drop(s);
                    if in_panel {
                        set_panel(None);
                    } else {
                        hide();
                    }
                    return Ok(());
                }
                co::VK::RETURN => {
                    drop(s);
                    act(shift);
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
                fit_window(&wnd, &s);
            }
            drop(s);
            redraw(wnd.hwnd(), true);
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
            fit_window(&wnd, &s);
            drop(s);
            redraw(wnd.hwnd(), true);
            Ok(())
        }
    });

    wnd.on().wm_mouse_move({
        let (wnd, state) = (wnd.clone(), state.clone());
        move |p| {
            let mut s = state.borrow_mut();
            let Some(layout) = mouse_layout(&s, wnd.hwnd()) else {
                return Ok(());
            };
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
            let hit = s.hit(&layout, at.0, at.1);
            let tray_hover = match hit {
                Some(Hit::Tray(i)) => Some(i),
                _ => None,
            };
            let bar_hover = match hit {
                Some(Hit::Bar(i)) => Some(i),
                _ => None,
            };
            let mut changed = tray_hover != s.tray_hover || bar_hover != s.bar_hover;
            s.tray_hover = tray_hover;
            s.bar_hover = bar_hover;
            if let Some(Hit::Row(row)) = hit
                && row != s.selected
            {
                s.selected = row;
                changed = true;
            }
            drop(s);
            if changed {
                redraw(wnd.hwnd(), false);
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
            let s = state.borrow();
            let hit = mouse_layout(&s, wnd.hwnd())
                .and_then(|layout| s.hit(&layout, p.coords.x, p.coords.y));
            drop(s);
            match hit {
                Some(Hit::Tray(index)) => click_tray(index, Click::Left),
                Some(Hit::Bar(index)) => click_bar(index, false),
                Some(Hit::Row(row)) => {
                    state.borrow_mut().selected = row;
                    act(false);
                }
                None => {}
            }
            Ok(())
        }
    });

    // The right button opens a tray icon's menu, or an app's.
    wnd.on().wm_r_button_down({
        let (wnd, state) = (wnd.clone(), state.clone());
        move |p| {
            let mut s = state.borrow_mut();
            let hit = mouse_layout(&s, wnd.hwnd())
                .and_then(|layout| s.hit(&layout, p.coords.x, p.coords.y));
            match hit {
                Some(Hit::Tray(index)) => {
                    drop(s);
                    click_tray(index, Click::Right);
                }
                Some(Hit::Bar(index)) => {
                    drop(s);
                    click_bar(index, true);
                }
                Some(Hit::Row(row)) => match s.results[row] {
                    Row::Tray(_) => {
                        s.selected = row;
                        drop(s);
                        act(true);
                    }
                    Row::App(_) => {
                        s.selected = row;
                        drop(s);
                        redraw(wnd.hwnd(), true);
                        // The menu runs its own message loop, so the state must not be borrowed.
                        match app_menu(wnd.hwnd()) {
                            Some(APP_OPEN) => act(false),
                            Some(APP_ADMIN) => act(true),
                            _ => {}
                        }
                    }
                    _ => {}
                },
                None => {}
            }
            Ok(())
        }
    });

    wnd.on().wm_mouse_wheel({
        let (wnd, state) = (wnd.clone(), state.clone());
        move |p| {
            let mut s = state.borrow_mut();
            let Some(layout) = mouse_layout(&s, wnd.hwnd()) else {
                return Ok(());
            };
            let rows = -isize::from(p.wheel_distance) / WHEEL_DELTA * WHEEL_ROWS;
            s.scroll_by(rows, layout.visible);
            drop(s);
            redraw(wnd.hwnd(), false);
            Ok(())
        }
    });

    if let Err(e) = wnd.run_main(Some(co::SW::HIDE)) {
        tracing::error!("launcher message loop failed: {e}");
    }
    mailbox.detach();
}

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
