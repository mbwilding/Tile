//! Making Tile the program Windows starts at sign-in in place of Explorer.
//!
//! Windows starts the program named by the `Shell` value under
//! `Software\Microsoft\Windows NT\CurrentVersion\Winlogon`. A value in the user's own hive
//! overrides the machine's `explorer.exe`, so this needs no admin rights and affects only this
//! user; removing it brings Explorer back at the next sign-in.
//!
//! File Explorer windows still open: with `SeparateProcess` on, each folder window runs in its
//! own process and doesn't bring the desktop and taskbar with it.

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::registry::{self, delete_value, dword, set_dword};

const WINLOGON_KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";
const SHELL_VALUE: &str = "Shell";
const EXPLORER_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const SEPARATE_VALUE: &str = "SeparateProcess";
/// Where the `SeparateProcess` value from before Tile took over is kept, to put back.
const SAVED_KEY: &str = r"Software\Tile";
const SAVED_VALUE: &str = "ShellPreviousSeparateProcess";
/// Stands in for "the value didn't exist".
const ABSENT: u32 = u32::MAX;

/// Windows' console host, which can run a console program with no window at all.
pub fn conhost() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("System32")
        .join("conhost.exe")
}

/// The command line that runs `exe` with `args` and no console window, for places that start a
/// console program without a way to hide it (the `Run` key, the sign-in shell).
pub fn headless_command(exe: &Path, args: &str) -> String {
    let command = format!(
        "\"{}\" --headless \"{}\"",
        conhost().display(),
        exe.display()
    );
    if args.is_empty() {
        command
    } else {
        format!("{command} {args}")
    }
}

/// Whether Windows starts Tile as this user's shell at sign-in.
pub fn is_shell() -> bool {
    registry::sz(WINLOGON_KEY, SHELL_VALUE)
        .is_some_and(|text| text.to_lowercase().contains("tile.exe"))
}

/// Makes `exe` (with `--shell`) the user's shell from the next sign-in, or with `None` puts
/// Explorer back, if the shell is Tile's.
pub fn set_shell(exe: Option<&Path>) -> io::Result<()> {
    match exe {
        Some(exe) => {
            if dword(SAVED_KEY, SAVED_VALUE).is_none() {
                let previous = dword(EXPLORER_KEY, SEPARATE_VALUE).unwrap_or(ABSENT);
                set_dword(SAVED_KEY, SAVED_VALUE, previous)?;
            }
            set_dword(EXPLORER_KEY, SEPARATE_VALUE, 1)?;
            registry::set_sz(WINLOGON_KEY, SHELL_VALUE, &headless_command(exe, "--shell"))
        }
        None => {
            // Someone else's shell isn't ours to remove.
            if is_shell() {
                delete_value(WINLOGON_KEY, SHELL_VALUE)?;
            }
            if let Some(saved) = dword(SAVED_KEY, SAVED_VALUE) {
                if saved == ABSENT {
                    delete_value(EXPLORER_KEY, SEPARATE_VALUE)?;
                } else {
                    set_dword(EXPLORER_KEY, SEPARATE_VALUE, saved)?;
                }
                delete_value(SAVED_KEY, SAVED_VALUE)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_commands_run_the_program_through_conhost() {
        let exe = Path::new(r"C:\Program Files\Tile\Tile.exe");
        let command = headless_command(exe, "--launcher");
        assert!(command.starts_with('"'));
        assert!(
            command
                .contains(r#"conhost.exe" --headless "C:\Program Files\Tile\Tile.exe" --launcher"#)
        );
        assert!(headless_command(exe, "").ends_with(r#"Tile.exe""#));
    }

    #[test]
    fn conhost_lives_in_system32() {
        assert!(conhost().ends_with(r"System32\conhost.exe"));
    }
}
