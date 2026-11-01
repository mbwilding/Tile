//! Starting programs for `exec` and the tray. An elevated Tile hands them to the launcher
//! (`tile --launcher`, running as the normal user), so they don't inherit its admin rights.

use std::{
    os::windows::process::CommandExt,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant},
};

use tile_ipc::launcher::{self, LaunchRequest, LaunchServer};

/// Runs a command line through `start`, detached and without a console window.
fn run(command_line: &str) {
    let result = tile_sys::hide_console(
        std::process::Command::new("cmd").raw_arg(format!("/C start \"\" {command_line}")),
    )
    .spawn();
    if let Err(e) = result {
        tracing::error!("failed to run `{command_line}`: {e}");
    }
}

fn elevated() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(tile_platform::is_elevated)
}

/// How many times a request waits for the launcher to start (it can come up a moment after an
/// elevated Tile at sign-in) and how long between tries.
const LAUNCHER_TRIES: u32 = 20;
const LAUNCHER_RETRY: Duration = Duration::from_millis(250);

/// Sends `request` to the launcher, waiting a few seconds for it to appear.
async fn send_to_launcher(request: &LaunchRequest) -> std::io::Result<()> {
    let mut last = Ok(());
    for _ in 0..LAUNCHER_TRIES {
        last = launcher::send(request).await;
        if last.is_ok() {
            break;
        }
        tokio::time::sleep(LAUNCHER_RETRY).await;
    }
    last
}

/// Runs a command line as the normal user. Must be called inside the Tokio runtime.
pub fn exec(command_line: &str) {
    if !elevated() {
        run(command_line);
        return;
    }
    let command_line = command_line.to_owned();
    tokio::spawn(async move {
        let request = LaunchRequest::Exec {
            command_line: command_line.clone(),
        };
        if let Err(e) = send_to_launcher(&request).await {
            tracing::warn!("no launcher ({e}); running `{command_line}` as administrator");
            run(&command_line);
        }
    });
}

/// Starts an app Start lists by its `shell:AppsFolder` id, as the normal user or, with `admin`,
/// as administrator. Must be called inside the Tokio runtime.
pub fn start_app(id: &str, admin: bool) {
    // A Store app's id has a `!`. `shell:AppsFolder` can't start one without Explorer, so it is
    // started through the activation manager.
    let store = id.contains('!');
    match (store, admin) {
        (true, false) => activate(id),
        (true, true) => activate_as_admin(id),
        (false, false) => exec(&format!("\"shell:AppsFolder\\{id}\"")),
        (false, true) => tile_platform::run_as_admin(&format!("shell:AppsFolder\\{id}")),
    }
}

/// Starts a Store app by its app user model id as the normal user.
fn activate(aumid: &str) {
    if !elevated() {
        activate_now(aumid);
        return;
    }
    let aumid = aumid.to_owned();
    tokio::spawn(async move {
        let request = LaunchRequest::Activate {
            aumid: aumid.clone(),
        };
        if let Err(e) = send_to_launcher(&request).await {
            tracing::warn!("no launcher ({e}); starting `{aumid}` as administrator");
            activate_now(&aumid);
        }
    });
}

/// Starts a Store app from this process, so it keeps Tile's rights when Tile is elevated.
fn activate_as_admin(aumid: &str) {
    if !elevated() {
        tracing::warn!("`{aumid}` can only start as administrator when Tile runs elevated");
    }
    activate_now(aumid);
}

/// Starts the Store app on a thread of its own, which sets up COM for it.
fn activate_now(aumid: &str) {
    let aumid = aumid.to_owned();
    std::thread::spawn(move || {
        if let Err(e) = tile_shell::uwp::activate(&aumid) {
            tracing::warn!("{e}");
        }
    });
}

/// Opens a file or folder with its default program.
pub fn open(path: &Path) {
    exec(&format!("\"{}\"", path.display()));
}

/// How long `tile --shell` waits for the sign-in task to start Tile.
const SHELL_WAIT: Duration = Duration::from_secs(30);

/// `tile --shell` without admin rights: serves launch requests like `--launcher` for the
/// elevated Tile the sign-in task starts. Returns `true` when asked to exit, and `false` if no
/// Tile appeared in time, for the caller to run Tile itself.
pub async fn serve_for_tile() -> anyhow::Result<bool> {
    if launcher::is_running().await {
        return Ok(true);
    }
    let server = LaunchServer::bind()?;
    tracing::info!("launcher started, waiting for Tile");
    // As the shell, this process answers for Store apps the way Explorer would.
    tile_shell::uwp::start();
    let mut serving = tokio::spawn(serve_requests(server));
    let deadline = Instant::now() + SHELL_WAIT;
    let mut seen = false;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = &mut serving => {
                tracing::info!("launcher stopped");
                return Ok(true);
            }
            _ = tick.tick(), if !seen => {
                if tile_ipc::is_running().await {
                    seen = true;
                } else if Instant::now() > deadline {
                    serving.abort();
                    return Ok(false);
                }
            }
        }
    }
}

/// `tile --launcher`: runs launch requests until asked to exit. Returns at once if a launcher
/// is already running.
pub async fn serve() -> anyhow::Result<()> {
    if launcher::is_running().await {
        return Ok(());
    }
    let server = LaunchServer::bind()?;
    tracing::info!("launcher started");
    // Restarting Tile replaces the sign-in copy of this process with this one, so it answers for
    // Store apps whenever Tile is the shell.
    if tile_platform::is_sign_in_shell() {
        tile_shell::uwp::start();
    }
    serve_requests(server).await;
    tracing::info!("launcher stopped");
    Ok(())
}

/// Carries out a launch request. Returns `false` for [`LaunchRequest::Exit`].
fn handle(request: LaunchRequest) -> bool {
    match request {
        LaunchRequest::Exec { command_line } => run(&command_line),
        LaunchRequest::Activate { aumid } => activate_now(&aumid),
        LaunchRequest::Exit => return false,
    }
    true
}

/// Carries out launch requests until one asks to exit.
async fn serve_requests(server: LaunchServer) {
    loop {
        match server.next().await {
            Ok(request) => {
                if !handle(request) {
                    break;
                }
            }
            Err(e) => tracing::debug!("bad launch request: {e}"),
        }
    }
}
