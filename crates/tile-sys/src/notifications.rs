//! Reading the user's toast notifications through `UserNotificationListener`, independent of
//! being the shell: this is the API behind Focus Assist's priority list, not the Explorer-only
//! path that pops `shellexperiencehost.exe`'s toasts.
//!
//! Two things were checked by hand before this module was written, from an unpackaged,
//! unsigned build, with no app manifest or package identity:
//! - `GetAccessStatus` reports `Allowed` outright, and `GetNotificationsAsync` returns full
//!   notification content. The `userNotificationListener` restricted capability that Microsoft's
//!   docs describe is a Store packaging concept; an unpackaged process already runs with the
//!   signed-in user's full rights, which this read-only API apparently accepts in place of it.
//! - The live `NotificationChanged` event does not: subscribing fails with
//!   `0x80070490` ("Element not found"), packaged app or not, which looks like it needs a
//!   per-package registration this process has none of. So this module polls
//!   `GetNotificationsAsync` instead and works out what is new by id.

use std::{
    collections::HashSet,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use windows::UI::Notifications::{
    KnownNotificationBindings, Management::UserNotificationListener, NotificationKinds,
    UserNotification,
};

use crate::devices::Com;

/// How often the listener thread asks Windows for the current notifications.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Whether Tile may read the user's notifications, from
/// [`UserNotificationListenerAccessStatus`](windows::UI::Notifications::Management::UserNotificationListenerAccessStatus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Granted; [`Listener::spawn`] will work.
    Allowed,
    /// Refused. Only Windows Settings' notification access page can change this back.
    Denied,
    /// Neither granted nor refused yet.
    Unspecified,
}

impl From<windows::UI::Notifications::Management::UserNotificationListenerAccessStatus> for Access {
    fn from(
        status: windows::UI::Notifications::Management::UserNotificationListenerAccessStatus,
    ) -> Self {
        use windows::UI::Notifications::Management::UserNotificationListenerAccessStatus as S;
        if status == S::Allowed {
            Self::Allowed
        } else if status == S::Denied {
            Self::Denied
        } else {
            Self::Unspecified
        }
    }
}

/// One posted toast notification, as `UserNotificationListener` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// The posting app's display name.
    pub app: String,
    /// The first text element, usually the title.
    pub title: String,
    /// The remaining text elements, joined with newlines.
    pub body: String,
}

fn current_listener() -> Option<UserNotificationListener> {
    let _com = Com::init();
    UserNotificationListener::Current().ok()
}

/// Whether Tile currently has permission to read notifications, or `None` if the API itself is
/// unavailable (older Windows).
pub fn access_status() -> Option<Access> {
    current_listener()?.GetAccessStatus().ok().map(Access::from)
}

/// Asks Windows for permission to read notifications. Per Microsoft's docs this only shows UI
/// the first time, while the status is [`Access::Unspecified`]; once allowed or denied it
/// returns immediately with the same answer. Call it from the UI thread.
pub fn request_access() -> Option<Access> {
    current_listener()?
        .RequestAccessAsync()
        .ok()?
        .join()
        .ok()
        .map(Access::from)
}

/// `notification`'s app name and toast text, or `None` if a part of it can't be read.
fn describe(notification: &UserNotification) -> Option<Notification> {
    let app = notification
        .AppInfo()
        .ok()?
        .DisplayInfo()
        .ok()?
        .DisplayName()
        .ok()?
        .to_string();
    let binding = notification
        .Notification()
        .ok()?
        .Visual()
        .ok()?
        .GetBinding(&KnownNotificationBindings::ToastGeneric().ok()?)
        .ok()?;
    let mut lines = binding
        .GetTextElements()
        .ok()?
        .into_iter()
        .filter_map(|element| element.Text().ok().map(|t| t.to_string()));
    let title = lines.next().unwrap_or_default();
    let body = lines.collect::<Vec<_>>().join("\n");
    Some(Notification { app, title, body })
}

/// The ids and parsed content of every notification posted right now, oldest first.
fn poll(listener: &UserNotificationListener) -> Vec<(u32, Notification)> {
    let Ok(notifications) = listener
        .GetNotificationsAsync(NotificationKinds::Toast)
        .and_then(|op| op.join())
    else {
        return Vec::new();
    };
    notifications
        .into_iter()
        .filter_map(|n| {
            let id = n.Id().ok()?;
            describe(&n).map(|parsed| (id, parsed))
        })
        .collect()
}

/// Polls for new notifications on a thread of its own, until dropped.
pub struct Listener {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    /// Starts polling, calling `on_notification` for each notification posted from now on, oldest
    /// first; whatever is already in Action Center when this starts is not replayed. Returns
    /// `None` if Tile doesn't currently have permission ([`access_status`] should be checked, and
    /// [`request_access`] asked for, before calling this).
    pub fn spawn(on_notification: impl Fn(Notification) + Send + 'static) -> Option<Self> {
        let _com = Com::init();
        if access_status() != Some(Access::Allowed) {
            return None;
        }
        let (stop, stopped) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("notifications".into())
            .spawn(move || run(&stopped, &on_notification))
            .ok();
        Some(Self { stop, thread })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(stopped: &mpsc::Receiver<()>, on_notification: &(impl Fn(Notification) + Send + 'static)) {
    let _com = Com::init();
    let Some(listener) = UserNotificationListener::Current().ok() else {
        return;
    };
    // Notifications already sitting in Action Center when Tile starts watching are not new;
    // only pop up the ones that arrive from here on.
    let mut seen: HashSet<u32> = poll(&listener).into_iter().map(|(id, _)| id).collect();
    loop {
        for (id, notification) in poll(&listener) {
            if seen.insert(id) {
                on_notification(notification);
            }
        }
        if stopped.recv_timeout(POLL_INTERVAL).is_ok() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The machine decides whether access is granted, so this only checks the call doesn't
    /// fail outright.
    #[test]
    fn reads_access_status_without_failing() {
        let _ = access_status();
    }

    /// Starting and immediately dropping the listener must not hang or panic, whatever the
    /// current access status is.
    #[test]
    fn starts_and_stops_cleanly() {
        let listener = Listener::spawn(|_| {});
        drop(listener);
    }
}
