//! Turning the window manager and the remapper on and off, reloading and pausing.

use super::{Daemon, fresh_desktops, indicator_settings, launcher_settings};
use crate::{
    logging,
    remap::{self, Remap},
    settings,
};
use tile_config::Loaded;

impl Daemon {
    /// Shows the windows we hid and undoes our borders and title bar changes.
    pub(super) fn release_windows(&mut self) {
        for id in self.desktops.hidden() {
            self.windows.show(id);
        }
        for id in self.desktops.managed() {
            self.windows.set_border(id, None);
        }
        for (id, tracked) in &mut self.tracked {
            if tracked.undecorated.take() == Some(true) {
                self.windows.set_decorated(*id, true);
            }
        }
    }

    /// Turns the window manager on: takes over Win+L, installs the binds and manages the
    /// windows that are open.
    pub(super) fn start_tiling(&mut self) {
        self.tiling = true;
        self.desktops = fresh_desktops(&self.loaded);
        self.update_lock_override();
        self.switch_off_click_to_do();
        self.install_binds();
        if !self.paused {
            self.startup();
        }
        self.update_cursor();
        tracing::info!("tiling started");
    }

    /// Turns the window manager off, giving back every window and Win+L.
    pub(super) fn stop_tiling(&mut self) {
        self.release_windows();
        self.tiling = false;
        self.update_lock_override();
        self.install_binds();
        self.reset_state();
        self.update_cursor();
        self.persist_restore();
        self.publish();
        tracing::info!("tiling stopped");
    }

    /// Forgets every window and monitor, as if no window had been managed yet.
    pub(super) fn reset_state(&mut self) {
        self.desktops = fresh_desktops(&self.loaded);
        self.monitors.clear();
        self.tracked.clear();
        self.ignored.clear();
        self.bordered = None;
        self.heal_at = None;
        self.drag = None;
    }

    /// Starts, updates or stops the key remapper to match `general.remap`.
    pub(super) fn update_remap(&mut self) {
        if !self.loaded.config.general.remap {
            self.stop_remap("remapping stopped");
            return;
        }
        let config = self.loaded.remap.clone();
        if let Some(remap) = &self.remap {
            remap.send(remap::Command::Load(config));
            return;
        }
        match Remap::start(config, self.remap_paused, self.remap_notices.clone()) {
            Ok(remap) => {
                self.remap = Some(remap);
                if !tile_platform::is_elevated() {
                    tracing::warn!(
                        "not running as administrator: keys typed into admin windows aren't \
                         remapped (`tile install` sets Tile up to run elevated)"
                    );
                }
                tracing::info!("remapping started");
            }
            Err(e) => tracing::error!("failed to install the remapping hook: {e}"),
        }
    }

    /// Removes the remapping hook, if it runs, logging `why`.
    pub(super) fn stop_remap(&mut self, why: &str) {
        if let Some(remap) = self.remap.take() {
            remap.stop();
            tracing::info!("{why}");
        }
    }

    pub(super) fn reload(&mut self) {
        match Loaded::load_or_default(&self.config_path) {
            Ok(loaded) => {
                self.tray.set_config_error(None);
                // Our own save of a tray switch lands here too; nothing else changed.
                let mut same = loaded.config.clone();
                same.tray = self.loaded.config.tray.clone();
                let unchanged = same == self.loaded.config;
                let shell = loaded.config.tray.shell;
                self.loaded = loaded;
                // The Shell switch follows the config file, whoever edited it.
                if shell != self.shell_on {
                    self.apply_shell(shell);
                }
                if unchanged {
                    return;
                }
                self.launcher.set_settings(launcher_settings(&self.loaded));
                self.indicator
                    .set_settings(indicator_settings(&self.loaded));
                self.install_binds();
                self.update_remap();
                self.update_cursor();
                self.sync_shell();
                match (self.tiling, self.loaded.config.general.tiling) {
                    (false, true) => self.start_tiling(),
                    (true, false) => self.stop_tiling(),
                    (true, true) => self.reload_tiling(),
                    (false, false) => {}
                }
                tracing::info!("config reloaded");
            }
            Err(e) => {
                tracing::error!("keeping previous config: {e}");
                self.tray.set_config_error(Some(e.summary()));
            }
        }
    }

    /// Applies a reloaded config to the running window manager.
    pub(super) fn reload_tiling(&mut self) {
        self.ignored.clear();
        self.update_lock_override();
        self.switch_off_click_to_do();
        self.install_binds();
        for tracked in self.tracked.values_mut() {
            tracked.decoration_rule = None;
        }
        for id in self.desktops.managed() {
            let rule = self
                .windows
                .info(id)
                .and_then(|info| self.loaded.rules.resolve(&info).decorations);
            if rule.is_some() {
                self.track(id).decoration_rule = rule;
            }
        }
        let config = &self.loaded.config;
        let effects = self
            .desktops
            .set_settings(config.layout_settings(), config.general.layout);
        self.repaint_borders();
        self.apply(effects);
    }

    pub(super) fn toggle_logging(&mut self) {
        logging::set_enabled(!logging::enabled());
        self.tray.set_logging(logging::enabled());
    }

    /// `togglepause`: pauses the window manager and the remapper, or resumes both if both are
    /// paused.
    pub(super) fn toggle_pause(&mut self) {
        let pause = !(self.paused && self.remap_paused);
        self.set_remap_paused(pause);
        self.set_paused(pause);
    }

    pub(super) fn set_remap_paused(&mut self, paused: bool) {
        self.remap_paused = paused;
        self.tray.set_remap_paused(paused);
        settings::update(|s| s.remap = !paused);
        if let Some(remap) = &self.remap {
            remap.send(remap::Command::Pause(paused));
        }
        tracing::info!("remapping {}", if paused { "paused" } else { "resumed" });
    }

    /// Pauses or resumes the window manager (binds, except `togglepause`, included).
    pub(super) fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.hotkeys.set_paused(paused || !self.tiling);
        self.tray.set_tile_paused(paused);
        settings::update(|s| s.tile = !paused);
        self.update_cursor();
        tracing::info!("tiling {}", if paused { "paused" } else { "resumed" });
        if !self.tiling {
            return;
        }
        if paused {
            // Hand every window back as it was: shown, with its title bar and border.
            self.release_windows();
            self.reset_state();
            self.persist_restore();
            self.publish();
        } else {
            self.startup();
        }
    }
}
