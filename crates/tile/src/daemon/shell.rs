//! Shell mode: hosting the notification area and the sign-in shell setting.

use tile_platform::Backdrop;
use tile_shell::notify::{Host, taskbar_exists};

use super::Daemon;
use crate::launch;

impl Daemon {
    /// Removes the black windows, so the desktop shows again, and gives the SmartScreen app
    /// check back, now that Tile no longer hosts the notification area.
    pub(super) fn restore_backdrop(&mut self) {
        self.backdrop = None;
        self.give_back_smartscreen();
    }

    /// Whether Tile acts as the Windows shell: `--shell`, or the tray's Shell switch.
    pub(super) fn shell_wanted(&self) -> bool {
        self.shell_forced || self.shell_on
    }

    /// The tray's Shell item: flips the saved switch, which then applies like a config edit.
    pub(super) fn toggle_shell(&mut self) {
        let on = !self.shell_on;
        self.save_switches(|s| s.shell = on);
        self.apply_shell(on);
    }

    /// Makes the Shell switch `on` or off: Windows starts Tile instead of Explorer at the next
    /// sign-in, Tile hosts the notification area now if no taskbar runs, and the launcher is
    /// always bound.
    pub(super) fn apply_shell(&mut self, on: bool) {
        self.shell_on = on;
        self.register_shell();
        self.tray.set_shell(self.shell_wanted());
        tracing::info!("shell mode {}", if on { "on" } else { "off" });
        self.install_binds();
        self.sync_shell();
    }

    /// Points the sign-in shell at the switch: Tile when it is on, Explorer when it is off.
    pub(super) fn register_shell(&self) {
        let exe = tile_platform::installed_exe();
        let result = if !self.shell_on {
            tile_platform::set_sign_in_shell(None)
        } else if exe.exists() {
            tile_platform::set_sign_in_shell(Some(&exe))
        } else {
            Err(std::io::Error::other(
                "Tile isn't installed (run `tile install`)",
            ))
        };
        if let Err(e) = result {
            tracing::warn!("couldn't change the sign-in shell, so Explorer still starts: {e}");
        }
    }

    /// At startup: hosts the notification area and, when there was no taskbar to begin with,
    /// starts what Explorer would have at sign-in.
    pub(super) fn start_shell(&mut self) {
        let bare = !taskbar_exists();
        self.sync_shell();
        let shell = &self.loaded.config.shell;
        if self.notify_host.is_none() || !bare || !shell.run_startup {
            return;
        }
        if !tile_platform::first_startup_this_session() {
            tracing::info!("the sign-in programs were already started this session");
            return;
        }
        let commands = tile_platform::startup_commands();
        tracing::info!("starting {} sign-in programs", commands.len());
        for command in commands {
            tracing::debug!("startup: {command}");
            launch::exec(&command);
        }
    }

    /// Hosts the notification area while Tile is the shell and no other taskbar runs, and lets
    /// go of it when one appears or Tile stops being the shell.
    pub(super) fn sync_shell(&mut self) {
        if self.notify_host.as_ref().is_some_and(|h| !h.running()) {
            tracing::info!("another taskbar took over the notification area");
            self.notify_host = None;
            self.restore_backdrop();
        }
        match (self.shell_wanted(), self.notify_host.is_some()) {
            (true, false) if taskbar_exists() => {
                if !self.taskbar_noted {
                    self.taskbar_noted = true;
                    tracing::info!("a taskbar is running; the notification area is left to it");
                }
            }
            (true, false) => {
                self.taskbar_noted = false;
                self.notify_host = Host::start(self.registry.clone());
                if self.notify_host.is_some() {
                    tracing::info!("hosting the notification area");
                    if self.backdrop.is_none() {
                        self.backdrop = Backdrop::black();
                    }
                    self.take_smartscreen();
                } else {
                    tracing::warn!("couldn't start the notification area");
                }
            }
            (false, true) => {
                if let Some(host) = self.notify_host.take() {
                    host.stop();
                }
                self.restore_backdrop();
                tracing::info!("stopped hosting the notification area");
            }
            _ => {}
        }
        // Notifications' own default follows whether we ended up hosting just now.
        self.update_notifications();
    }
}
