//! Taking over Win+L.
//!
//! Windows handles Win+L below any keyboard hook, so the only way to use it as a
//! keybind is the per-user `DisableLockWorkstation` policy. While it is set, locking
//! goes through [`lock_workstation`], which lifts the policy just long enough to lock.

use std::{io, path::Path, process::Command};

use winsafe as w;

use crate::registry;

const POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\System";
const POLICY_VALUE: &str = "DisableLockWorkstation";

/// Whether locking is currently disabled by the per-user policy.
pub fn lock_disabled() -> bool {
    registry::dword(POLICY_KEY, POLICY_VALUE).is_some_and(|v| v != 0)
}

/// Disables (`true`) or re-enables (`false`) Win+L and workstation locking.
/// Returns whether it worked; the policy key is admin-only on some machines.
pub fn set_lock_disabled(disabled: bool) -> bool {
    let result = if disabled {
        registry::set_dword(POLICY_KEY, POLICY_VALUE, 1)
    } else {
        registry::delete_value(POLICY_KEY, POLICY_VALUE)
    };
    if let Err(e) = &result {
        tracing::debug!(
            "failed to {} Win+L: {e}",
            if disabled { "take over" } else { "restore" }
        );
    }
    result.is_ok()
}

/// PowerShell that asks for elevation once (UAC) and lets the current user manage the
/// policy key. Only `QueryValues` and `SetValue` on that one key are granted.
const GRANT_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$inner = @"
`$p = 'Registry::HKEY_USERS\$sid\Software\Microsoft\Windows\CurrentVersion\Policies\System'
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

/// Asks Windows (one UAC prompt) for the current user to be allowed to manage the Win+L
/// policy, by writing a script to `script` and starting it. Returns once it has started; the
/// permission lands when the prompt is accepted.
pub fn request_lock_permission(script: &Path) -> io::Result<()> {
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

/// Locks the workstation, temporarily lifting our policy if needed.
/// Returns whether the policy was lifted, so the caller can re-apply it shortly after.
pub fn lock_workstation() -> bool {
    let lifted = lock_disabled() && set_lock_disabled(false);
    if let Err(e) = w::LockWorkStation() {
        tracing::warn!("LockWorkStation failed: {e}");
    }
    lifted
}
