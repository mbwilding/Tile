//! The daemon: owns the window manager state, the remapper and every OS integration, and runs the
//! event loop that joins them. The loop and startup are here; the rest of `Daemon` is split by
//! concern into the submodules.

mod apply;
mod commands;
mod events;
mod lifecycle;
mod lock;
mod manage;
mod pointer;
mod shell;
mod tracked;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, bail};
use tile_config::Loaded;
use tile_core::{Command, Desktops, LayoutKind, MonitorInfo, WindowId};
use tile_ipc::{Incoming, Response, Server};
use tile_launcher::{Launcher, LauncherEvent, LauncherSettings};
use tile_platform::{
    Backdrop, DragEvent, HotkeyEvent, Hotkeys, Indicator, IndicatorSettings, Tray, TrayEvent,
    WinDrag, WinEvent, WinEventHooks, Windows,
    desktops::{self as vd, DesktopChange},
    foreground,
};
use tile_shell::{
    consent::Prompts,
    notify::{Host, Registry},
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::{Instant, MissedTickBehavior, interval, sleep_until},
};

use self::tracked::Tracked;
use crate::{
    launch, logging,
    paths::{self, Restore},
    remap::{Notice, Remap},
    settings,
};

/// How long an event we caused ourselves (hide/show) may take to arrive.
const SELF_EVENT_WINDOW: Duration = Duration::from_secs(2);
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const DEFOCUS_GRACE: Duration = Duration::from_millis(500);
/// Logged when the remapper's emergency exit is typed.
const REMAP_PANIC: &str =
    "emergency exit (Backspace+Escape+Enter): remapping is off until the config is reloaded";

#[derive(Default, PartialEq)]
struct Published {
    workspace: Option<u8>,
    window: Option<(isize, String, String)>,
    layout: Option<LayoutKind>,
}

struct Receivers {
    events: UnboundedReceiver<WinEvent>,
    desktops: UnboundedReceiver<DesktopChange>,
    hotkeys: UnboundedReceiver<HotkeyEvent>,
    drags: UnboundedReceiver<DragEvent>,
    tray: UnboundedReceiver<TrayEvent>,
    launcher: UnboundedReceiver<LauncherEvent>,
    ipc: UnboundedReceiver<Incoming>,
    config: UnboundedReceiver<()>,
    remap: UnboundedReceiver<Notice>,
}

struct Daemon {
    desktops: Desktops,
    windows: Windows,
    loaded: Loaded,
    config_path: PathBuf,
    commands: Vec<Command>,
    hotkeys: Hotkeys,
    win_drag: WinDrag,
    /// The window being dragged with Win + left button.
    drag: Option<WindowId>,
    tray: Tray,
    launcher: Launcher,
    indicator: Indicator,
    /// The tray icons apps have added, while Tile hosts the notification area.
    registry: Registry,
    /// The notification area's window, while Tile is the shell and no other taskbar runs.
    notify_host: Option<Host>,
    /// Started with `--shell`: act as the shell whatever the tray's Shell switch says.
    shell_forced: bool,
    /// The tray's Shell switch.
    shell_on: bool,
    /// Said that a taskbar is running so Tile leaves the notification area to it.
    taskbar_noted: bool,
    /// The black desktop Tile paints while it hosts the notification area.
    backdrop: Option<Backdrop>,
    /// Brings up UAC prompts that wait for a taskbar click, while Tile hosts the notification
    /// area.
    prompts: Option<Prompts>,
    /// The window manager is paused (tray "Tile").
    paused: bool,
    /// The key remapper is paused (tray "Remap").
    remap_paused: bool,
    quit: bool,
    /// The window manager is on (`general.tiling`).
    tiling: bool,
    /// The key remapper, while `general.remap` is on.
    remap: Option<Remap>,
    remap_notices: UnboundedSender<Notice>,
    monitors: Vec<MonitorInfo>,
    /// What the daemon keeps about each window it manages.
    tracked: HashMap<WindowId, Tracked>,
    /// Windows excluded by rules or that we can't control (elevated).
    ignored: HashSet<WindowId>,
    bordered: Option<WindowId>,
    subscribers: Vec<UnboundedSender<Response>>,
    published: Published,
    saved_restore: Restore,
    /// We set the policy that disables Win+L (and must remove it again).
    lock_taken: bool,
    /// We already asked (UAC) for permission to manage the Win+L policy this run.
    lock_grant_requested: bool,
    /// After the `lock` dispatcher lifted the Win+L policy: when to re-apply it.
    relock_at: Option<Instant>,
    /// Shortly after a focus change: re-check borders apps may have repainted on activation.
    heal_at: Option<Instant>,
    /// Re-entrancy guard for re-syncing focus after Windows refused a focus change.
    resyncing_focus: bool,
    /// When Tile last put focus on the desktop because the focused monitor had nothing to focus.
    defocused_at: Option<Instant>,
    /// While typing hid the pointer: where it was then.
    cursor_hidden: Option<(i32, i32)>,
    /// While the pointer is hidden: when to next check whether the mouse moved.
    cursor_check_at: Option<Instant>,
}

fn launcher_settings(loaded: &Loaded) -> LauncherSettings {
    LauncherSettings {
        config: loaded.config.launcher.clone(),
        hide: loaded.launcher_hide.clone(),
        style: loaded.config.launcher_style(),
    }
}

fn indicator_settings(loaded: &Loaded) -> IndicatorSettings {
    IndicatorSettings {
        config: loaded.config.workspace_indicator.clone(),
        style: loaded.config.indicator_style(),
    }
}

/// Apps repaint their frame on activation, after our focus event; wait that long to re-check.
const HEAL_DELAY: Duration = Duration::from_millis(150);

/// How long a forced placement may take to land before a refusal is believed.
const FORCE_GRACE: Duration = Duration::from_millis(500);

/// How long the Win+L policy stays lifted for `lock` to take effect.
const RELOCK_DELAY: Duration = Duration::from_secs(2);

/// How often a hidden pointer checks for mouse movement.
const CURSOR_POLL: Duration = Duration::from_millis(30);

/// Starts `tile --watchdog <pid>`: a detached process that undoes our window changes
/// when this process exits for any reason, including being killed.
fn spawn_watchdog() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!("no watchdog: {e}");
            return;
        }
    };
    let pid = std::process::id().to_string();
    match tile_sys::spawn_detached(&exe, &["--watchdog", &pid]) {
        Ok(child) => tracing::debug!("watchdog started (pid {})", child.id()),
        Err(e) => tracing::warn!("failed to start watchdog: {e}"),
    }
}

/// Sleeps until `at`, or forever without one.
async fn until(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// No windows or monitors yet, on the native desktop being shown, with `loaded`'s layouts.
fn fresh_desktops(loaded: &Loaded) -> Desktops {
    let config = &loaded.config;
    Desktops::new(
        vd::current_desktop().unwrap_or_default(),
        config.layout_settings(),
        config.general.layout,
    )
}

/// Whether a window currently lives on a native desktop other than the visible one.
fn on_other_desktop(id: WindowId) -> bool {
    matches!((vd::desktop_of(id), vd::current_desktop()), (Some(d), Some(c)) if d != c)
}

/// Runs the window manager. `config_error` says why the config file didn't load, if it didn't
/// (and `loaded` is the defaults).
pub async fn run(
    config_path: PathBuf,
    loaded: Loaded,
    config_error: Option<String>,
    shell_forced: bool,
) -> anyhow::Result<()> {
    if tile_ipc::is_running().await {
        bail!("Tile is already running");
    }
    let server = Server::bind(tile_platform::user_sid().as_deref())
        .context("failed to create the IPC pipe")?;
    let _com = vd::init_com();

    // Recover windows a crashed instance left hidden or undecorated.
    paths::load_restore().apply();
    paths::save_restore(&Restore::default());
    spawn_watchdog();

    let (hotkey_tx, hotkeys_rx) = mpsc::unbounded_channel();
    let (tray_tx, tray_rx) = mpsc::unbounded_channel();
    let (launcher_tx, launcher_rx) = mpsc::unbounded_channel();
    let (ipc_tx, ipc_rx) = mpsc::unbounded_channel();
    let (config_tx, config_rx) = mpsc::unbounded_channel();
    let (desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let (remap_tx, remap_rx) = mpsc::unbounded_channel();
    let (drag_tx, drags_rx) = mpsc::unbounded_channel();

    let (hooks, events) = WinEventHooks::install()
        .await
        .context("failed to hook window events")?;
    let _desktop_listener = vd::listen(desktop_tx);
    // Before the remapper: see `Hotkeys`.
    let hotkeys = Hotkeys::start(hotkey_tx).context("failed to install the keyboard hook")?;
    let win_drag = WinDrag::start(drag_tx).context("failed to install the mouse hook")?;
    let tray = Tray::spawn(tray_tx);
    let registry = Registry::default();
    let launcher = Launcher::spawn(
        launcher_tx,
        launcher_settings(&loaded),
        paths::data_dir().join("launcher.json"),
        registry.clone(),
    );
    let indicator = Indicator::spawn(indicator_settings(&loaded));
    let _watcher = tile_config::watch(&config_path, move || {
        let _ = config_tx.send(());
    })
    .inspect_err(|e| tracing::warn!("config hot reload unavailable: {e}"))
    .ok();
    tokio::spawn(server.serve(ipc_tx));

    let saved = settings::load();
    let mut daemon = Daemon {
        desktops: fresh_desktops(&loaded),
        windows: Windows::new(),
        loaded,
        config_path,
        commands: Vec::new(),
        hotkeys,
        win_drag,
        drag: None,
        tray,
        launcher,
        indicator,
        registry,
        notify_host: None,
        shell_forced,
        shell_on: saved.shell,
        taskbar_noted: false,
        backdrop: None,
        prompts: None,
        paused: !saved.tile,
        remap_paused: !saved.remap,
        quit: false,
        tiling: false,
        remap: None,
        remap_notices: remap_tx,
        monitors: Vec::new(),
        tracked: HashMap::new(),
        ignored: HashSet::new(),
        bordered: None,
        subscribers: Vec::new(),
        published: Published::default(),
        saved_restore: Restore::default(),
        lock_taken: false,
        lock_grant_requested: false,
        relock_at: None,
        heal_at: None,
        resyncing_focus: false,
        defocused_at: None,
        cursor_hidden: None,
        cursor_check_at: None,
    };
    daemon.tray.set_tile_paused(daemon.paused);
    daemon.tray.set_remap_paused(daemon.remap_paused);
    daemon.install_binds();
    if daemon.loaded.config.general.tiling {
        daemon.start_tiling();
    }
    daemon.update_remap();
    daemon.update_cursor();
    daemon.tray.set_logging(logging::enabled());
    daemon.tray.set_shell(daemon.shell_wanted());
    daemon.tray.set_config_error(config_error);
    // The sign-in shell follows the saved switch, so a first run with it on sets it up, and
    // one with it off puts Explorer back. Started as the shell, it leaves things as they are.
    if daemon.shell_on || !daemon.shell_forced {
        daemon.register_shell();
    }
    daemon.start_shell();
    tracing::info!("Tile started");

    daemon
        .event_loop(Receivers {
            events,
            desktops: desktop_rx,
            hotkeys: hotkeys_rx,
            drags: drags_rx,
            tray: tray_rx,
            launcher: launcher_rx,
            ipc: ipc_rx,
            config: config_rx,
            remap: remap_rx,
        })
        .await;

    daemon.shutdown();
    hooks.unhook().await;
    tracing::info!("Tile stopped");
    Ok(())
}

impl Daemon {
    /// The daemon's state for the window `id`, made empty if there is none yet.
    fn track(&mut self, id: WindowId) -> &mut Tracked {
        self.tracked.entry(id).or_default()
    }

    /// Whether the user is moving or resizing `id` with the mouse.
    fn is_moving(&self, id: WindowId) -> bool {
        self.tracked.get(&id).is_some_and(|t| t.moving)
    }

    /// Whether `id` is placed with force, past its app's minimum size.
    fn is_forced(&self, id: WindowId) -> bool {
        self.tracked.get(&id).is_some_and(|t| t.forced.is_some())
    }

    async fn event_loop(&mut self, mut rx: Receivers) {
        let mut sweep = interval(Duration::from_secs(2));
        sweep.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut reload_at: Option<Instant> = None;

        while !self.quit {
            // The movers draw animations themselves; the daemon only retires them when done.
            let animation_end = self
                .tracked
                .values()
                .filter_map(|t| t.tween.as_ref())
                .map(|t| Instant::from_std(t.animation.end()))
                .min();
            tokio::select! {
                () = until(self.cursor_check_at) => self.check_cursor(),
                () = until(self.relock_at) => {
                    self.relock_at = None;
                    if self.lock_taken {
                        tile_platform::set_lock_disabled(true);
                    }
                }
                () = until(self.heal_at) => {
                    self.heal_at = None;
                    self.heal_borders();
                }
                Some(event) = rx.events.recv() => self.on_event(event),
                Some(change) = rx.desktops.recv() => self.on_desktop(change),
                Some(event) = rx.hotkeys.recv() => match event {
                    HotkeyEvent::Bind(index) => {
                        if let Some(command) = self.commands.get(index).cloned() {
                            tracing::debug!("hotkey: {command}");
                            self.run_command(command);
                        }
                    }
                    HotkeyEvent::Typed => self.hide_cursor(),
                },
                Some(event) = rx.drags.recv() => self.on_drag(event),
                Some(event) = rx.tray.recv() => self.on_tray(event),
                Some(event) = rx.launcher.recv() => match event {
                    LauncherEvent::Launch(id) => launch::start_app(&id, false),
                    LauncherEvent::LaunchAsAdmin(id) => launch::start_app(&id, true),
                    LauncherEvent::Run(line) => launch::exec(&line),
                    LauncherEvent::Dispatch(command) => self.run_command(command),
                    LauncherEvent::Tray { key, click, at } => {
                        if !self.registry.click(key, click, at) {
                            tracing::debug!("the tray icon has gone");
                        }
                    }
                    LauncherEvent::System(action) => self.run_system(action),
                },
                Some(incoming) = rx.ipc.recv() => self.on_ipc(incoming),
                Some(()) = rx.config.recv() => reload_at = Some(Instant::now() + RELOAD_DEBOUNCE),
                Some(Notice::Panic) = rx.remap.recv() => self.stop_remap(REMAP_PANIC),
                () = until(reload_at) => {
                    reload_at = None;
                    self.reload();
                }
                () = until(animation_end) => {
                    let now = std::time::Instant::now();
                    for tracked in self.tracked.values_mut() {
                        if tracked.tween.as_ref().is_some_and(|t| t.animation.end() <= now) {
                            tracked.tween = None;
                        }
                    }
                }
                _ = sweep.tick() => self.sweep(),
                _ = tokio::signal::ctrl_c() => self.quit = true,
            }
        }
    }

    fn startup(&mut self) {
        self.refresh_monitors();
        self.discover_existing();
        if let Some(id) = foreground() {
            self.on_foreground(id);
        }
    }

    /// Manages every window that is visible right now, without stealing focus. They are taken
    /// left to right, then top to bottom, so each lands in the tile nearest where it was.
    fn discover_existing(&mut self) {
        let mut all = self.windows.discover_all();
        all.sort_by_key(|f| {
            let (x, y) = f.rect.center();
            (x, y)
        });
        for found in all {
            if vd::is_pinned(found.id) {
                self.windows.forget(found.id);
                continue;
            }
            let effects = self.manage(found, false);
            self.apply(effects);
        }
    }

    fn shutdown(&mut self) {
        self.stop_remap("remapping stopped");
        self.show_cursor();
        self.release_windows();
        if self.lock_taken {
            tile_platform::set_lock_disabled(false);
        }
        paths::save_restore(&Restore::default());
        self.launcher.shutdown();
        self.indicator.shutdown();
        self.tray.shutdown();
        if let Some(host) = self.notify_host.take() {
            host.stop();
        }
        self.restore_backdrop();
    }
}
