//! Popups for Windows toast notifications, read with `UserNotificationListener`
//! (see [`tile_sys::notifications`]). `notifications.enabled` chooses when: left unset, only
//! while Tile hosts the shell, since that is when Explorer's own toasts don't show.

use tile_sys::notifications::{self, Access, Notification};

use super::Daemon;

impl Daemon {
    /// Whether notification popups should be running right now.
    fn notifications_wanted(&self) -> bool {
        self.loaded
            .config
            .notifications
            .enabled
            .unwrap_or_else(|| self.notify_host.is_some())
    }

    /// Applies the popup's current settings, and starts or stops the listener to match
    /// [`Self::notifications_wanted`]. Called after a config change and whenever shell hosting
    /// starts or stops, so this must stay cheap and quiet when there is nothing new to do.
    pub(super) fn update_notifications(&mut self) {
        let wanted = self.notifications_wanted();
        self.toast
            .set_settings(super::toast_settings(&self.loaded, wanted));
        if wanted {
            self.start_notifications();
        } else {
            self.notify_listener = None;
            self.notify_denied = false;
        }
    }

    /// Starts the listener if it isn't already running. Windows is only asked for permission
    /// once a run, and a refusal is only logged once, so this is safe to call on every retry.
    fn start_notifications(&mut self) {
        if self.notify_listener.is_some() {
            return;
        }
        match notifications::access_status() {
            Some(Access::Unspecified) => {
                if !self.notify_access_requested {
                    self.notify_access_requested = true;
                    // `request_access` blocks until the user answers a prompt, if Windows shows
                    // one, so it must not run on the daemon's own thread.
                    std::thread::spawn(|| {
                        notifications::request_access();
                    });
                }
                return;
            }
            Some(Access::Denied) | None => {
                self.warn_notifications_denied();
                return;
            }
            Some(Access::Allowed) => {}
        }
        let tx = self.notify_tx.clone();
        match notifications::Listener::spawn(move |notification| {
            let _ = tx.send(notification);
        }) {
            Some(listener) => {
                self.notify_listener = Some(listener);
                self.notify_denied = false;
                tracing::info!("notification popups started");
            }
            None => self.warn_notifications_denied(),
        }
    }

    fn warn_notifications_denied(&mut self) {
        if !self.notify_denied {
            self.notify_denied = true;
            tracing::warn!(
                "wanted notification popups, but Tile doesn't have permission to read \
                 notifications (check Settings, Notifications, for Tile)"
            );
        }
    }

    /// Shows `notification` as a popup on the focused monitor, or the first one if nothing is
    /// focused, unless `togglednd` has muted popups or `notifications.filter` mutes this app.
    pub(super) fn show_notification(&mut self, notification: Notification) {
        if self.dnd {
            return;
        }
        let outcome = self.loaded.notification_filters.resolve(&notification.app);
        if outcome.mute {
            return;
        }
        let area = self
            .desktops
            .current_ref()
            .focused_monitor()
            .map(|m| m.work_area)
            .or_else(|| tile_platform::monitors().first().map(|m| m.work_area));
        if let Some(area) = area {
            self.toast.show(notification, area, outcome.duration_ms);
        }
    }

    /// `togglednd`: mutes or unmutes popups, without touching Windows' own notification
    /// settings, so Focus Assist and Action Center behave exactly as the user set them.
    pub(super) fn toggle_dnd(&mut self) {
        self.dnd = !self.dnd;
        tracing::info!(
            "notification popups {}",
            if self.dnd { "muted" } else { "unmuted" }
        );
    }
}
