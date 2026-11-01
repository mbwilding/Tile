//! Turning the SmartScreen app check off machine-wide while Tile is the shell.
//!
//! SmartScreen's "Windows protected your PC" warning for an unrecognised downloaded program is
//! shown through a shell service only Explorer provides (see `docs/shell.md#smartscreen`).
//! Without it the check still runs, finds no way to ask, and the launch is silently cancelled
//! instead of offered. While Tile hosts the notification area this turns the check off with the
//! `EnableSmartScreen` policy, and restores whatever value was there before as soon as it stops.

use std::{io, path::Path, process::Command};

use winsafe::{self as w, co};

use crate::registry;

const POLICY_KEY: &str = r"SOFTWARE\Policies\Microsoft\Windows\System";
const POLICY_VALUE: &str = "EnableSmartScreen";
/// Where the policy value from before Tile took over is kept, to put back.
const SAVED_KEY: &str = r"Software\Tile";
const SAVED_VALUE: &str = "SmartScreenPreviousEnable";
/// Stands in for "the value didn't exist".
const ABSENT: u32 = u32::MAX;

/// The current `EnableSmartScreen` policy value. Reading it needs no admin rights.
fn policy() -> Option<u32> {
    match w::HKEY::LOCAL_MACHINE.RegGetValue(
        Some(POLICY_KEY),
        Some(POLICY_VALUE),
        co::RRF::RT_REG_DWORD,
    ) {
        Ok(w::RegistryValue::Dword(v)) => Some(v),
        _ => None,
    }
}

fn set_policy(value: u32) -> io::Result<()> {
    w::HKEY::LOCAL_MACHINE
        .RegSetKeyValue(
            Some(POLICY_KEY),
            Some(POLICY_VALUE),
            w::RegistryValue::Dword(value),
        )
        .map_err(|e| io::Error::other(format!("couldn't write {POLICY_KEY}\\{POLICY_VALUE}: {e}")))
}

fn delete_policy() -> io::Result<()> {
    match w::HKEY::LOCAL_MACHINE
        .RegOpenKeyEx(
            Some(POLICY_KEY),
            co::REG_OPTION::default(),
            co::KEY::SET_VALUE,
        )
        .and_then(|k| k.RegDeleteValue(Some(POLICY_VALUE)))
    {
        Ok(()) | Err(co::ERROR::FILE_NOT_FOUND | co::ERROR::PATH_NOT_FOUND) => Ok(()),
        Err(e) => Err(io::Error::other(format!(
            "couldn't remove {POLICY_KEY}\\{POLICY_VALUE}: {e}"
        ))),
    }
}

/// Whether Tile currently has the app check turned off.
pub fn smartscreen_overridden() -> bool {
    registry::dword(SAVED_KEY, SAVED_VALUE).is_some()
}

fn turn_off() -> io::Result<()> {
    if registry::dword(SAVED_KEY, SAVED_VALUE).is_none() {
        let previous = policy().unwrap_or(ABSENT);
        registry::set_dword(SAVED_KEY, SAVED_VALUE, previous)?;
    }
    set_policy(0)
}

fn restore() -> io::Result<()> {
    let Some(saved) = registry::dword(SAVED_KEY, SAVED_VALUE) else {
        return Ok(());
    };
    if saved == ABSENT {
        delete_policy()?;
    } else {
        set_policy(saved)?;
    }
    registry::delete_value(SAVED_KEY, SAVED_VALUE)
}

/// Turns the app check off (`disabled`) or puts back the value from before, saving it the first
/// time. Returns whether it worked; the policy key is admin-only until
/// [`request_smartscreen_permission`] has been granted.
pub fn set_smartscreen_disabled(disabled: bool) -> bool {
    let result = if disabled { turn_off() } else { restore() };
    if let Err(e) = &result {
        tracing::debug!(
            "failed to {} the SmartScreen app check: {e}",
            if disabled { "turn off" } else { "restore" }
        );
    }
    result.is_ok()
}

/// PowerShell that asks for elevation once (UAC) and lets the current user manage the policy
/// key. Only `QueryValues` and `SetValue` on that one key are granted.
const GRANT_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$inner = @"
`$p = 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Policies\Microsoft\Windows\System'
if (-not (Test-Path `$p)) { New-Item -Path `$p -Force | Out-Null }
`$acl = Get-Acl `$p
`$id = New-Object Security.Principal.SecurityIdentifier('$sid')
`$rule = New-Object Security.AccessControl.RegistryAccessRule(`$id, 'QueryValues, SetValue', 'Allow')
`$acl.AddAccessRule(`$rule)
Set-Acl -Path `$p -AclObject `$acl
"@
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($inner))
Start-Process powershell -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile', '-EncodedCommand', $encoded
"#;

/// Asks Windows (one UAC prompt) for the current user to be allowed to manage the
/// `EnableSmartScreen` policy, by writing a script to `script` and starting it. Returns once it
/// has started; the permission lands when the prompt is accepted.
pub fn request_smartscreen_permission(script: &Path) -> io::Result<()> {
    if let Some(dir) = script.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(script, GRANT_SCRIPT)?;
    tile_sys::hide_console(
        Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script),
    )
    .spawn()
    .map(drop)
}
