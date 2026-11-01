use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
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
    ToggleLogging,
    OpenConfig,
    OpenLogs,
    Quit,
    /// Monitors or the work area changed.
    DisplayChanged,
    /// The session was locked, unlocked or switched.
    SessionChanged,
}

const POLL_TIMER: usize = 1;

/// The tray icon, living on a dedicated thread with a hidden window and message loop.
pub struct Tray {
    shutdown: Arc<AtomicBool>,
    checks: Arc<Checks>,
    thread: Option<thread::JoinHandle<()>>,
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

/// A 32x32 icon: four rounded tiles, the top left one in the accent colour, or red while the
/// config has errors.
fn icon(error: bool) -> Option<Icon> {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let in_tile = |v: u32| (2..15).contains(&v) || (17..30).contains(&v);
            let corner = |v: u32| matches!(v, 2 | 14 | 17 | 29);
            let filled = in_tile(x) && in_tile(y) && !(corner(x) && corner(y));
            let primary = x < 16 && y < 16;
            let pixel = match (filled, primary) {
                (false, _) => [0, 0, 0, 0],
                (true, true) if error => [0xf3, 0x8b, 0xa8, 0xff],
                (true, true) => [0x89, 0xb4, 0xfa, 0xff],
                (true, false) => [0xcd, 0xd6, 0xf4, 0xff],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).ok()
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
    pub fn spawn(tx: mpsc::UnboundedSender<TrayEvent>) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(Checks::default());
        let (stop, state) = (shutdown.clone(), checks.clone());
        let thread = thread::Builder::new()
            .name("tray".into())
            .spawn(move || run(&tx, &stop, &state))
            .expect("spawn tray thread");
        Self {
            shutdown,
            checks,
            thread: Some(thread),
        }
    }

    /// Reflects the window manager's pause state in the menu.
    pub fn set_tile_paused(&self, paused: bool) {
        self.checks.tile_paused.store(paused, Ordering::Relaxed);
    }

    /// Reflects the key remapper's pause state in the menu.
    pub fn set_remap_paused(&self, paused: bool) {
        self.checks.remap_paused.store(paused, Ordering::Relaxed);
    }

    /// Reflects whether Tile is the shell in the menu.
    pub fn set_shell(&self, shell: bool) {
        self.checks.shell.store(shell, Ordering::Relaxed);
    }

    pub fn set_logging(&self, logging: bool) {
        self.checks.logging.store(logging, Ordering::Relaxed);
    }

    /// Shows that the config file has errors (and what they are) or, with `None`, that it
    /// loaded.
    pub fn set_config_error(&self, error: Option<String>) {
        *self
            .checks
            .config_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    /// Removes the icon and stops the tray thread.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(tx: &mpsc::UnboundedSender<TrayEvent>, shutdown: &Arc<AtomicBool>, checks: &Arc<Checks>) {
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Tile",
        class_name: "tile-message-window",
        ..Default::default()
    });
    let state: Rc<RefCell<Option<(TrayIcon, Items)>>> = Rc::new(RefCell::new(None));

    wnd.on().wm_create({
        let (wnd, state, checks) = (wnd.clone(), state.clone(), checks.clone());
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
            wnd.hwnd().SetTimer(POLL_TIMER, 100, None)?;
            Ok(0)
        }
    });

    wnd.on().wm_timer(POLL_TIMER, {
        let (wnd, state, tx, shutdown, checks) = (
            wnd.clone(),
            state.clone(),
            tx.clone(),
            shutdown.clone(),
            checks.clone(),
        );
        move || {
            if shutdown.load(Ordering::Relaxed) {
                state.borrow_mut().take();
                tile_sys::unwatch_sessions(wnd.hwnd().ptr() as isize);
                w::PostQuitMessage(0);
                return Ok(());
            }
            let mut state = state.borrow_mut();
            let Some((tray, items)) = state.as_mut() else {
                return Ok(());
            };
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                let mapped = if event.id == *items.tile.id() {
                    Some(TrayEvent::ToggleTile)
                } else if event.id == *items.remap.id() {
                    Some(TrayEvent::ToggleRemap)
                } else if event.id == *items.shell.id() {
                    Some(TrayEvent::ToggleShell)
                } else if event.id == *items.log.id() {
                    Some(TrayEvent::ToggleLogging)
                } else if event.id == items.open_config || event.id == *items.error.id() {
                    Some(TrayEvent::OpenConfig)
                } else if event.id == items.open_logs {
                    Some(TrayEvent::OpenLogs)
                } else if event.id == items.quit {
                    Some(TrayEvent::Quit)
                } else {
                    None
                };
                if let Some(e) = mapped {
                    let _ = tx.send(e);
                }
            }
            items.sync(tray, &checks);
            Ok(())
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
}
