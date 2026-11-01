//! The tray icon and its menu, and the hidden window that hears about display and session
//! changes.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc,
    },
};

use crate::{
    gui::{Mailbox, WindowThread},
    tile_icon,
};
use tokio::sync::mpsc;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};
use winsafe::{self as w, co, gui, prelude::*};

/// Something that happened on the tray / message thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Pause or resume the window manager.
    ToggleTile,
    /// Pause or resume the key remapper.
    ToggleRemap,
    /// Make Tile the shell, or stop.
    ToggleShell,
    /// Switch logging to a file on or off.
    ToggleLogging,
    /// Open the config file.
    OpenConfig,
    /// Open the log folder.
    OpenLogs,
    /// Quit Tile.
    Quit,
    /// Monitors or the work area changed.
    DisplayChanged,
    /// The session was locked, unlocked or switched.
    SessionChanged,
}

/// What the tray thread is woken for.
enum Request {
    /// The check marks or the config error changed.
    Sync,
    /// A menu item was clicked.
    Clicked(MenuId),
    /// Remove the icon and end the thread.
    Quit,
}

/// The tray icon, living on a dedicated thread with a hidden window and message loop.
pub struct Tray {
    thread: WindowThread<Request>,
    checks: Arc<Checks>,
}

/// What the menu's check marks should show, set by the daemon.
#[derive(Default)]
struct Checks {
    tile_paused: AtomicBool,
    remap_paused: AtomicBool,
    shell: AtomicBool,
    logging: AtomicBool,
    /// Why the config file couldn't be loaded, while it can't.
    config_error: Mutex<Option<String>>,
}

/// The longest error shown in the menu, in characters.
const MENU_ERROR_LEN: usize = 60;
/// Windows cuts tray tooltips off at 127 characters.
const TOOLTIP_LEN: usize = 127;

/// `text` cut to `max` characters, with an ellipsis when it was longer.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The 32x32 tray icon, red while the config has errors.
fn icon(error: bool) -> Option<Icon> {
    Icon::from_rgba(tile_icon::rgba(32, error), 32, 32).ok()
}

struct Items {
    menu: Menu,
    /// Shown first while the config has errors; opens the config.
    error: MenuItem,
    error_separator: PredefinedMenuItem,
    /// The error the menu, icon and tooltip show now.
    shown_error: Option<String>,
    /// Checked while the window manager runs (not paused).
    tile: CheckMenuItem,
    /// Checked while the key remapper runs (not paused).
    remap: CheckMenuItem,
    /// Checked while Tile is the shell.
    shell: CheckMenuItem,
    log: CheckMenuItem,
    open_logs: MenuId,
    open_config: MenuId,
    quit: MenuId,
}

impl Items {
    fn sync(&mut self, tray: &TrayIcon, checks: &Checks) {
        let set = |item: &CheckMenuItem, want: bool| {
            if item.is_checked() != want {
                item.set_checked(want);
            }
        };
        set(&self.tile, !checks.tile_paused.load(Ordering::Relaxed));
        set(&self.remap, !checks.remap_paused.load(Ordering::Relaxed));
        set(&self.shell, checks.shell.load(Ordering::Relaxed));
        set(&self.log, checks.logging.load(Ordering::Relaxed));

        let error = checks
            .config_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if error == self.shown_error {
            return;
        }
        match (&self.shown_error, &error) {
            (None, Some(_)) => {
                let _ = self
                    .menu
                    .prepend_items(&[&self.error, &self.error_separator]);
            }
            (Some(_), None) => {
                let _ = self.menu.remove(&self.error);
                let _ = self.menu.remove(&self.error_separator);
            }
            _ => {}
        }
        let tooltip = match &error {
            Some(e) => {
                self.error
                    .set_text(format!("Config error: {}", truncate(e, MENU_ERROR_LEN)));
                truncate(&format!("Tile: config error\n{e}"), TOOLTIP_LEN)
            }
            None => "Tile".to_owned(),
        };
        let _ = tray.set_tooltip(Some(tooltip));
        let _ = tray.set_icon(icon(error.is_some()));
        self.shown_error = error;
    }
}

fn build(checks: &Checks) -> Result<(TrayIcon, Items), String> {
    let tile = CheckMenuItem::new("Tile", true, true, None);
    let remap = CheckMenuItem::new("Remap", true, true, None);
    let shell = CheckMenuItem::new("Shell", true, false, None);
    let log = CheckMenuItem::new("Log", true, false, None);
    let open_logs = MenuItem::new("Open logs", true, None);
    let open_config = MenuItem::new("Open config", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &tile,
        &remap,
        &shell,
        &log,
        &PredefinedMenuItem::separator(),
        &open_logs,
        &open_config,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| e.to_string())?;
    let mut builder = TrayIconBuilder::new()
        .with_tooltip("Tile")
        .with_menu(Box::new(menu.clone()));
    if let Some(icon) = icon(false) {
        builder = builder.with_icon(icon);
    }
    let tray = builder.build().map_err(|e| e.to_string())?;
    let mut items = Items {
        menu,
        error: MenuItem::new("Config error", true, None),
        error_separator: PredefinedMenuItem::separator(),
        shown_error: None,
        open_logs: open_logs.id().clone(),
        open_config: open_config.id().clone(),
        quit: quit.id().clone(),
        tile,
        remap,
        shell,
        log,
    };
    items.sync(&tray, checks);
    Ok((tray, items))
}

impl Tray {
    /// Starts the tray thread, which sends what happens there to `tx`. If the thread can't
    /// start, Tile runs without a tray.
    ///
    /// Menu clicks reach the thread through `tray-icon`'s one process-wide handler, so start
    /// only one tray per process.
    pub fn spawn(tx: mpsc::UnboundedSender<TrayEvent>) -> Self {
        let checks = Arc::new(Checks::default());
        let state = checks.clone();
        let thread = WindowThread::spawn("tray", move |requests, mailbox| {
            run(&tx, &state, requests, &mailbox);
        });
        Self { thread, checks }
    }

    fn sync(&self) {
        self.thread.send(Request::Sync);
    }

    /// Reflects the window manager's pause state in the menu.
    pub fn set_tile_paused(&self, paused: bool) {
        self.checks.tile_paused.store(paused, Ordering::Relaxed);
        self.sync();
    }

    /// Reflects the key remapper's pause state in the menu.
    pub fn set_remap_paused(&self, paused: bool) {
        self.checks.remap_paused.store(paused, Ordering::Relaxed);
        self.sync();
    }

    /// Reflects whether Tile is the shell in the menu.
    pub fn set_shell(&self, shell: bool) {
        self.checks.shell.store(shell, Ordering::Relaxed);
        self.sync();
    }

    /// Reflects whether logging is on in the menu.
    pub fn set_logging(&self, logging: bool) {
        self.checks.logging.store(logging, Ordering::Relaxed);
        self.sync();
    }

    /// Shows that the config file has errors (and what they are) or, with `None`, that it
    /// loaded.
    pub fn set_config_error(&self, error: Option<String>) {
        *self
            .checks
            .config_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
        self.sync();
    }

    /// Removes the icon and stops the tray thread.
    pub fn shutdown(&mut self) {
        self.thread.shutdown(Request::Quit);
    }
}

impl Items {
    /// The event for a click on the menu item `id`.
    fn event(&self, id: &MenuId) -> Option<TrayEvent> {
        Some(if id == self.tile.id() {
            TrayEvent::ToggleTile
        } else if id == self.remap.id() {
            TrayEvent::ToggleRemap
        } else if id == self.shell.id() {
            TrayEvent::ToggleShell
        } else if id == self.log.id() {
            TrayEvent::ToggleLogging
        } else if *id == self.open_config || id == self.error.id() {
            TrayEvent::OpenConfig
        } else if *id == self.open_logs {
            TrayEvent::OpenLogs
        } else if *id == self.quit {
            TrayEvent::Quit
        } else {
            return None;
        })
    }
}

fn run(
    tx: &mpsc::UnboundedSender<TrayEvent>,
    checks: &Arc<Checks>,
    requests: std_mpsc::Receiver<Request>,
    mailbox: &Mailbox<Request>,
) {
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Tile",
        class_name: "tile-message-window",
        ..Default::default()
    });
    let state: Rc<RefCell<Option<(TrayIcon, Items)>>> = Rc::new(RefCell::new(None));

    MenuEvent::set_event_handler(Some({
        let mailbox = mailbox.clone();
        move |event: MenuEvent| mailbox.send(Request::Clicked(event.id))
    }));

    wnd.on().wm_create({
        let (wnd, state, checks, mailbox) =
            (wnd.clone(), state.clone(), checks.clone(), mailbox.clone());
        move |_| {
            match build(&checks) {
                Ok(built) => *state.borrow_mut() = Some(built),
                Err(e) => tracing::error!("failed to create tray icon: {e}"),
            }
            if !tile_sys::watch_sessions(wnd.hwnd().ptr() as isize) {
                tracing::warn!(
                    "can't watch for the session locking; remapped keys may stick after unlocking"
                );
            }
            mailbox.attach(wnd.hwnd());
            Ok(0)
        }
    });

    wnd.on().wm(co::WM::APP, {
        let (wnd, state, tx, checks) = (wnd.clone(), state.clone(), tx.clone(), checks.clone());
        move |_| {
            let mut changed = false;
            while let Ok(request) = requests.try_recv() {
                match request {
                    Request::Sync => changed = true,
                    Request::Clicked(id) => {
                        let event = state.borrow().as_ref().and_then(|(_, i)| i.event(&id));
                        if let Some(event) = event {
                            let _ = tx.send(event);
                        }
                    }
                    Request::Quit => {
                        state.borrow_mut().take();
                        tile_sys::unwatch_sessions(wnd.hwnd().ptr() as isize);
                        w::PostQuitMessage(0);
                        return Ok(0);
                    }
                }
            }
            if changed && let Some((tray, items)) = state.borrow_mut().as_mut() {
                items.sync(tray, &checks);
            }
            Ok(0)
        }
    });

    wnd.on().wm_display_change({
        let tx = tx.clone();
        move |_| {
            let _ = tx.send(TrayEvent::DisplayChanged);
            Ok(())
        }
    });

    wnd.on().wm_wts_session_change({
        let tx = tx.clone();
        move |_| {
            let _ = tx.send(TrayEvent::SessionChanged);
            Ok(())
        }
    });

    // WM_SETTINGCHANGE (same value as WM_WININICHANGE) fires when the work area changes.
    wnd.on().wm(co::WM::WININICHANGE, {
        let tx = tx.clone();
        move |_| {
            let _ = tx.send(TrayEvent::DisplayChanged);
            Ok(0)
        }
    });

    if let Err(e) = wnd.run_main(Some(co::SW::HIDE)) {
        tracing::error!("tray message loop failed: {e}");
    }
    mailbox.detach();
}
