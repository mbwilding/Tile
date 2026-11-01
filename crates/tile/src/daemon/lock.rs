//! The Win+L override and its permission.

use tokio::time::Instant;

use super::{Daemon, RELOCK_DELAY};
use crate::paths;

impl Daemon {
    /// Takes over or gives back Win+L to match `general.override_win_l` (while tiling).
    pub(super) fn update_lock_override(&mut self) {
        let want = self.tiling && self.loaded.config.general.override_win_l;
        // Leave a policy the user (or an admin) set themselves alone.
        if want && !self.lock_taken && !tile_platform::lock_disabled() {
            if tile_platform::set_lock_disabled(true) {
                self.lock_taken = true;
                tracing::info!("took over Win+L");
            } else {
                self.request_lock_permission();
            }
        } else if !want && self.lock_taken {
            tile_platform::set_lock_disabled(false);
            self.lock_taken = false;
            tracing::info!("gave Win+L back to Windows");
        }
        self.persist_restore();
    }

    /// Switches Click to Do off if `general.disable_click_to_do` asks for it. It stays off after
    /// Tile exits: Windows pops up a notice on every flip.
    pub(super) fn switch_off_click_to_do(&self) {
        if self.tiling
            && self.loaded.config.general.disable_click_to_do
            && tile_platform::click_to_do_available()
            && !tile_platform::click_to_do_disabled()
            && tile_platform::disable_click_to_do()
        {
            tracing::info!("switched off Click to Do");
        }
    }

    /// The policy key is admin-only on some machines. Ask once (one UAC prompt) for the user
    /// to be allowed to manage it; `sweep` retries the takeover until that lands.
    pub(super) fn request_lock_permission(&mut self) {
        if self.lock_grant_requested {
            return;
        }
        self.lock_grant_requested = true;
        tracing::warn!(
            "Win+L needs a one-time permission; asking Windows (UAC). Win+L binds wait until then"
        );
        let script = paths::data_dir().join("grant-win-l.ps1");
        let written = std::fs::create_dir_all(paths::data_dir())
            .and_then(|()| std::fs::write(&script, tile_platform::GRANT_SCRIPT));
        if let Err(e) = written {
            tracing::error!("could not write {}: {e}", script.display());
            return;
        }
        let spawned = tile_sys::hide_console(
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script),
        )
        .spawn();
        if let Err(e) = spawned {
            tracing::error!("could not request Win+L permission: {e}");
        }
    }

    /// Retries taking over Win+L after the permission prompt.
    pub(super) fn retry_lock_override(&mut self) {
        let want = self.loaded.config.general.override_win_l;
        if want
            && self.lock_grant_requested
            && !self.lock_taken
            && tile_platform::set_lock_disabled(true)
        {
            self.lock_taken = true;
            tracing::info!("took over Win+L");
            self.persist_restore();
            self.install_binds();
        }
    }

    /// Whether Win+L can be bound right now (Windows' lock shortcut is disabled).
    pub(super) fn win_l_available(&self) -> bool {
        self.lock_taken || tile_platform::lock_disabled()
    }

    pub(super) fn lock(&mut self) {
        if tile_platform::lock_workstation() {
            self.relock_at = Some(Instant::now() + RELOCK_DELAY);
        }
    }
}
