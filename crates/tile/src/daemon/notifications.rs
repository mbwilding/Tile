//! Popups for Windows toast notifications, read with `UserNotificationListener`
//! (see [`tile_sys::notifications`]). Off by default: `notifications.enabled` turns it on.

use tile_sys::notifications::{self, Notification};

use super::Daemon;

impl Daemon {
    /// Starts or stops the notification listener to match `notifications.enabled`, and applies
    /// the popup's current settings either way.
    pub(super) fn update_notifications(&mut self) {
        self.toast.set_settings(super::toast_settings(&self.loaded));
        if !self.loaded.config.notifications.enabled {
            self.notify_listener = None;
            return;
        }
        if self.notify_listener.is_some() {
            return;
        }
        // The first time this is turned on, Windows may ask the user once; after that it
        // remembers the answer itself, the same as any other `GetAccessStatus` caller.
        if notifications::access_status() == Some(notifications::Access::Unspecified) {
            notifications::request_access();
        }
        let tx = self.notify_tx.clone();
        match notifications::Listener::spawn(move |notification| {
            let _ = tx.send(notification);
        }) {
            Some(listener) => {
                self.notify_listener = Some(listener);
                tracing::info!("notification popups started");
            }
            None => tracing::warn!(
                "notifications.enabled is on, but Tile doesn't have permission to read \
                 notifications (check Settings, Notifications, for Tile)"
            ),
        }
    }

    /// Shows `notification` as a popup on the focused monitor, or the first one if nothing is
    /// focused.
    pub(super) fn show_notification(&mut self, notification: Notification) {
        let area = self
            .desktops
            .current_ref()
            .focused_monitor()
            .map(|m| m.work_area)
            .or_else(|| tile_platform::monitors().first().map(|m| m.work_area));
        if let Some(area) = area {
            self.toast.show(notification, area);
        }
    }
}
