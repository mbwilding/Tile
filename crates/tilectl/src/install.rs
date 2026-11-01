//! `tilectl install` / `uninstall`.
//!
//! Tile is installed to `C:\Program Files\Tile` and started at sign-in by a scheduled task that
//! runs it elevated, so it can manage admin windows without a UAC prompt at every sign-in. The
//! programs it starts go through the launcher (`tile.exe --launcher`, started by the user's
//! `Run` key), so they run as the normal user. `tilectl` goes on the user's `PATH`.
//!
//! Key, the remapper that is now part of Tile, is stopped and its install removed, so keys
//! aren't remapped twice.
//!
//! The admin part runs in a second `tilectl` started through UAC ([`elevated`]), which reports
//! back over a named pipe.

use std::{
    fs, io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, ToNsName,
    tokio::{Stream, prelude::*},
};
use tile_ipc::{
    Client, Request,
    launcher::{self, LaunchRequest},
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use winsafe::{self as w, co, prelude::*};

/// How long a running Tile or launcher may take to exit.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the elevated step may take once the UAC prompt was accepted.
const ELEVATED_TIMEOUT: Duration = Duration::from_secs(60);

/// The scheduled task that starts Tile.
const TASK: &str = "Tile";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const LAUNCHER_RUN_VALUE: &str = "Tile launcher";
/// The `Run` value earlier, non-elevated installs used for Tile itself.
const OLD_RUN_VALUE: &str = "Tile";
/// The pipe and scheduled task of Key, the standalone remapper Tile absorbed.
const KEY_SOCKET: &str = "key.sock";
const KEY_TASK: &str = "Key";
const ENV_KEY: &str = "Environment";
const PATH_VALUE: &str = "Path";

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The programs `install` copies.
const EXES: [&str; 2] = ["tile.exe", "tilectl.exe"];

/// The admin part of `install` / `uninstall`.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Step {
    /// Copy the programs from `source`, then register and start the task for `user`.
    Install {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        user: String,
    },
    /// Remove the task and the programs.
    Uninstall,
}

/// `C:\Program Files\Tile`. Tile runs elevated, so it must live where only admins can write.
fn install_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
        .join("Tile")
}

/// `%LOCALAPPDATA%\Programs\Tile`, where earlier versions installed Tile.
fn old_install_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Programs")
        .join("Tile")
}

/// `C:\Program Files\Key`, where Key was installed.
fn key_install_dir() -> PathBuf {
    install_dir().with_file_name("Key")
}

/// The folder holding the build to install: this `tilectl.exe`'s own, or for the installed
/// `tilectl`, `target\release` under the current folder (a Tile checkout).
fn build_dir(dir: &Path) -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let own = exe.parent().context("no folder for tilectl.exe")?;
    let source = if own == dir {
        std::env::current_dir()?.join("target").join("release")
    } else {
        own.to_owned()
    };
    for name in EXES {
        if !source.join(name).exists() {
            bail!(
                "no build to install: {} not found. Run `cargo build --release --workspace` in a Tile \
                 checkout, then `tilectl install` from there",
                source.join(name).display()
            );
        }
    }
    Ok(source)
}

/// Installs the build (see the module docs) and starts it, replacing a running Tile.
pub async fn install() -> anyhow::Result<()> {
    let dir = install_dir();
    let source = build_dir(&dir)?;
    stop_tile().await?;
    stop_launcher().await?;
    stop_key().await?;
    println!("Installing to {} (needs admin rights)...", dir.display());
    let note = run_elevated(&Step::Install {
        source,
        user: current_user()?,
    })
    .await?;
    let tile = dir.join("tile.exe");
    set_run(
        LAUNCHER_RUN_VALUE,
        Some(&format!("\"{}\" --launcher", tile.display())),
    )
    .context("failed to register the launcher to start at sign-in")?;
    set_on_path(&dir, true)?;
    remove_old_install();
    start_detached(&tile, &["--launcher"]).context("failed to start the launcher")?;
    println!("Installed. {note}");
    println!("`tilectl` is on your PATH in terminals started after your next sign-in.");
    Ok(())
}

/// Stops Tile and undoes [`install`]. The config and logs are kept.
pub async fn uninstall() -> anyhow::Result<()> {
    stop_tile().await?;
    stop_launcher().await?;
    set_run(LAUNCHER_RUN_VALUE, None).context("failed to remove the launcher from sign-in")?;
    let dir = install_dir();
    set_on_path(&dir, false)?;
    remove_old_install();
    println!("Removing {} (needs admin rights)...", dir.display());
    run_elevated(&Step::Uninstall).await?;
    println!("Uninstalled. Your config (%APPDATA%\\Tile) and logs were kept.");
    Ok(())
}

/// Runs `step` in a `tilectl` started through UAC and waits for its report: a note to show
/// on success, or the error.
async fn run_elevated(step: &Step) -> anyhow::Result<String> {
    let report = format!("tile-install-{}.sock", std::process::id());
    let listener = ListenerOptions::new()
        .name(report.as_str().to_ns_name::<GenericNamespaced>()?)
        .create_tokio()?;
    let exe = std::env::current_exe()?;
    let mut parameters = format!("elevated --report \"{report}\"");
    match step {
        Step::Install { source, user } => {
            parameters += &format!(
                " install --source \"{}\" --user \"{user}\"",
                source.display()
            );
        }
        Step::Uninstall => parameters += " uninstall",
    }
    // Returns once the prompt is answered.
    let started = w::ShellExecuteEx(&w::SHELLEXECUTEINFO {
        verb: Some("runas"),
        file: &exe.to_string_lossy(),
        parameters: Some(&parameters),
        show: co::SW::HIDE,
        ..Default::default()
    });
    match started {
        Err(co::ERROR::CANCELLED) => bail!("admin rights were declined"),
        Err(e) => bail!("failed to start the admin step: {e}"),
        Ok(()) => {}
    }
    let conn = tokio::time::timeout(ELEVATED_TIMEOUT, listener.accept())
        .await
        .context("the admin step didn't report back")??;
    let mut line = String::new();
    BufReader::new(&conn).read_line(&mut line).await?;
    match line.trim_end().split_once(':') {
        Some(("ok", note)) => Ok(note.to_owned()),
        Some(("error", message)) => bail!("{message}"),
        _ => bail!("the admin step failed"),
    }
}

/// `tilectl elevated`: does `step` (we run as administrator) and reports to `report`.
pub async fn elevated(step: Step, report: &str) -> anyhow::Result<()> {
    let line = match run_step(&step) {
        Ok(note) => format!("ok:{note}\n"),
        Err(e) => format!("error:{e:#}\n"),
    };
    let conn = Stream::connect(report.to_ns_name::<GenericNamespaced>()?).await?;
    (&conn).write_all(line.as_bytes()).await?;
    // The write finishes in the background: exiting now could cancel it, so wait for the
    // installer to read the report and hang up.
    let _ = (&conn).read_to_end(&mut Vec::new()).await;
    Ok(())
}

fn run_step(step: &Step) -> anyhow::Result<String> {
    let dir = install_dir();
    match step {
        Step::Install { source, user } => {
            copy_programs(source, &dir)?;
            register_task(&dir.join("tile.exe"), user)
                .context("failed to register the sign-in task")?;
            delete_task(KEY_TASK).context("failed to remove Key's sign-in task")?;
            remove_programs(&key_install_dir())?;
            // We run as administrator: started by us, Tile is too. Unless the prompt was
            // answered with another account, whose desktop it must not start on.
            if current_user()?.eq_ignore_ascii_case(user) {
                start_detached(&dir.join("tile.exe"), &[]).context("failed to start Tile")?;
                Ok("Tile is running as administrator and starts at sign-in.".into())
            } else {
                Ok("Tile starts as administrator at your next sign-in.".into())
            }
        }
        Step::Uninstall => {
            delete_task(TASK).context("failed to remove the sign-in task")?;
            remove_programs(&dir)?;
            Ok(String::new())
        }
    }
}

/// Copies the programs into `dir`. A running exe can't be overwritten but can be renamed, so
/// old ones are moved aside, and those earlier updates moved aside are deleted once unused.
fn copy_programs(source: &Path, dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    for path in fs::read_dir(dir)?.flatten().map(|e| e.path()) {
        if path.extension().is_some_and(|ext| ext == "old") {
            let _ = fs::remove_file(path);
        }
    }
    for name in EXES {
        let target = dir.join(name);
        if target.exists() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            fs::rename(&target, target.with_extension(format!("{nanos}.old")))
                .with_context(|| format!("failed to move {} aside", target.display()))?;
        }
        fs::copy(source.join(name), &target)
            .with_context(|| format!("failed to copy to {}", target.display()))?;
    }
    Ok(())
}

/// Removes `dir`. A running `tilectl.exe` in it can't be deleted yet, so `cmd` finishes the job
/// once it has exited.
fn remove_programs(dir: &Path) -> anyhow::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound && dir.exists() => {
            Command::new("cmd")
                .raw_arg(format!(
                    "/C ping -n 3 127.0.0.1 >nul & rmdir /S /Q \"{}\"",
                    dir.display()
                ))
                .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
                .spawn()
                .with_context(|| format!("failed to delete {}", dir.display()))?;
        }
        _ => {}
    }
    Ok(())
}

fn task_service() -> anyhow::Result<w::ITaskService> {
    let service = w::CoCreateInstance::<w::ITaskService>(
        &co::CLSID::TaskScheduler,
        None::<&w::IUnknown>,
        co::CLSCTX::INPROC_SERVER,
    )?;
    service.Connect(None, None, None, None)?;
    Ok(service)
}

/// Registers the task that starts `exe` elevated when `user` signs in.
fn register_task(exe: &Path, user: &str) -> anyhow::Result<()> {
    let _com = w::CoInitializeEx(co::COINIT::APARTMENTTHREADED)?;
    let service = task_service()?;
    let folder = service.GetFolder("\\")?;
    let task = service.NewTask()?;
    task.get_Principal()?
        .put_RunLevel(co::TASK_RUNLEVEL::HIGHEST)?;
    // The defaults would stop Tile after 3 days or on battery, and run it at low priority.
    let settings = task.get_Settings()?;
    settings.put_DisallowStartIfOnBatteries(false)?;
    settings.put_StopIfGoingOnBatteries(false)?;
    settings.put_ExecutionTimeLimit("PT0S")?;
    settings.put_MultipleInstances(co::TASK_INSTANCES::IGNORE_NEW)?;
    settings.put_Priority(4)?;
    let trigger = task
        .get_Triggers()?
        .Create(co::TASK_TRIGGER_TYPE2::LOGON)?
        .QueryInterface::<w::ILogonTrigger>()?;
    trigger.put_UserId(user)?;
    let action = task
        .get_Actions()?
        .Create(co::TASK_ACTION_TYPE::EXEC)?
        .QueryInterface::<w::IExecAction>()?;
    action.put_Path(&exe.to_string_lossy())?;
    folder.RegisterTaskDefinition(
        Some(TASK),
        &task,
        co::TASK_CREATION::CREATE_OR_UPDATE,
        Some(user),
        None,
        co::TASK_LOGON::INTERACTIVE_TOKEN,
        None,
    )?;
    Ok(())
}

fn delete_task(name: &str) -> anyhow::Result<()> {
    let _com = w::CoInitializeEx(co::COINIT::APARTMENTTHREADED)?;
    let folder = task_service()?.GetFolder("\\")?;
    if folder.GetTask(name).is_ok() {
        folder.DeleteTask(name)?;
    }
    Ok(())
}

/// `DOMAIN\user` of whoever runs `tilectl`: the task must run as them, even when the UAC
/// prompt is answered with another account's credentials.
fn current_user() -> anyhow::Result<String> {
    let user = std::env::var("USERNAME").context("USERNAME isn't set")?;
    Ok(match std::env::var("USERDOMAIN") {
        Ok(domain) => format!("{domain}\\{user}"),
        Err(_) => user,
    })
}

/// Asks a running Tile to exit and waits until it has.
async fn stop_tile() -> anyhow::Result<()> {
    if !tile_ipc::is_running().await {
        return Ok(());
    }
    Client::send(&Request::Dispatch {
        command: "exit".into(),
    })
    .await?;
    wait_until_stopped(tile_ipc::is_running, "Tile").await?;
    println!("Stopped the running Tile.");
    Ok(())
}

/// Asks a running launcher to exit and waits until it has.
async fn stop_launcher() -> anyhow::Result<()> {
    if !launcher::is_running().await {
        return Ok(());
    }
    launcher::send(&LaunchRequest::Exit).await?;
    if wait_until_stopped(launcher::is_running, "launcher")
        .await
        .is_ok()
    {
        return Ok(());
    }
    // It can hang while exiting; Tile is already stopped, so what's left is the launcher.
    let _ = Command::new("taskkill")
        .args(["/f", "/im", "tile.exe"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    wait_until_stopped(launcher::is_running, "launcher").await
}

async fn key_running() -> bool {
    match KEY_SOCKET.to_ns_name::<GenericNamespaced>() {
        Ok(name) => Stream::connect(name).await.is_ok(),
        Err(_) => false,
    }
}

/// Asks a running Key to exit (its pipe takes an `exit` line) and waits until it has.
async fn stop_key() -> anyhow::Result<()> {
    if !key_running().await {
        return Ok(());
    }
    let conn = Stream::connect(KEY_SOCKET.to_ns_name::<GenericNamespaced>()?).await?;
    (&conn).write_all(b"exit\n").await?;
    BufReader::new(&conn).read_line(&mut String::new()).await?;
    wait_until_stopped(key_running, "Key").await?;
    println!("Stopped the running Key: Tile remaps keys now.");
    Ok(())
}

async fn wait_until_stopped<F: Future<Output = bool>>(
    running: fn() -> F,
    what: &str,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while running().await {
        if Instant::now() > deadline {
            bail!(
                "the running {what} didn't exit; quit it from the tray or Task Manager and try again"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Removes what earlier, per-user versions installed (their `Run` value, `PATH` entry and
/// folder) and Key's `PATH` entry.
fn remove_old_install() {
    let old = old_install_dir();
    let _ = set_on_path(&key_install_dir(), false);
    let _ = set_run(OLD_RUN_VALUE, None);
    let _ = set_on_path(&old, false);
    let _ = fs::remove_dir_all(&old);
}

/// Starts `exe` without a console and outside our job object if allowed, so it outlives us
/// (e.g. when `cargo run` kills its job on exit).
fn start_detached(exe: &Path, args: &[&str]) -> io::Result<()> {
    let launch = |flags: u32| {
        Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    launch(base | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| launch(base))
        .map(drop)
}

fn io_error(e: co::ERROR) -> io::Error {
    io::Error::from_raw_os_error(e.raw() as i32)
}

/// Sets the user's `Run` value `name` to `command`, or removes it with `None`.
fn set_run(name: &str, command: Option<&str>) -> io::Result<()> {
    match command {
        Some(command) => w::HKEY::CURRENT_USER.RegSetKeyValue(
            Some(RUN_KEY),
            Some(name),
            w::RegistryValue::Sz(command.to_owned()),
        ),
        None => w::HKEY::CURRENT_USER
            .RegOpenKeyEx(Some(RUN_KEY), co::REG_OPTION::default(), co::KEY::SET_VALUE)
            .and_then(|key| key.RegDeleteValue(Some(name)))
            .or_else(|e| {
                if e == co::ERROR::FILE_NOT_FOUND {
                    Ok(())
                } else {
                    Err(e)
                }
            }),
    }
    .map_err(io_error)
}

/// Adds `dir` to (`present`) or removes it from the user's `PATH`, keeping its other entries
/// and its registry type. Terminals see the change after the next sign-in.
fn set_on_path(dir: &Path, present: bool) -> anyhow::Result<()> {
    let flags = co::RRF::RT_REG_SZ | co::RRF::RT_REG_EXPAND_SZ | co::RRF::NOEXPAND;
    let (path, expand) =
        match w::HKEY::CURRENT_USER.RegGetValue(Some(ENV_KEY), Some(PATH_VALUE), flags) {
            Ok(w::RegistryValue::Sz(s)) => (s, false),
            Ok(w::RegistryValue::ExpandSz(s)) => (s, true),
            _ => (String::new(), true),
        };
    let Some(path) = edit_path(&path, &dir.to_string_lossy(), present) else {
        return Ok(());
    };
    let value = if expand {
        w::RegistryValue::ExpandSz(path)
    } else {
        w::RegistryValue::Sz(path)
    };
    w::HKEY::CURRENT_USER
        .RegSetKeyValue(Some(ENV_KEY), Some(PATH_VALUE), value)
        .map_err(io_error)
        .context("failed to update your PATH")
}

/// `path` with `dir` added or removed, or `None` if it already is the way `present` asks.
/// Entries match case-insensitively, ignoring a trailing backslash.
fn edit_path(path: &str, dir: &str, present: bool) -> Option<String> {
    let same = |entry: &str| {
        entry
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(dir.trim_end_matches('\\'))
    };
    let mut entries: Vec<&str> = path.split(';').filter(|e| !e.is_empty()).collect();
    if entries.iter().any(|e| same(e)) == present {
        return None;
    }
    if present {
        entries.push(dir);
    } else {
        entries.retain(|e| !same(e));
    }
    Some(entries.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Program Files\Tile";

    #[test]
    fn adds_once_keeping_other_entries() {
        let path = r"%USERPROFILE%\bin;C:\tools;";
        assert_eq!(
            edit_path(path, DIR, true).unwrap(),
            format!(r"%USERPROFILE%\bin;C:\tools;{DIR}")
        );
        assert_eq!(edit_path(&format!(r"C:\tools;{DIR}\"), DIR, true), None);
        assert_eq!(edit_path("", DIR, true).unwrap(), DIR);
    }

    #[test]
    fn removes_every_matching_entry() {
        let path = format!(r"C:\tools;{};%USERPROFILE%\bin;{DIR}\", DIR.to_lowercase());
        assert_eq!(
            edit_path(&path, DIR, false).unwrap(),
            r"C:\tools;%USERPROFILE%\bin"
        );
        assert_eq!(edit_path(r"C:\tools", DIR, false), None);
    }
}
