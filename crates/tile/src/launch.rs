//! Starting programs for `exec` and the tray. An elevated Tile hands them to the launcher
//! (`tile --launcher`, running as the normal user), so they don't inherit its admin rights.

use std::{
    os::windows::process::CommandExt,
    path::Path,
    process::Stdio,
    sync::OnceLock,
    time::{Duration, Instant},
};

use tile_ipc::launcher::{self, LaunchRequest, LaunchServer};

/// Runs a command line through `start`, detached and without a console window.
fn run(command_line: &str) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let result = std::process::Command::new("cmd")
        .raw_arg(format!("/C start \"\" {command_line}"))
        .creation_flags(CREATE_NO_WINDOW)
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

/// Starts a Store app by its app user model id as the normal user. Must be called inside the
/// Tokio runtime.
pub fn activate(aumid: &str) {
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

/// Whether `tile install` registered the sign-in task, which starts Tile elevated.
pub fn sign_in_task_exists() -> bool {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("schtasks")
        .args(["/query", "/tn", "Tile"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|status| status.success())
}

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
    let mut serving = tokio::spawn(async move {
        loop {
            match server.next().await {
                Ok(LaunchRequest::Exec { command_line }) => run(&command_line),
                Ok(LaunchRequest::Activate { aumid }) => activate_now(&aumid),
                Ok(LaunchRequest::Exit) => break,
                Err(e) => tracing::debug!("bad launch request: {e}"),
            }
        }
    });
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
    loop {
        match server.next().await {
            Ok(LaunchRequest::Exec { command_line }) => run(&command_line),
            Ok(LaunchRequest::Activate { aumid }) => activate_now(&aumid),
            Ok(LaunchRequest::Exit) => break,
            Err(e) => tracing::debug!("bad launch request: {e}"),
        }
    }
    tracing::info!("launcher stopped");
    Ok(())
}
