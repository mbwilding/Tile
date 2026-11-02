//! A check, run in a throwaway process, that the Windows internals the Store app support calls
//! behave as Tile expects on this build of Windows.
//!
//! Those calls go through vtable slots and exports found by reading Windows 11 (build 26100). A
//! build that moves one makes the call crash or fail. [`run`] makes each of them once, so the
//! caller learns that in a process it can afford to lose, instead of in the shell.

use std::{
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
};

/// How long the session gets to come up.
const HOST_WAIT: Duration = Duration::from_secs(5);
/// How long the session runs before the check calls it a pass.
const HOST_RUN: Duration = Duration::from_millis(700);

/// Why a check didn't pass.
#[derive(Debug)]
pub enum Error {
    /// Something Tile expects of Windows isn't so.
    Failed(String),
    /// The check couldn't be made, for a reason that says nothing about Windows (another
    /// process is already the shell).
    Inconclusive(String),
}

/// What [`fingerprint`] looks at: the files the support's internals live in. A servicing update
/// that changes one changes this.
const FILES: [&str; 6] = [
    "ApplicationFrame.dll",
    "CoreMessaging.dll",
    "win32u.dll",
    "twinui.dll",
    "Windows.UI.dll",
    "Windows.UI.Immersive.dll",
];

/// Identifies the build of Windows by the files the support depends on: their sizes and
/// modification times.
pub fn fingerprint() -> String {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let dir = std::path::Path::new(&root).join("System32");
    FILES
        .iter()
        .filter_map(|name| {
            let meta = std::fs::metadata(dir.join(name)).ok()?;
            let secs = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some(format!("{name}:{}:{secs}", meta.len()))
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Makes every internal call once. Run it in a process of its own: a bad slot ends the process.
pub fn run() -> Result<(), Error> {
    // SAFETY: takes no pointers; stops Windows showing a crash dialog for this process.
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
    // The frames first: it needs the shell window, and stops there if another process has it,
    // before a second session could get in the way of that process's.
    crate::frame::probe()?;
    host()
}

/// Creates the session and the lifecycle manager, and runs the session for a moment.
fn host() -> Result<(), Error> {
    let worker = thread::spawn(crate::uwp::host);
    let deadline = Instant::now() + HOST_WAIT;
    while !crate::uwp::hosted() {
        if worker.is_finished() {
            return Err(stopped(worker, "the session didn't come up"));
        }
        if Instant::now() > deadline {
            return Err(Error::Failed("the session took too long to come up".into()));
        }
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(HOST_RUN);
    if worker.is_finished() {
        return Err(stopped(worker, "the session stopped at once"));
    }
    Ok(())
}

/// The failure of a host thread that ended, with `what` and the reason it gave.
fn stopped(worker: thread::JoinHandle<Result<(), String>>, what: &str) -> Error {
    let reason = worker
        .join()
        .unwrap_or_else(|_| Err("it panicked".into()))
        .err()
        .unwrap_or_default();
    Error::Failed(format!("{what}: {reason}"))
}
