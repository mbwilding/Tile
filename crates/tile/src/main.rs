mod cli;
mod daemon;
mod install;
mod launch;
mod logging;
mod paths;
mod remap;
mod settings;
mod task;

use std::path::PathBuf;

use clap::Parser;
use tile_config::Loaded;

/// A dynamic tiling window manager and key remapper for Windows. Without a command
/// it runs the window manager; the commands control one that is running.
#[derive(Parser)]
#[command(version = env!("TILE_VERSION"), about)]
struct Args {
    #[command(subcommand)]
    command: Option<cli::Cmd>,
    /// Config file to use instead of %APPDATA%\Tile\config.yml.
    #[arg(long, short)]
    config: Option<PathBuf>,
    /// Log filter, e.g. `info` or `tile=debug`; also switches logging on. `RUST_LOG` takes
    /// precedence. Without either, the tray's Logging setting decides (default off, `info`).
    #[arg(long)]
    log: Option<String>,
    /// Internal: wait for the daemon with this process id to exit, then undo its changes.
    #[arg(long, hide = true)]
    watchdog: Option<u32>,
    /// Internal: start programs for an elevated Tile, as the normal user.
    #[arg(long, hide = true)]
    launcher: bool,
    /// Run as the Windows shell, as the tray's Shell switch sets up: host the notification area
    /// and start the sign-in programs, whether or not the tray's Shell switch is on.
    #[arg(long)]
    shell: bool,
}

/// Runs in a separate process so windows are restored even if the daemon is killed.
fn watchdog(pid: u32) {
    // A process we can't watch might still be running; restoring now would undo its work.
    if !tile_platform::wait_for_exit(pid) {
        return;
    }
    let restore = paths::load_restore();
    if !restore.is_empty() {
        restore.apply();
        paths::save_restore(&paths::Restore::default());
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if let Some(command) = args.command {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(cli::run(command));
    }
    // Started without a terminal (sign-in, a task, Explorer), there is no console to keep.
    tile_sys::detach_console();
    if let Some(pid) = args.watchdog {
        watchdog(pid);
        return Ok(());
    }
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(tile_config::default_path);
    settings::init(&config_path);
    if args.launcher {
        let _guard = logging::init("launcher.log", args.log.as_deref());
        log_panics();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(launch::serve());
    }
    // Signed in as the shell without admin rights while the sign-in task starts an elevated
    // Tile: serve it as the launcher, and take over only if the task never starts Tile.
    let helper = args.shell && !tile_platform::is_elevated() && task::exists(task::TILE);
    let _guard = logging::init(
        if helper { "launcher.log" } else { "tile.log" },
        args.log.as_deref(),
    );
    if helper {
        log_panics();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        if runtime.block_on(launch::serve_for_tile())? {
            return Ok(());
        }
        tracing::warn!("the sign-in task didn't start Tile; running it here");
    }
    let Some(_instance) = tile_sys::single_instance("Tile") else {
        tracing::error!("Tile is already running");
        anyhow::bail!("Tile is already running");
    };
    // The watchdog can go down with a closing terminal, so a Tile run from one cleans up itself too.
    tile_sys::on_console_close(|| paths::load_restore().apply());

    // Never leave windows hidden or without title bars, even if we crash.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(
            "panic: {info}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        paths::load_restore().apply();
        previous(info);
    }));

    if let Err(e) = tile_config::create_if_missing(&config_path) {
        tracing::warn!("could not create {}: {e}", config_path.display());
    }
    let (loaded, config_error) = match Loaded::load_or_default(&config_path) {
        Ok(loaded) => (loaded, None),
        Err(e) => {
            tracing::error!("{e}; using the built-in defaults");
            (Loaded::defaults(), Some(e.summary()))
        }
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(daemon::run(config_path, loaded, config_error, args.shell));
    if let Err(e) = &result {
        tracing::error!("{e:#}");
    }
    result
}

/// Writes a panic and where it happened to the log before the process goes down.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(
            "panic: {info}
{}",
            std::backtrace::Backtrace::force_capture()
        );
        previous(info);
    }));
}
