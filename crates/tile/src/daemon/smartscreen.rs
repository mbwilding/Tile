//! Turning SmartScreen's app check off while Tile hosts the notification area, and back on when
//! it doesn't.

use super::Daemon;
use crate::paths;

/// The state of Tile's SmartScreen override.
#[derive(Debug, Default)]
pub(super) struct SmartScreen {
    /// We turned the app check off (and must put it back).
    pub(super) taken: bool,
    /// We already asked (UAC) for permission to manage the policy this run.
    grant_requested: bool,
}

impl Daemon {
    /// Turns the app check off: called once Tile starts hosting the notification area, since
    /// that is when SmartScreen's warning has nowhere to show (see `docs/shell.md#smartscreen`).
    pub(super) fn take_smartscreen(&mut self) {
        if self.smartscreen.taken {
            return;
        }
        if tile_platform::set_smartscreen_disabled(true) {
            self.smartscreen.taken = true;
            tracing::info!("turned the SmartScreen app check off while hosting the shell");
            self.persist_restore();
        } else {
            self.request_smartscreen_permission();
        }
    }

    /// Puts the app check back: called once Tile stops hosting the notification area.
    pub(super) fn give_back_smartscreen(&mut self) {
        if self.smartscreen.taken || tile_platform::smartscreen_overridden() {
            tile_platform::set_smartscreen_disabled(false);
            self.smartscreen.taken = false;
            tracing::info!("restored the SmartScreen app check");
            self.persist_restore();
        }
    }

    /// The policy key is admin-only on some machines. Ask once (one UAC prompt) for the user to
    /// be allowed to manage it; `sweep` retries the takeover until that lands.
    pub(super) fn request_smartscreen_permission(&mut self) {
        if self.smartscreen.grant_requested {
            return;
        }
        self.smartscreen.grant_requested = true;
        tracing::warn!("turning SmartScreen off needs a one-time permission; asking Windows (UAC)");
        let script = paths::data_dir().join("grant-smartscreen.ps1");
        if let Err(e) = tile_platform::request_smartscreen_permission(&script) {
            tracing::error!("could not request SmartScreen permission: {e}");
        }
    }

    /// Retries taking the app check over after the permission prompt.
    pub(super) fn retry_smartscreen_override(&mut self) {
        if self.notify_host.is_some()
            && self.smartscreen.grant_requested
            && !self.smartscreen.taken
            && tile_platform::set_smartscreen_disabled(true)
        {
            self.smartscreen.taken = true;
            tracing::info!("turned the SmartScreen app check off while hosting the shell");
            self.persist_restore();
        }
    }
}
