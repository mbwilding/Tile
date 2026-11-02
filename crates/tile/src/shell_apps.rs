//! Whether the Store app support suits this build of Windows, found by trying it in a throwaway
//! process (`tile --probe-shell`) once per build and remembering the answer.
//!
//! The support calls undocumented Windows internals, so a servicing update can turn a call into a
//! crash. Trying it first costs a process; losing the shell helper to it would cost every app
//! launched while it restarts.

use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tile_shell::probe::{self, Error};

use crate::paths::data_dir;

/// How long the probe gets before it is ended and counted as a failure.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The probe's exit code when it couldn't be run, which says nothing about Windows.
const INCONCLUSIVE: i32 = 3;

/// What the probe found for one build of Windows and of Tile.
#[derive(Serialize, Deserialize)]
struct Verdict {
    fingerprint: String,
    ok: bool,
    reason: String,
}

fn cache_file() -> std::path::PathBuf {
    data_dir().join("shell-compat.json")
}

/// What identifies the build under test: Windows' files and this program.
fn fingerprint() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| fs::metadata(exe).ok())
        .map(|meta| {
            let secs = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            format!("tile:{}:{secs}", meta.len())
        })
        .unwrap_or_default();
    format!("{};{exe}", probe::fingerprint())
}

/// Whether the Store app support may start: `Ok` if the last probe of this build passed (the
/// probe runs now if there is none), or the reason it is off.
pub fn verify() -> Result<(), String> {
    let fingerprint = fingerprint();
    let known = fs::read_to_string(cache_file())
        .ok()
        .and_then(|text| serde_json::from_str::<Verdict>(&text).ok())
        .filter(|v| v.fingerprint == fingerprint);
    if let Some(verdict) = known {
        return if verdict.ok {
            Ok(())
        } else {
            Err(format!(
                "this build of Windows failed the check ({}); delete {} to try again",
                verdict.reason,
                cache_file().display()
            ))
        };
    }
    tracing::info!("checking the Store app support on this build of Windows");
    match run_probe() {
        Probe::Passed => {
            remember(&fingerprint, true, String::new());
            tracing::info!("the Store app support passed the check");
            Ok(())
        }
        Probe::Failed(reason) => {
            remember(&fingerprint, false, reason.clone());
            Err(format!("this build of Windows failed the check ({reason})"))
        }
        Probe::Inconclusive(why) => {
            tracing::warn!("couldn't check the Store app support, starting it anyway: {why}");
            Ok(())
        }
    }
}

fn remember(fingerprint: &str, ok: bool, reason: String) {
    let verdict = Verdict {
        fingerprint: fingerprint.to_owned(),
        ok,
        reason,
    };
    let saved = fs::create_dir_all(data_dir()).and_then(|()| {
        fs::write(
            cache_file(),
            serde_json::to_string(&verdict).unwrap_or_default(),
        )
    });
    if let Err(e) = saved {
        tracing::warn!("couldn't save the Store app check: {e}");
    }
}

enum Probe {
    Passed,
    Failed(String),
    Inconclusive(String),
}

/// Runs `tile --probe-shell` and reads how it ended.
fn run_probe() -> Probe {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Probe::Inconclusive(e.to_string()),
    };
    let mut command = Command::new(exe);
    command
        .arg("--probe-shell")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match tile_sys::hide_console(&mut command).spawn() {
        Ok(child) => child,
        Err(e) => return Probe::Inconclusive(e.to_string()),
    };
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Probe::Failed("it didn't finish".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Probe::Inconclusive(e.to_string()),
        }
    };
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut said);
    }
    let said = said.trim().to_owned();
    match status.code() {
        Some(0) => Probe::Passed,
        Some(INCONCLUSIVE) => Probe::Inconclusive(said),
        Some(code) if !said.is_empty() => Probe::Failed(format!("{said} (exit {code})")),
        // A crash ends with the exception's code, such as 0xC0000005.
        Some(code) => Probe::Failed(format!("it crashed (exit {:#x})", code as u32)),
        None => Probe::Failed("it was ended".into()),
    }
}

/// `tile --probe-shell`: runs the check and exits with how it went.
pub fn probe_and_exit() -> ! {
    let code = match probe::run() {
        Ok(()) => 0,
        Err(Error::Failed(reason)) => {
            println!("{reason}");
            1
        }
        Err(Error::Inconclusive(reason)) => {
            println!("{reason}");
            INCONCLUSIVE
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_has_a_fingerprint() {
        assert!(fingerprint().contains("CoreMessaging.dll:"));
    }

    #[test]
    fn a_verdict_survives_json() {
        let verdict = Verdict {
            fingerprint: "a;b".into(),
            ok: false,
            reason: "it crashed (exit 0xc0000005)".into(),
        };
        let json = serde_json::to_string(&verdict).unwrap();
        let back: Verdict = serde_json::from_str(&json).unwrap();
        assert_eq!((back.fingerprint, back.ok), ("a;b".into(), false));
    }
}
