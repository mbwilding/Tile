//! The Win+L override and its permission.

use tokio::time::Instant;

use super::{Daemon, RELOCK_DELAY};
use crate::paths;

/// The state of Tile's Win+L takeover.
#[derive(Debug, Default)]
pub(super) struct WinL {
    /// We set the policy that disables Win+L (and must remove it again).
    pub(super) taken: bool,
    /// We already asked (UAC) for permission to manage the policy this run.
    grant_requested: bool,
    /// After the `lock` dispatcher lifted the policy: when to re-apply it.
    pub(super) relock_at: Option<Instant>,
}

impl Daemon {
    /// Takes over or gives back Win+L to match `general.override_win_l` (while tiling).
    pub(super) fn update_lock_override(&mut self) {
        let want = self.tiling && self.loaded.config.general.override_win_l;
        // Leave a policy the user (or an admin) set themselves alone.
        if want && !self.win_l.taken && !tile_platform::lock_disabled() {
            if tile_platform::set_lock_disabled(true) {
                self.win_l.taken = true;
                tracing::info!("took over Win+L");
            } else {
                self.request_lock_permission();
            }
        } else if !want && self.win_l.taken {
            tile_platform::set_lock_disabled(false);
            self.win_l.taken = false;
            tracing::info!("gave Win+L back to Windows");
        }
        self.persist_restore();
    }

    /// The policy key is admin-only on some machines. Ask once (one UAC prompt) for the user
    /// to be allowed to manage it; `sweep` retries the takeover until that lands.
    pub(super) fn request_lock_permission(&mut self) {
        if self.win_l.grant_requested {
            return;
        }
        self.win_l.grant_requested = true;
        tracing::warn!(
            "Win+L needs a one-time permission; asking Windows (UAC). Win+L binds wait until then"
        );
        let script = paths::data_dir().join("grant-win-l.ps1");
        if let Err(e) = tile_platform::request_lock_permission(&script) {
            tracing::error!("could not request Win+L permission: {e}");
        }
    }

    /// Retries taking over Win+L after the permission prompt.
    pub(super) fn retry_lock_override(&mut self) {
        let want = self.loaded.config.general.override_win_l;
        if want
            && self.win_l.grant_requested
            && !self.win_l.taken
            && tile_platform::set_lock_disabled(true)
        {
            self.win_l.taken = true;
            tracing::info!("took over Win+L");
            self.persist_restore();
            self.install_binds();
        }
    }

    /// Whether Win+L can be bound right now (Windows' lock shortcut is disabled).
    pub(super) fn win_l_available(&self) -> bool {
        self.win_l.taken || tile_platform::lock_disabled()
    }

    /// The `lock` dispatcher: locks the workstation, re-applying the policy shortly after if it
    /// had to be lifted.
    pub(super) fn lock(&mut self) {
        if tile_platform::lock_workstation() {
            self.win_l.relock_at = Some(Instant::now() + RELOCK_DELAY);
        }
    }

    /// Re-applies the policy `lock` lifted.
    pub(super) fn relock(&mut self) {
        self.win_l.relock_at = None;
        if self.win_l.taken {
            tile_platform::set_lock_disabled(true);
        }
    }
}
