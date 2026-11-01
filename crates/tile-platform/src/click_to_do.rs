//! Windows' Click to Do (Copilot+ PCs): holding Win shows a glow around the pointer, ready
//! for Win + click. It gets in the way of Win-based keybinds, so Tile can switch it off.
//!
//! This flips the same per-user switch as Settings → Privacy & security → Click to Do. Windows
//! announces every flip, so Tile switches it off once and leaves it off.

use std::{path::Path, process::Command};

use crate::registry;

const KEY: &str = r"Software\Microsoft\Windows\Shell\ClickToDo";
const VALUE: &str = "DisableClickToDo";
const EXE: &str =
    r"C:\Windows\SystemApps\MicrosoftWindows.Client.CoreAI_cw5n1h2txyewy\ClickToDo.exe";

/// Whether this PC has Click to Do at all.
pub fn click_to_do_available() -> bool {
    Path::new(EXE).exists()
}

/// Whether Click to Do is switched off.
pub fn click_to_do_disabled() -> bool {
    registry::dword(KEY, VALUE).is_some_and(|v| v != 0)
}

/// Switches Click to Do off and stops it. Returns whether the switch could be changed.
pub fn disable_click_to_do() -> bool {
    if let Err(e) = registry::set_dword(KEY, VALUE, 1) {
        tracing::warn!("failed to disable Click to Do: {e}");
        return false;
    }
    // The running instance doesn't notice the switch.
    let killed =
        tile_sys::hide_console(Command::new("taskkill").args(["/IM", "ClickToDo.exe", "/F"]))
            .spawn();
    if let Err(e) = killed {
        tracing::debug!("could not stop Click to Do: {e}");
    }
    true
}
