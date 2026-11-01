use std::{
    collections::{HashMap, HashSet},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, bail};
use tile_config::Loaded;
use tile_core::{
    Command, Effect, LayoutKind, ManageOptions, MonitorInfo, Rect, WindowId,
    system::{Action, Power},
};
use tile_hook::notify::{Host, Registry, taskbar_exists};
use tile_ipc::{Event, Incoming, Query, Request, Response, Server};
use tile_platform::{
    Animation, Backdrop, Discovered, DragEvent, HotkeyEvent, Hotkeys, Indicator, IndicatorSettings,
    Launcher, LauncherEvent, LauncherSettings, Mover, PlaceError, Tray, TrayEvent, WinDrag,
    WinEvent, WinEventHooks, Windows, close_foreground, cursor_pos,
    desktops::{self as vd, DesktopChange, DesktopId},
    focus_desktop, focus_exe, foreground, is_shell_window, monitors, mouse_button_down,
    set_cursor_pos,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::{Instant, MissedTickBehavior, interval, sleep_until},
};

use crate::{
    desktops::Desktops,
    launch, logging,
    paths::{self, Restore},
    remap::{self, Notice, Remap},
    settings,
};

/// How long an event we caused ourselves (hide/show) may take to arrive.
const SELF_EVENT_WINDOW: Duration = Duration::from_secs(2);
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
/// Logged when the remapper's emergency exit is typed.
const REMAP_PANIC: &str =
    "emergency exit (Backspace+Escape+Enter): remapping is off until the config is reloaded";

/// A window being animated. The mover draws the frames on its own thread; the daemon only
/// keeps the animation to know where the window is and when it ends.
struct Tween {
    animation: Animation,
    mover: Mover,
}

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
    /// The last rect we placed each window at (or are animating it towards).
    placed: HashMap<WindowId, Rect>,
    tweens: HashMap<WindowId, Tween>,
    /// Windows the user is dragging / resizing with the mouse.
    moving: HashSet<WindowId>,
    /// Of those, the ones whose edge has been dragged, so the others have been following it.
    resizing: HashSet<WindowId>,
    pending_show: HashMap<WindowId, (u32, Instant)>,
    pending_hide: HashMap<WindowId, (u32, Instant)>,
    /// Windows excluded by rules or that we can't control (elevated).
    ignored: HashSet<WindowId>,
    bordered: Option<WindowId>,
    /// Windows handled as undecorated; `true` if we actually removed a title bar.
    undecorated: HashMap<WindowId, bool>,
    /// Per-window `decorations` overrides from rules.
    decoration_rules: HashMap<WindowId, bool>,
    subscribers: Vec<UnboundedSender<Response>>,
    published: Published,
    saved_restore: Restore,
    /// Windows that refused their tile size (app minimum size); placed with force from now
    /// on. The instant is when force was last applied.
    forced: HashMap<WindowId, Instant>,
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

fn bump(map: &mut HashMap<WindowId, (u32, Instant)>, id: WindowId) {
    let entry = map.entry(id).or_insert((0, Instant::now()));
    entry.0 += 1;
    entry.1 = Instant::now();
}

/// Consumes one expected self-inflicted event, if there is a recent one.
fn consume(map: &mut HashMap<WindowId, (u32, Instant)>, id: WindowId) -> bool {
    match map.get_mut(&id) {
        Some((count, at)) if *count > 0 && at.elapsed() < SELF_EVENT_WINDOW => {
            *count -= 1;
            if *count == 0 {
                map.remove(&id);
            }
            true
        }
        Some(_) => {
            map.remove(&id);
            false
        }
        None => false,
    }
}

/// Starts `tile --watchdog <pid>`: a detached process that undoes our window changes
/// when this process exits for any reason, including being killed.
fn spawn_watchdog() {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!("no watchdog: {e}");
            return;
        }
    };
    let pid = std::process::id().to_string();
    let launch = |flags: u32| {
        std::process::Command::new(&exe)
            .args(["--watchdog", &pid])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // Escape our job object if allowed, so killing the job doesn't take the watchdog too.
    match launch(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| launch(base)) {
        Ok(child) => tracing::debug!("watchdog started (pid {})", child.id()),
        Err(e) => tracing::warn!("failed to start watchdog: {e}"),
    }
}

/// Whether a window currently lives on a native desktop other than the visible one.
fn on_other_desktop(raw: isize) -> bool {
    matches!((vd::desktop_of(raw), vd::current_desktop()), (Some(d), Some(c)) if d != c)
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

    let settings = loaded.config.layout_settings();
    let layout = loaded.config.general.layout;
    let current = vd::current_desktop().unwrap_or_default();
    let saved = settings::load();
    let mut daemon = Daemon {
        desktops: Desktops::new(current, settings, layout),
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
        paused: !saved.tile,
        remap_paused: !saved.remap,
        quit: false,
        tiling: false,
        remap: None,
        remap_notices: remap_tx,
        monitors: Vec::new(),
        placed: HashMap::new(),
        tweens: HashMap::new(),
        moving: HashSet::new(),
        resizing: HashSet::new(),
        pending_show: HashMap::new(),
        pending_hide: HashMap::new(),
        ignored: HashSet::new(),
        bordered: None,
        undecorated: HashMap::new(),
        decoration_rules: HashMap::new(),
        subscribers: Vec::new(),
        published: Published::default(),
        saved_restore: Restore::default(),
        forced: HashMap::new(),
        lock_taken: false,
        lock_grant_requested: false,
        relock_at: None,
        heal_at: None,
        resyncing_focus: false,
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
    async fn event_loop(&mut self, mut rx: Receivers) {
        let mut sweep = interval(Duration::from_secs(2));
        sweep.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut reload_at: Option<Instant> = None;

        while !self.quit {
            // The movers draw animations themselves; the daemon only retires them when done.
            let animation_end = self.tweens.values().map(|t| t.animation.end()).min();
            let animations = async {
                match animation_end {
                    Some(at) => sleep_until(Instant::from_std(at)).await,
                    None => std::future::pending().await,
                }
            };
            let reload = async {
                match reload_at {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let relock_at = self.relock_at;
            let relock = async {
                match relock_at {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let heal_at = self.heal_at;
            let heal = async {
                match heal_at {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let cursor_check_at = self.cursor_check_at;
            let cursor_check = async {
                match cursor_check_at {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = cursor_check => self.check_cursor(),
                () = relock => {
                    self.relock_at = None;
                    if self.lock_taken {
                        tile_platform::set_lock_disabled(true);
                    }
                }
                () = heal => {
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
                    LauncherEvent::Launch(id) => {
                        launch::exec(&format!("\"shell:AppsFolder\\{id}\""));
                    }
                    LauncherEvent::LaunchAsAdmin(id) => {
                        tile_platform::run_as_admin(&format!("shell:AppsFolder\\{id}"));
                    }
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
                () = reload => {
                    reload_at = None;
                    self.reload();
                }
                () = animations => {
                    let now = std::time::Instant::now();
                    self.tweens.retain(|_, t| t.animation.end() > now);
                }
                _ = sweep.tick() => self.sweep(),
                _ = tokio::signal::ctrl_c() => self.quit = true,
            }
        }
    }

    fn startup(&mut self) {
        self.refresh_monitors();
        self.discover_existing();
        if let Some(raw) = foreground() {
            self.on_foreground(raw);
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
            if vd::is_pinned(found.id.0) {
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

    /// Removes the black windows, so the desktop shows again.
    fn restore_backdrop(&mut self) {
        self.backdrop = None;
    }

    /// Whether Tile acts as the Windows shell: `--shell`, or the tray's Shell switch.
    fn shell_wanted(&self) -> bool {
        self.shell_forced || self.shell_on
    }

    /// The tray's Shell item: flips the saved switch, which then applies like a config edit.
    fn toggle_shell(&mut self) {
        let on = !self.shell_on;
        settings::update(|s| s.shell = on);
        self.apply_shell(on);
    }

    /// Makes the Shell switch `on` or off: Windows starts Tile instead of Explorer at the next
    /// sign-in, Tile hosts the notification area now if no taskbar runs, and the launcher is
    /// always bound.
    fn apply_shell(&mut self, on: bool) {
        self.shell_on = on;
        self.register_shell();
        self.tray.set_shell(self.shell_wanted());
        tracing::info!("shell mode {}", if on { "on" } else { "off" });
        self.install_binds();
        self.sync_shell();
    }

    /// Points the sign-in shell at the switch: Tile when it is on, Explorer when it is off.
    fn register_shell(&self) {
        let exe = tile_platform::installed_exe();
        let result = if !self.shell_on {
            tile_platform::set_sign_in_shell(None)
        } else if exe.exists() {
            tile_platform::set_sign_in_shell(Some(&exe))
        } else {
            Err("Tile isn't installed (run `tilectl install`)".to_owned())
        };
        if let Err(e) = result {
            tracing::warn!("couldn't change the sign-in shell, so Explorer still starts: {e}");
        }
    }

    /// At startup: hosts the notification area and, when there was no taskbar to begin with,
    /// starts what Explorer would have at sign-in.
    fn start_shell(&mut self) {
        let bare = !taskbar_exists();
        self.sync_shell();
        let shell = &self.loaded.config.shell;
        if self.notify_host.is_none() || !bare || !shell.run_startup {
            return;
        }
        if !tile_platform::first_startup_this_session() {
            tracing::info!("the sign-in programs were already started this session");
            return;
        }
        let commands = tile_platform::startup_commands();
        tracing::info!("starting {} sign-in programs", commands.len());
        for command in commands {
            tracing::debug!("startup: {command}");
            launch::exec(&command);
        }
    }

    /// Hosts the notification area while Tile is the shell and no other taskbar runs, and lets
    /// go of it when one appears or Tile stops being the shell.
    fn sync_shell(&mut self) {
        if self.notify_host.as_ref().is_some_and(|h| !h.running()) {
            tracing::info!("another taskbar took over the notification area");
            self.notify_host = None;
            self.restore_backdrop();
        }
        match (self.shell_wanted(), self.notify_host.is_some()) {
            (true, false) if taskbar_exists() => {
                if !self.taskbar_noted {
                    self.taskbar_noted = true;
                    tracing::info!("a taskbar is running; the notification area is left to it");
                }
            }
            (true, false) => {
                self.taskbar_noted = false;
                self.notify_host = Host::start(self.registry.clone());
                if self.notify_host.is_some() {
                    tracing::info!("hosting the notification area");
                    if self.backdrop.is_none() {
                        self.backdrop = Backdrop::black();
                    }
                } else {
                    tracing::warn!("couldn't start the notification area");
                }
            }
            (false, true) => {
                if let Some(host) = self.notify_host.take() {
                    host.stop();
                }
                self.restore_backdrop();
                tracing::info!("stopped hosting the notification area");
            }
            _ => {}
        }
    }

    /// Does what a system row of the launcher asks for.
    fn run_system(&mut self, action: Action) {
        if action == Action::Power(Power::Lock) {
            self.run_command(Command::Lock);
        } else if let Some(line) = tile_platform::system_command_line(&action) {
            launch::exec(&line);
        } else {
            let what = format!("{action:?}");
            let failed = what.clone();
            let spawned = std::thread::Builder::new()
                .name("system action".into())
                .spawn(move || {
                    if !tile_platform::apply_system_action(&action) {
                        tracing::warn!("couldn't do {failed}");
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!("couldn't do {what}: {e}");
            }
        }
    }

    /// Shows the windows we hid and undoes our borders and title bar changes.
    fn release_windows(&mut self) {
        for id in self.desktops.hidden() {
            self.windows.show(id);
        }
        for id in self.desktops.managed() {
            self.windows.set_border(id, None);
        }
        for (id, changed) in std::mem::take(&mut self.undecorated) {
            if changed {
                self.windows.set_decorated(id, true);
            }
        }
    }

    /// Turns the window manager on: takes over Win+L, installs the binds and manages the
    /// windows that are open.
    fn start_tiling(&mut self) {
        self.tiling = true;
        let config = &self.loaded.config;
        self.desktops = Desktops::new(
            vd::current_desktop().unwrap_or_default(),
            config.layout_settings(),
            config.general.layout,
        );
        self.update_lock_override();
        self.switch_off_click_to_do();
        self.install_binds();
        if !self.paused {
            self.startup();
        }
        self.update_cursor();
        tracing::info!("tiling started");
    }

    /// Turns the window manager off, giving back every window and Win+L.
    fn stop_tiling(&mut self) {
        self.release_windows();
        self.tiling = false;
        self.update_lock_override();
        self.install_binds();
        self.reset_state();
        self.update_cursor();
        self.persist_restore();
        self.publish();
        tracing::info!("tiling stopped");
    }

    /// Forgets every window and monitor, as if no window had been managed yet.
    fn reset_state(&mut self) {
        let config = &self.loaded.config;
        self.desktops = Desktops::new(
            vd::current_desktop().unwrap_or_default(),
            config.layout_settings(),
            config.general.layout,
        );
        self.monitors.clear();
        self.placed.clear();
        self.tweens.clear();
        self.moving.clear();
        self.pending_show.clear();
        self.pending_hide.clear();
        self.ignored.clear();
        self.bordered = None;
        self.decoration_rules.clear();
        self.forced.clear();
        self.heal_at = None;
        self.drag = None;
    }

    /// Starts, updates or stops the key remapper to match `general.remap`.
    fn update_remap(&mut self) {
        if !self.loaded.config.general.remap {
            self.stop_remap("remapping stopped");
            return;
        }
        let config = self.loaded.remap.clone();
        if let Some(remap) = &self.remap {
            remap.send(remap::Command::Load(config));
            return;
        }
        match Remap::start(config, self.remap_paused, self.remap_notices.clone()) {
            Ok(remap) => {
                self.remap = Some(remap);
                if !tile_platform::is_elevated() {
                    tracing::warn!(
                        "not running as administrator: keys typed into admin windows aren't \
                         remapped (`tilectl install` sets Tile up to run elevated)"
                    );
                }
                tracing::info!("remapping started");
            }
            Err(e) => tracing::error!("failed to install the remapping hook: {e}"),
        }
    }

    /// Removes the remapping hook, if it runs, logging `why`.
    fn stop_remap(&mut self, why: &str) {
        if let Some(remap) = self.remap.take() {
            remap.stop();
            tracing::info!("{why}");
        }
    }

    /// Installs the binds. While tiling is off or paused only those that
    /// [work while paused](Command::works_while_paused) fire.
    /// In shell mode the launcher is how everything starts, so it is always bound: a bind of
    /// yours if there is one, else Win+R or, if that is taken, Win+D.
    fn shell_launcher_bind(&self) -> Option<(tile_config::Keybind, Command)> {
        let binds = &self.loaded.binds;
        if !self.shell_wanted() || binds.iter().any(|(_, c)| *c == Command::Launcher) {
            return None;
        }
        ["super+r", "super+d"].into_iter().find_map(|keys| {
            let bind: tile_config::Keybind = keys.parse().ok()?;
            let free = !binds.iter().any(|(b, _)| *b == bind);
            if free {
                tracing::info!("shell mode: the launcher is bound to {keys}");
            }
            free.then_some((bind, Command::Launcher))
        })
    }

    fn install_binds(&mut self) {
        self.hotkeys.set_paused(self.paused || !self.tiling);
        // Without the takeover, Windows would lock *and* run the bind.
        let win_l = self.win_l_available();
        if !win_l && self.loaded.binds.iter().any(|(bind, _)| bind.is_win_l()) {
            tracing::info!(
                "Win+L binds are skipped: Windows keeps Win+L (see `general.override_win_l`)"
            );
        }
        let fallback = self.shell_launcher_bind();
        let mut usable: Vec<_> = self
            .loaded
            .binds
            .iter()
            .filter(|(bind, _)| win_l || !bind.is_win_l())
            .collect();
        usable.extend(fallback.as_ref());
        let binds: Vec<_> = usable
            .iter()
            .map(|(bind, command)| (bind.clone(), command.works_while_paused()))
            .collect();
        self.commands = usable.iter().map(|(_, c)| c.clone()).collect();
        for error in self.hotkeys.set_binds(&binds) {
            tracing::error!("{error}");
        }
        self.hotkeys
            .set_disable_start_menu(self.loaded.config.general.disable_start_menu);
    }

    fn reload(&mut self) {
        match Loaded::load_or_default(&self.config_path) {
            Ok(loaded) => {
                self.tray.set_config_error(None);
                // Our own save of a tray switch lands here too; nothing else changed.
                let mut same = loaded.config.clone();
                same.tray = self.loaded.config.tray.clone();
                let unchanged = same == self.loaded.config;
                let shell = loaded.config.tray.shell;
                self.loaded = loaded;
                // The Shell switch follows the config file, whoever edited it.
                if shell != self.shell_on {
                    self.apply_shell(shell);
                }
                if unchanged {
                    return;
                }
                self.launcher.set_settings(launcher_settings(&self.loaded));
                self.indicator
                    .set_settings(indicator_settings(&self.loaded));
                self.install_binds();
                self.update_remap();
                self.update_cursor();
                self.sync_shell();
                match (self.tiling, self.loaded.config.general.tiling) {
                    (false, true) => self.start_tiling(),
                    (true, false) => self.stop_tiling(),
                    (true, true) => self.reload_tiling(),
                    (false, false) => {}
                }
                tracing::info!("config reloaded");
            }
            Err(e) => {
                tracing::error!("keeping previous config: {e}");
                self.tray.set_config_error(Some(e.summary()));
            }
        }
    }

    /// Applies a reloaded config to the running window manager.
    fn reload_tiling(&mut self) {
        self.ignored.clear();
        self.update_lock_override();
        self.switch_off_click_to_do();
        self.install_binds();
        self.decoration_rules = self
            .desktops
            .managed()
            .into_iter()
            .filter_map(|id| {
                let info = self.windows.info(id)?;
                self.loaded
                    .rules
                    .resolve(&info)
                    .decorations
                    .map(|d| (id, d))
            })
            .collect();
        let config = &self.loaded.config;
        let effects = self
            .desktops
            .set_settings(config.layout_settings(), config.general.layout);
        self.repaint_borders();
        self.apply(effects);
    }

    fn toggle_logging(&mut self) {
        logging::set_enabled(!logging::enabled());
        self.tray.set_logging(logging::enabled());
    }

    /// `togglepause`: pauses the window manager and the remapper, or resumes both if both are
    /// paused.
    fn toggle_pause(&mut self) {
        let pause = !(self.paused && self.remap_paused);
        self.set_remap_paused(pause);
        self.set_paused(pause);
    }

    fn set_remap_paused(&mut self, paused: bool) {
        self.remap_paused = paused;
        self.tray.set_remap_paused(paused);
        settings::update(|s| s.remap = !paused);
        if let Some(remap) = &self.remap {
            remap.send(remap::Command::Pause(paused));
        }
        tracing::info!("remapping {}", if paused { "paused" } else { "resumed" });
    }

    /// Pauses or resumes the window manager (binds, except `togglepause`, included).
    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.hotkeys.set_paused(paused || !self.tiling);
        self.tray.set_tile_paused(paused);
        settings::update(|s| s.tile = !paused);
        self.update_cursor();
        tracing::info!("tiling {}", if paused { "paused" } else { "resumed" });
        if !self.tiling {
            return;
        }
        if paused {
            // Hand every window back as it was: shown, with its title bar and border.
            self.release_windows();
            self.reset_state();
            self.persist_restore();
            self.publish();
        } else {
            self.startup();
        }
    }

    /// `focusorexec`: focuses a window of the command's program if there is one, otherwise
    /// runs the command.
    /// Focuses a window Tile manages, switching to its workspace first.
    fn focus_managed(&mut self, id: WindowId) {
        let effects = self.desktops.with_owner(id, |s| s.window_focused(id));
        self.apply(effects);
        self.windows.focus(id);
        if let Some(frame) = self.windows.frame(id) {
            self.warp_cursor(frame);
        }
    }

    fn focus_or_exec(&mut self, command: &str) {
        let exe = Path::new(tile_config::program(command))
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        if let Some(exe) = exe {
            let current = self.desktops.current_id();
            let focused = self.focused();
            let mut matches: Vec<WindowId> = self
                .desktops
                .managed()
                .into_iter()
                .filter(|id| {
                    self.desktops.owner(*id) == Some(current)
                        && self
                            .windows
                            .info(*id)
                            .is_some_and(|i| i.exe.eq_ignore_ascii_case(&exe))
                })
                .collect();
            matches.sort_by_key(|id| id.0);
            // Pressing again moves on to the next window of the program.
            let next = focused
                .and_then(|f| matches.iter().find(|id| id.0 > f.0))
                .or_else(|| matches.first())
                .copied();
            if let Some(id) = next {
                self.focus_managed(id);
                return;
            }
            if focus_exe(&exe) {
                return;
            }
        }
        launch::exec(command);
    }

    /// Takes over or gives back Win+L to match `general.override_win_l` (while tiling).
    fn update_lock_override(&mut self) {
        let want = self.tiling && self.loaded.config.general.override_win_l;
        // Leave a policy the user (or an admin) set themselves alone.
        if want && !self.lock_taken && !tile_platform::lock_disabled() {
            if tile_platform::set_lock_disabled(true) {
                self.lock_taken = true;
                tracing::info!("took over Win+L");
            } else {
                self.request_lock_permission();
            }
        } else if !want && self.lock_taken {
            tile_platform::set_lock_disabled(false);
            self.lock_taken = false;
            tracing::info!("gave Win+L back to Windows");
        }
        self.persist_restore();
    }

    /// Switches Click to Do off if `general.disable_click_to_do` asks for it. It stays off after
    /// Tile exits: Windows pops up a notice on every flip.
    fn switch_off_click_to_do(&self) {
        if self.tiling
            && self.loaded.config.general.disable_click_to_do
            && tile_platform::click_to_do_available()
            && !tile_platform::click_to_do_disabled()
            && tile_platform::disable_click_to_do()
        {
            tracing::info!("switched off Click to Do");
        }
    }

    /// The policy key is admin-only on some machines. Ask once (one UAC prompt) for the user
    /// to be allowed to manage it; `sweep` retries the takeover until that lands.
    fn request_lock_permission(&mut self) {
        if self.lock_grant_requested {
            return;
        }
        self.lock_grant_requested = true;
        tracing::warn!(
            "Win+L needs a one-time permission; asking Windows (UAC). Win+L binds wait until then"
        );
        let script = paths::data_dir().join("grant-win-l.ps1");
        let written = std::fs::create_dir_all(paths::data_dir())
            .and_then(|()| std::fs::write(&script, tile_platform::GRANT_SCRIPT));
        if let Err(e) = written {
            tracing::error!("could not write {}: {e}", script.display());
            return;
        }
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let spawned = std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        if let Err(e) = spawned {
            tracing::error!("could not request Win+L permission: {e}");
        }
    }

    /// Retries taking over Win+L after the permission prompt.
    fn retry_lock_override(&mut self) {
        let want = self.loaded.config.general.override_win_l;
        if want
            && self.lock_grant_requested
            && !self.lock_taken
            && tile_platform::set_lock_disabled(true)
        {
            self.lock_taken = true;
            tracing::info!("took over Win+L");
            self.persist_restore();
            self.install_binds();
        }
    }

    /// Whether Win+L can be bound right now (Windows' lock shortcut is disabled).
    fn win_l_available(&self) -> bool {
        self.lock_taken || tile_platform::lock_disabled()
    }

    fn lock(&mut self) {
        if tile_platform::lock_workstation() {
            self.relock_at = Some(Instant::now() + RELOCK_DELAY);
        }
    }

    fn focused(&self) -> Option<WindowId> {
        self.desktops.current_ref().focused()
    }

    fn run_command(&mut self, command: Command) {
        match command {
            Command::Reload => self.reload(),
            Command::TogglePause => self.toggle_pause(),
            Command::ToggleTile => self.set_paused(!self.paused),
            Command::ToggleRemap => self.set_remap_paused(!self.remap_paused),
            Command::ToggleShell => self.toggle_shell(),
            Command::ToggleLog => self.toggle_logging(),
            Command::Exit => self.quit = true,
            Command::Exec(line) => launch::exec(&line),
            Command::FocusOrExec(line) => self.focus_or_exec(&line),
            Command::KillActive => close_foreground(),
            Command::Desktop(target) => vd::switch_desktop(target),
            Command::Lock => self.lock(),
            Command::Launcher => {
                let area = (self.tiling && !self.paused)
                    .then(|| self.focused_monitor().map(|m| m.work_area))
                    .flatten();
                self.launcher.toggle(area);
            }
            _ if self.paused || !self.tiling => {}
            Command::MoveToDesktop(target) => {
                if let Some(id) = self.focused() {
                    vd::move_to_desktop(id.0, target);
                    self.check_desktop(id);
                }
            }
            Command::Pin => {
                if let Some(id) = self.focused() {
                    vd::toggle_pin(id.0);
                    self.check_desktop(id);
                }
            }
            command => {
                let switches =
                    matches!(command, Command::Workspace(_) | Command::MoveToWorkspace(_));
                let before = self.focused_monitor().map(|m| m.id);
                let effects = self.desktops.current().apply(&command);
                let focus = effects.iter().rev().find_map(|e| match e {
                    Effect::Focus(id) => Some(*id),
                    _ => None,
                });
                let window = focus.and_then(|id| {
                    effects.iter().rev().find_map(|e| match e {
                        Effect::Place { id: i, rect } if *i == id => Some(*rect),
                        _ => None,
                    })
                });
                self.apply(effects);
                let moved_monitor = self.focused_monitor().map(|m| m.id) != before;
                let target = window.or_else(|| {
                    self.focused_monitor()
                        .filter(|_| moved_monitor)
                        .map(|m| m.rect)
                });
                if let Some(target) = target {
                    self.warp_cursor(target);
                }
                if switches {
                    self.show_workspace();
                }
            }
        }
    }

    /// After a key moved focus: puts the pointer on the focused window (it stays put if already
    /// over it), then hides it as typing would, until the mouse moves.
    fn warp_cursor(&mut self, target: Rect) {
        if !self.loaded.config.cursor.warp {
            return;
        }
        let (x, y) = cursor_pos();
        if !target.contains_point(x, y) {
            set_cursor_pos(target.center());
        }
        if self.cursor_hidden.is_some() {
            // Already hidden by typing: it must not reappear because we moved it.
            self.cursor_hidden = Some(cursor_pos());
        } else {
            self.hide_cursor();
        }
    }

    /// Flashes the focused workspace's number on its monitor.
    fn show_workspace(&self) {
        let state = self.desktops.current_ref();
        if let (Some(workspace), Some(monitor)) =
            (state.focused_workspace(), state.focused_monitor())
        {
            self.indicator.show(workspace, monitor.work_area);
        }
    }

    fn focused_monitor(&self) -> Option<&MonitorInfo> {
        self.desktops.current_ref().focused_monitor()
    }

    /// Starts or stops hiding the pointer while typing, to match `cursor.hide_on_key`.
    fn update_cursor(&mut self) {
        self.win_drag
            .set_enabled(self.loaded.config.cursor.drag && self.tiling && !self.paused);
        let hide = self.loaded.config.cursor.hide_on_key && !self.paused;
        self.hotkeys.set_report_typing(hide);
        if !hide {
            self.show_cursor();
        }
    }

    /// A key was typed: hide the pointer until the mouse moves.
    fn hide_cursor(&mut self) {
        if self.cursor_hidden.is_some() || self.paused || !self.loaded.config.cursor.hide_on_key {
            return;
        }
        // Recorded first: if we die mid-way, the watchdog still restores the cursors.
        self.cursor_hidden = Some(cursor_pos());
        self.persist_restore();
        if !tile_hook::hide_cursor() {
            tracing::debug!("some system cursors couldn't be hidden");
        }
        self.cursor_check_at = Some(Instant::now() + CURSOR_POLL);
    }

    /// Win + left-drag: the hook moves the window; here it is kept clear of tiling while it
    /// moves, and re-tiled where it is dropped.
    fn on_drag(&mut self, event: DragEvent) {
        match event {
            DragEvent::Start { window } => {
                let id = WindowId(window);
                if !self.desktops.is_managed(id) || self.desktops.is_hidden(id) {
                    return;
                }
                self.windows.focus(id);
                self.moving.insert(id);
                self.tweens.remove(&id);
                self.drag = Some(id);
            }
            DragEvent::End { x, y } => {
                let Some(id) = self.drag.take() else {
                    return;
                };
                self.moving.remove(&id);
                self.placed.remove(&id);
                if let Some(rect) = self.windows.frame(id) {
                    let effects = self
                        .desktops
                        .with_owner(id, |s| s.window_moved(id, rect, (x, y)));
                    self.apply(effects);
                }
            }
        }
    }

    /// Brings a hidden pointer back.
    fn show_cursor(&mut self) {
        if self.cursor_hidden.take().is_none() {
            return;
        }
        self.cursor_check_at = None;
        if !tile_hook::restore_cursors() {
            tracing::warn!("failed to restore the system cursors");
        }
        self.persist_restore();
    }

    /// Shows the hidden pointer once the mouse moved or a button is pressed.
    fn check_cursor(&mut self) {
        if self.cursor_hidden != Some(cursor_pos()) || mouse_button_down() {
            self.show_cursor();
        } else {
            self.cursor_check_at = Some(Instant::now() + CURSOR_POLL);
        }
    }

    fn refresh_monitors(&mut self) {
        let current = monitors();
        if current.is_empty() || current == self.monitors {
            return;
        }
        tracing::info!("monitors: {current:?}");
        self.monitors.clone_from(&current);
        let effects = self.desktops.set_monitors(current);
        self.apply(effects);
    }

    /// The monitor a window should join: under the mouse for new windows, where it already
    /// sits for existing ones. `None` falls back to the focused monitor.
    fn monitor_for(&self, rect: Rect, opened: bool) -> Option<usize> {
        let (x, y) = if opened { cursor_pos() } else { rect.center() };
        self.monitors
            .iter()
            .position(|m| m.rect.contains_point(x, y))
    }

    /// Starts managing a discovered window on the native desktop it lives on. `opened` is
    /// false for windows that already existed (startup / resume), which must not steal focus.
    fn manage(&mut self, found: Discovered, opened: bool) -> Vec<Effect> {
        let outcome = self.loaded.rules.resolve(&found.info);
        if outcome.ignore {
            self.windows.forget(found.id);
            self.ignored.insert(found.id);
            return Vec::new();
        }
        tracing::debug!("managing {:?} {:?}", found.id, found.info);
        if let Some(decorations) = outcome.decorations {
            self.decoration_rules.insert(found.id, decorations);
        }
        if self.loaded.config.general.borders {
            self.windows
                .set_border(found.id, Some(self.loaded.config.border_inactive()));
        }
        let desktop = vd::desktop_of(found.id.0).unwrap_or_else(|| self.desktops.current_id());
        let options = ManageOptions {
            floating: outcome.float.unwrap_or(found.floating_hint),
            workspace: outcome.workspace,
            monitor: outcome
                .monitor
                .or_else(|| self.monitor_for(found.rect, opened)),
            rect: found.rect,
            focus: opened && self.loaded.config.general.focus_new_windows,
        };
        self.desktops.open(desktop, found.id, found.info, options)
    }

    fn try_manage(&mut self, raw: isize) {
        if self.ignored.contains(&WindowId(raw)) {
            return;
        }
        if let Some(found) = self.windows.discover(raw) {
            // Windows shown on every desktop float above the tiling.
            if vd::is_pinned(raw) {
                self.windows.forget(found.id);
                return;
            }
            let effects = self.manage(found, true);
            self.apply(effects);
        }
    }

    fn unmanage(&mut self, id: WindowId) {
        tracing::debug!("unmanaging {id:?}");
        let alive = self.windows.is_alive(id);
        if self.undecorated.remove(&id) == Some(true) && alive {
            self.windows.set_decorated(id, true);
        }
        if self.desktops.is_hidden(id) && alive {
            self.windows.show(id);
        }
        self.decoration_rules.remove(&id);
        self.forced.remove(&id);
        let effects = self.desktops.close(id);
        self.windows.forget(id);
        self.placed.remove(&id);
        self.tweens.remove(&id);
        self.moving.remove(&id);
        self.pending_hide.remove(&id);
        self.pending_show.remove(&id);
        if self.bordered == Some(id) {
            self.bordered = None;
        }
        self.apply(effects);
    }

    /// Re-homes a window after it was moved to another native desktop or (un)pinned.
    fn check_desktop(&mut self, id: WindowId) {
        if vd::is_pinned(id.0) {
            if self.desktops.is_managed(id) {
                self.unmanage(id);
            }
            return;
        }
        let Some(desktop) = vd::desktop_of(id.0) else {
            return;
        };
        match self.desktops.owner(id) {
            None => self.try_manage(id.0),
            Some(owner) if owner != desktop => self.migrate(id, desktop),
            Some(_) => {}
        }
    }

    fn migrate(&mut self, id: WindowId, desktop: DesktopId) {
        tracing::debug!("{id:?} moved to desktop {desktop:032x}");
        let floating = self.desktops.is_floating(id);
        if self.desktops.is_hidden(id) {
            bump(&mut self.pending_show, id);
            self.windows.show(id);
        }
        let effects = self.desktops.close(id);
        self.apply(effects);
        self.placed.remove(&id);
        self.tweens.remove(&id);
        let info = self.windows.info(id).unwrap_or_default();
        let rect = self.windows.frame(id).unwrap_or_default();
        let options = ManageOptions {
            floating,
            rect,
            ..ManageOptions::default()
        };
        let effects = self.desktops.open(desktop, id, info, options);
        self.apply(effects);
    }

    fn switch_desktop(&mut self, desktop: DesktopId) {
        if desktop == self.desktops.current_id() {
            return;
        }
        tracing::debug!("switched to desktop {desktop:032x}");
        let effects = self.desktops.switch_to(desktop);
        self.apply(effects);
        // Windows on this desktop we haven't seen yet (e.g. created while it was hidden).
        self.discover_existing();
        if let Some(raw) = foreground() {
            self.on_foreground(raw);
        }
    }

    fn on_desktop(&mut self, change: DesktopChange) {
        if self.paused || !self.tiling {
            return;
        }
        match change {
            DesktopChange::Switched(desktop) => self.switch_desktop(desktop),
            DesktopChange::Window(raw) => self.check_desktop(WindowId(raw)),
            DesktopChange::Changed => {
                // A removed desktop's windows move to its neighbour.
                for id in self.desktops.managed() {
                    self.check_desktop(id);
                }
            }
        }
    }

    fn on_foreground(&mut self, raw: isize) {
        let id = WindowId(raw);
        tracing::info!(
            "foreground {raw:#x} {:?} managed={}",
            self.windows.info(id).map(|i| i.exe),
            self.desktops.is_managed(id)
        );
        if !self.desktops.is_managed(id) {
            self.try_manage(raw);
            if !self.desktops.is_managed(id) {
                if !is_shell_window(raw) {
                    let effects = self.desktops.current().window_focused(id);
                    self.apply(effects);
                }
                return;
            }
        }
        // Focus can arrive before the desktop switch notification.
        if self.desktops.owner(id) != Some(self.desktops.current_id())
            && let Some(live) = vd::current_desktop()
        {
            self.switch_desktop(live);
        }
        let effects = self.desktops.with_owner(id, |s| s.window_focused(id));
        self.apply(effects);
    }

    fn on_event(&mut self, event: WinEvent) {
        if self.paused || !self.tiling {
            return;
        }
        match event {
            WinEvent::Show(raw) | WinEvent::Uncloaked(raw) => {
                let id = WindowId(raw);
                if !self.desktops.is_managed(id) {
                    self.try_manage(raw);
                } else if !consume(&mut self.pending_show, id)
                    && self.desktops.is_hidden(id)
                    && self.desktops.owner(id) == Some(self.desktops.current_id())
                {
                    // The app re-showed a window we hid: follow it to its workspace.
                    let effects = self.desktops.with_owner(id, |s| s.window_focused(id));
                    self.apply(effects);
                }
            }
            WinEvent::Hide(raw) | WinEvent::Cloaked(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id)
                    && !consume(&mut self.pending_hide, id)
                    && !self.windows.is_visible(id)
                {
                    // The shell cloaks windows on other desktops: keep managing those.
                    if on_other_desktop(raw) {
                        self.check_desktop(id);
                    } else {
                        self.unmanage(id);
                    }
                }
            }
            WinEvent::Destroy(raw) => {
                let id = WindowId(raw);
                self.ignored.remove(&id);
                if self.desktops.is_managed(id) {
                    self.unmanage(id);
                }
            }
            WinEvent::Foreground(raw) => self.on_foreground(raw),
            WinEvent::MinimizeStart(raw) => {
                let id = WindowId(raw);
                let effects = self.desktops.with_owner(id, |s| s.window_minimized(id));
                self.apply(effects);
            }
            WinEvent::MinimizeEnd(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id) {
                    let effects = self.desktops.with_owner(id, |s| s.window_restored(id));
                    self.apply(effects);
                } else {
                    self.try_manage(raw);
                }
            }
            WinEvent::MoveSizeStart(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id) {
                    self.moving.insert(id);
                    self.resizing.remove(&id);
                    self.tweens.remove(&id);
                }
            }
            WinEvent::MoveSizeEnd(raw) => {
                let id = WindowId(raw);
                if self.moving.remove(&id) {
                    self.placed.remove(&id);
                    let resized = self.resizing.remove(&id);
                    if let Some(rect) = self.windows.frame(id) {
                        let cursor = cursor_pos();
                        let effects = self.desktops.with_owner(id, |s| {
                            if resized {
                                s.window_resized(id, rect)
                            } else {
                                s.window_moved(id, rect, cursor)
                            }
                        });
                        self.apply(effects);
                    }
                }
            }
            WinEvent::NameChange(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id) {
                    if self.focused() == Some(id)
                        && let Some(info) = self.windows.info(id)
                    {
                        self.desktops.current().set_info(id, info);
                        self.publish();
                    }
                } else if foreground() == Some(raw) {
                    // Some apps only set a title after showing their window.
                    self.try_manage(raw);
                }
            }
            WinEvent::LocationChange(raw) => {
                let id = WindowId(raw);
                if self.moving.contains(&id) {
                    self.follow_resize(id);
                } else {
                    self.enforce_size(id);
                }
            }
        }
    }

    /// While an edge of a tiled window is dragged, fits the other windows around it as it goes,
    /// without animating them, so they keep up with the pointer.
    fn follow_resize(&mut self, id: WindowId) {
        let Some(rect) = self.windows.frame(id) else {
            return;
        };
        let effects = self
            .desktops
            .with_owner(id, |s| s.window_resizing(id, rect));
        if effects.is_empty() {
            return;
        }
        self.resizing.insert(id);
        for effect in effects {
            if let Effect::Place { id: other, rect } = effect {
                // The window being dragged is skipped by `place`: the user holds it.
                if let Err(gone) = self.place(other, rect, false) {
                    self.ignored.insert(gone);
                    self.unmanage(gone);
                }
            }
        }
        self.update_borders();
    }

    /// Some apps enforce a minimum size larger than their tile. Re-place those with force so
    /// they fit; if one still refuses, at least slide it back inside the work area.
    fn enforce_size(&mut self, id: WindowId) {
        if self.moving.contains(&id)
            || self.tweens.contains_key(&id)
            || self.desktops.is_hidden(id)
            || self.desktops.is_floating(id)
        {
            return;
        }
        let Some(target) = self.placed.get(&id).copied() else {
            return;
        };
        let Some(frame) = self.windows.frame(id) else {
            return;
        };
        // Placement is asynchronous: only a window already at its tile's position but larger
        // than the tile has refused the size; anything else may simply not have moved yet.
        let at_target = (frame.x - target.x).abs() <= 2 && (frame.y - target.y).abs() <= 2;
        let refused = frame.w > target.w + 2 || frame.h > target.h + 2;
        if !at_target || !refused {
            return;
        }
        match self.forced.get(&id) {
            None => {
                tracing::debug!(
                    "{id:?} wants at least {}x{}; forcing its tile size",
                    frame.w,
                    frame.h
                );
                self.forced.insert(id, Instant::now());
                if self.place_now(id, target, true).is_err() {
                    self.unmanage(id);
                }
                return;
            }
            // Give the forced placement time to land before judging it.
            Some(at) if at.elapsed() < FORCE_GRACE => return,
            Some(_) => {}
        }
        let (cx, cy) = target.center();
        let Some(area) = self
            .monitors
            .iter()
            .find(|m| m.rect.contains_point(cx, cy))
            .map(|m| m.work_area)
        else {
            return;
        };
        if frame.right() <= area.right() && frame.bottom() <= area.bottom() {
            return;
        }
        let x = target.x.min(area.right() - frame.w).max(area.x);
        let y = target.y.min(area.bottom() - frame.h).max(area.y);
        tracing::debug!(
            "{id:?} resized itself to {}x{} even when forced; keeping it on screen",
            frame.w,
            frame.h
        );
        if self
            .place_now(id, Rect::new(x, y, frame.w, frame.h), true)
            .is_err()
        {
            self.unmanage(id);
        }
    }

    fn on_tray(&mut self, event: TrayEvent) {
        match event {
            TrayEvent::ToggleTile => self.set_paused(!self.paused),
            TrayEvent::ToggleRemap => self.set_remap_paused(!self.remap_paused),
            TrayEvent::ToggleShell => self.toggle_shell(),
            TrayEvent::ToggleLogging => self.toggle_logging(),
            // If the config file couldn't be created, open its folder instead.
            TrayEvent::OpenConfig => {
                if let Err(e) = tile_config::create_if_missing(&self.config_path) {
                    tracing::warn!("could not create {}: {e}", self.config_path.display());
                }
                launch::open(&self.config_path);
            }
            TrayEvent::OpenLogs => {
                // Nothing may have been logged yet.
                let _ = std::fs::create_dir_all(paths::log_dir());
                launch::open(&paths::log_dir());
            }
            TrayEvent::Quit => self.quit = true,
            TrayEvent::DisplayChanged => {
                if let Some(backdrop) = &self.backdrop {
                    backdrop.refresh();
                }
                if self.tiling {
                    self.refresh_monitors();
                }
            }
            TrayEvent::SessionChanged => {
                if let Some(remap) = &self.remap {
                    remap.send(remap::Command::Reset);
                }
                self.show_cursor();
            }
        }
    }

    fn on_ipc(&mut self, incoming: Incoming) {
        let Incoming { request, reply } = incoming;
        let response = match request {
            Request::Dispatch { command } => match command.parse::<Command>() {
                Ok(command) => {
                    self.run_command(command);
                    Response::Ok
                }
                Err(e) => Response::Error {
                    message: e.to_string(),
                },
            },
            Request::Query { what } => {
                let state = self.desktops.current_ref();
                match what {
                    Query::Monitors => Response::Monitors {
                        monitors: state.snapshot_monitors(),
                    },
                    Query::Workspaces => Response::Workspaces {
                        workspaces: state.snapshot_workspaces(),
                    },
                    Query::Windows => Response::Windows {
                        windows: state.snapshot_windows(),
                    },
                    Query::ActiveWindow => Response::ActiveWindow {
                        window: state.snapshot_active_window(),
                    },
                }
            }
            Request::Subscribe => {
                for event in self.current_events() {
                    let _ = reply.send(Response::Event { event });
                }
                self.subscribers.push(reply);
                return;
            }
        };
        let _ = reply.send(response);
    }

    /// Applies effects from the core state to the real windows.
    fn apply(&mut self, effects: Vec<Effect>) {
        let shown: HashSet<WindowId> = effects
            .iter()
            .filter_map(|e| {
                if let Effect::Show(id) = e {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        // Before the old window is hidden: hiding the focused window makes Windows pick another
        // one, possibly on another monitor, and we would follow it.
        if effects.contains(&Effect::Defocus) {
            tracing::info!("nothing to focus here: focusing the desktop");
            focus_desktop();
        }
        let mut failed = Vec::new();
        let mut focus_refused = false;
        for effect in effects {
            match effect {
                Effect::Place { id, rect } => {
                    if let Err(id) = self.place(id, rect, !shown.contains(&id)) {
                        failed.push(id);
                    }
                }
                Effect::Show(id) => {
                    bump(&mut self.pending_show, id);
                    self.windows.show(id);
                }
                Effect::Hide(id) => {
                    bump(&mut self.pending_hide, id);
                    self.tweens.remove(&id);
                    self.windows.hide(id);
                }
                Effect::Focus(id) => {
                    if !self.windows.focus(id) {
                        tracing::debug!("Windows refused to focus {id:?}");
                        focus_refused = true;
                    }
                }
                Effect::Defocus => {}
                Effect::Close(id) => self.windows.close(id),
                Effect::Exec(line) => launch::exec(&line),
            }
        }
        for id in failed {
            self.ignored.insert(id);
            self.unmanage(id);
        }
        self.update_decorations();
        self.update_borders();
        self.persist_restore();
        self.publish();
        // Keep our notion of focus (and the highlight) truthful when Windows said no.
        if focus_refused && !self.resyncing_focus {
            self.resyncing_focus = true;
            if let Some(raw) = foreground() {
                self.on_foreground(raw);
            }
            self.resyncing_focus = false;
        }
    }

    fn wants_decorations(&self, id: WindowId) -> bool {
        self.decoration_rules.get(&id).copied().unwrap_or_else(|| {
            self.loaded.config.general.decorations || self.desktops.is_floating(id)
        })
    }

    /// Adds or removes title bars so every managed window matches the config.
    fn update_decorations(&mut self) {
        for id in self.desktops.managed() {
            let want = self.wants_decorations(id);
            match (want, self.undecorated.get(&id).copied()) {
                (false, None) => {
                    let changed = self.windows.set_decorated(id, false);
                    self.undecorated.insert(id, changed);
                }
                (true, Some(changed)) => {
                    if changed {
                        self.windows.set_decorated(id, true);
                    }
                    self.undecorated.remove(&id);
                }
                _ => {}
            }
        }
    }

    fn place(&mut self, id: WindowId, rect: Rect, may_animate: bool) -> Result<(), WindowId> {
        if self.moving.contains(&id) {
            return Ok(());
        }
        let previous = self.placed.insert(id, rect);
        if previous == Some(rect) {
            return Ok(());
        }
        let animations = &self.loaded.config.animations;
        if animations.enabled
            && may_animate
            && !self.desktops.is_hidden(id)
            && self.desktops.owner(id) == Some(self.desktops.current_id())
            && let Some(previous) = previous
        {
            let now = std::time::Instant::now();
            // Retargeting continues from wherever the running animation is right now.
            let existing = self
                .tweens
                .remove(&id)
                .map(|t| (t.animation.at(now).0, t.mover));
            if let Some((from, mover)) =
                existing.or_else(|| Mover::spawn(id.0).map(|m| (previous, m)))
                && let Some(insets) = self.windows.insets_of(id)
            {
                let config = &self.loaded.config.animations;
                let animation = Animation {
                    from,
                    to: rect,
                    start: now,
                    duration: Duration::from_millis(config.duration_ms.max(1)),
                    curve: config.curve(),
                    insets,
                    force: self.forced.contains_key(&id),
                };
                mover.animate(animation);
                self.tweens.insert(id, Tween { animation, mover });
                return Ok(());
            }
        }
        self.tweens.remove(&id);
        self.place_now(id, rect, true)
    }

    /// `settled`: the final placement of a move (not an animation frame), which repaints.
    fn place_now(&mut self, id: WindowId, rect: Rect, settled: bool) -> Result<(), WindowId> {
        match self
            .windows
            .place(id, rect, self.forced.contains_key(&id), settled)
        {
            Ok(()) => Ok(()),
            Err(PlaceError::Gone) => Err(id),
            Err(PlaceError::AccessDenied) => {
                tracing::warn!("cannot control {id:?} (elevated?), ignoring it");
                Err(id)
            }
            Err(PlaceError::Other(e)) => {
                tracing::debug!("placing {id:?} failed: {e}");
                Ok(())
            }
        }
    }

    /// Drops windows that disappeared without us noticing.
    fn sweep(&mut self) {
        self.sync_shell();
        if !self.tiling {
            return;
        }
        let dead: Vec<_> = self
            .desktops
            .managed()
            .into_iter()
            .filter(|id| !self.windows.is_alive(*id))
            .collect();
        for id in dead {
            self.unmanage(id);
        }
        for id in self.desktops.managed() {
            if self.desktops.owner(id) == Some(self.desktops.current_id()) {
                self.enforce_size(id);
            }
        }
        self.retry_lock_override();
        self.heal_borders();
    }

    fn update_borders(&mut self) {
        let general = &self.loaded.config.general;
        if !general.borders {
            return;
        }
        let focused = self.desktops.current_ref().focused();
        if focused == self.bordered {
            return;
        }
        if let Some(old) = self.bordered {
            self.windows
                .set_border(old, Some(self.loaded.config.border_inactive()));
        }
        if let Some(new) = focused {
            self.windows
                .set_border(new, Some(self.loaded.config.border_active()));
        }
        self.bordered = focused;
        self.heal_at = Some(Instant::now() + HEAL_DELAY);
    }

    /// Re-applies border colors that apps overwrote (Chromium/Electron repaint their frame
    /// on activation and theme changes).
    fn heal_borders(&mut self) {
        let general = &self.loaded.config.general;
        if !general.borders {
            return;
        }
        let focused = self.desktops.current_ref().focused();
        let current = self.desktops.current_id();
        for id in self.desktops.managed() {
            if self.desktops.owner(id) != Some(current) || self.desktops.is_hidden(id) {
                continue;
            }
            let want = Some(if Some(id) == focused {
                self.loaded.config.border_active()
            } else {
                self.loaded.config.border_inactive()
            });
            // DWM doesn't report border colors on current Windows builds (E_INVALIDARG), so
            // when it can't say, just re-assert ours: a no-op when unchanged.
            if self.windows.border_is(id, want) != Some(true) {
                self.windows.set_border(id, want);
            }
        }
    }

    fn repaint_borders(&mut self) {
        let general = &self.loaded.config.general;
        let color = general
            .borders
            .then_some(self.loaded.config.border_inactive());
        for id in self.desktops.managed() {
            self.windows.set_border(id, color);
        }
        self.bordered = None;
    }

    /// Records what must be undone if we die, for the panic hook and the watchdog.
    fn persist_restore(&mut self) {
        let mut restore = Restore {
            hidden: self.desktops.hidden().into_iter().map(|id| id.0).collect(),
            undecorated: self
                .undecorated
                .iter()
                .filter(|(_, changed)| **changed)
                .map(|(id, _)| id.0)
                .collect(),
            lock_disabled: self.lock_taken,
            cursor_hidden: self.cursor_hidden.is_some(),
        };
        restore.hidden.sort_unstable();
        restore.undecorated.sort_unstable();
        if restore != self.saved_restore {
            paths::save_restore(&restore);
            self.saved_restore = restore;
        }
    }

    fn snapshot(&self) -> Published {
        let state = self.desktops.current_ref();
        Published {
            workspace: state.focused_workspace(),
            window: state
                .snapshot_active_window()
                .map(|w| (w.id, w.title, w.exe)),
            layout: state.active_layout(),
        }
    }

    fn current_events(&self) -> Vec<Event> {
        let now = self.snapshot();
        let mut events = Vec::new();
        events.extend(now.workspace.map(|id| Event::Workspace { id }));
        events.push(match now.window {
            Some((id, title, exe)) => Event::ActiveWindow {
                id: Some(id),
                title: Some(title),
                exe: Some(exe),
            },
            None => Event::ActiveWindow {
                id: None,
                title: None,
                exe: None,
            },
        });
        events.extend(now.layout.map(|layout| Event::Layout { layout }));
        events
    }

    /// Notifies IPC subscribers about changes to the workspace, active window and layout.
    fn publish(&mut self) {
        let now = self.snapshot();
        if now == self.published {
            return;
        }
        let mut events = Vec::new();
        if now.workspace != self.published.workspace
            && let Some(id) = now.workspace
        {
            events.push(Event::Workspace { id });
        }
        if now.window != self.published.window {
            events.push(match &now.window {
                Some((id, title, exe)) => Event::ActiveWindow {
                    id: Some(*id),
                    title: Some(title.clone()),
                    exe: Some(exe.clone()),
                },
                None => Event::ActiveWindow {
                    id: None,
                    title: None,
                    exe: None,
                },
            });
        }
        if now.layout != self.published.layout
            && let Some(layout) = now.layout
        {
            events.push(Event::Layout { layout });
        }
        self.published = now;
        self.subscribers.retain(|tx| {
            events.iter().all(|event| {
                tx.send(Response::Event {
                    event: event.clone(),
                })
                .is_ok()
            })
        });
    }
}
