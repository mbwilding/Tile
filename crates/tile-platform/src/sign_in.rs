//! Making Tile the program Windows starts at sign-in in place of Explorer.
//!
//! Windows starts the program named by the `Shell` value under
//! `Software\Microsoft\Windows NT\CurrentVersion\Winlogon`. A value in the user's own hive
//! overrides the machine's `explorer.exe`, so this needs no admin rights and affects only this
//! user; removing it brings Explorer back at the next sign-in.
//!
//! File Explorer windows still open: with `SeparateProcess` on, each folder window runs in its
//! own process and doesn't bring the desktop and taskbar with it.

use std::path::{Path, PathBuf};

use winsafe::{self as w, co};

const WINLOGON_KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";
const SHELL_VALUE: &str = "Shell";
const EXPLORER_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const SEPARATE_VALUE: &str = "SeparateProcess";
/// Where the `SeparateProcess` value from before Tile took over is kept, to put back.
const SAVED_KEY: &str = r"Software\Tile";
const SAVED_VALUE: &str = "ShellPreviousSeparateProcess";
/// Stands in for "the value didn't exist".
const ABSENT: u32 = u32::MAX;

/// The installed `tile.exe`, where the shell has to live.
pub fn installed_exe() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
        .join("Tile")
        .join("tile.exe")
}

fn dword(key: &str, name: &str) -> Option<u32> {
    match w::HKEY::CURRENT_USER.RegGetValue(Some(key), Some(name), co::RRF::RT_REG_DWORD) {
        Ok(w::RegistryValue::Dword(v)) => Some(v),
        _ => None,
    }
}

fn set_dword(key: &str, name: &str, value: u32) -> Result<(), String> {
    w::HKEY::CURRENT_USER
        .RegSetKeyValue(Some(key), Some(name), w::RegistryValue::Dword(value))
        .map_err(|e| format!("failed to write {key}\\{name}: {e}"))
}

fn delete_value(key: &str, name: &str) -> Result<(), String> {
    match w::HKEY::CURRENT_USER
        .RegOpenKeyEx(Some(key), co::REG_OPTION::default(), co::KEY::SET_VALUE)
        .and_then(|k| k.RegDeleteValue(Some(name)))
    {
        Ok(()) | Err(co::ERROR::FILE_NOT_FOUND | co::ERROR::PATH_NOT_FOUND) => Ok(()),
        Err(e) => Err(format!("failed to remove {key}\\{name}: {e}")),
    }
}

/// Whether Windows starts Tile as this user's shell at sign-in.
pub fn is_shell() -> bool {
    match w::HKEY::CURRENT_USER.RegGetValue(
        Some(WINLOGON_KEY),
        Some(SHELL_VALUE),
        co::RRF::RT_REG_SZ,
    ) {
        Ok(w::RegistryValue::Sz(text)) => text.to_lowercase().contains("tile.exe"),
        _ => false,
    }
}

/// Makes `exe` (with `--shell`) the user's shell from the next sign-in, or with `None` puts
/// Explorer back, if the shell is Tile's.
pub fn set_shell(exe: Option<&Path>) -> Result<(), String> {
    match exe {
        Some(exe) => {
            if dword(SAVED_KEY, SAVED_VALUE).is_none() {
                let previous = dword(EXPLORER_KEY, SEPARATE_VALUE).unwrap_or(ABSENT);
                set_dword(SAVED_KEY, SAVED_VALUE, previous)?;
            }
            set_dword(EXPLORER_KEY, SEPARATE_VALUE, 1)?;
            w::HKEY::CURRENT_USER
                .RegSetKeyValue(
                    Some(WINLOGON_KEY),
                    Some(SHELL_VALUE),
                    w::RegistryValue::Sz(format!("\"{}\" --shell", exe.display())),
                )
                .map_err(|e| format!("failed to set the shell: {e}"))
        }
        None => {
            // Someone else's shell isn't ours to remove.
            let ours = matches!(
                w::HKEY::CURRENT_USER.RegGetValue(
                    Some(WINLOGON_KEY),
                    Some(SHELL_VALUE),
                    co::RRF::RT_REG_SZ
                ),
                Ok(w::RegistryValue::Sz(shell)) if shell.to_lowercase().contains("tile.exe")
            );
            if ours {
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
