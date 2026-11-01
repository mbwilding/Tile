//! Which windows are managed, on which monitor and native desktop.

use tile_core::{Effect, ManageOptions, Rect, WindowId};
use tile_platform::{
    Discovered, cursor_pos, desktops as vd,
    desktops::{DesktopChange, DesktopId},
    foreground, monitors,
};

use super::{Daemon, bump};

impl Daemon {
    pub(super) fn refresh_monitors(&mut self) {
        let current = monitors();
        if current.is_empty() || current == self.monitors {
            return;
        }
        tracing::info!("monitors: {current:?}");
        self.monitors.clone_from(&current);
        let effects = self.desktops.set_monitors(current);
        self.apply(effects);
    }

    /// The monitor a window should join: under the mouse for new windows, where it already
    /// sits for existing ones. `None` falls back to the focused monitor.
    pub(super) fn monitor_for(&self, rect: Rect, opened: bool) -> Option<usize> {
        let (x, y) = if opened { cursor_pos() } else { rect.center() };
        self.monitors
            .iter()
            .position(|m| m.rect.contains_point(x, y))
    }

    /// Starts managing a discovered window on the native desktop it lives on. `opened` is
    /// false for windows that already existed (startup / resume), which must not steal focus.
    pub(super) fn manage(&mut self, found: Discovered, opened: bool) -> Vec<Effect> {
        let outcome = self.loaded.rules.resolve(&found.info);
        if outcome.ignore {
            self.windows.forget(found.id);
            self.ignored.insert(found.id);
            return Vec::new();
        }
        tracing::debug!("managing {:?} {:?}", found.id, found.info);
        if let Some(decorations) = outcome.decorations {
            self.decoration_rules.insert(found.id, decorations);
        }
        if self.loaded.config.general.borders {
            self.windows
                .set_border(found.id, Some(self.loaded.config.border_inactive()));
        }
        let desktop = vd::desktop_of(found.id).unwrap_or_else(|| self.desktops.current_id());
        let options = ManageOptions {
            floating: outcome.float.unwrap_or(found.floating_hint),
            workspace: outcome.workspace,
            monitor: outcome
                .monitor
                .or_else(|| self.monitor_for(found.rect, opened)),
            rect: found.rect,
            focus: opened && self.loaded.config.general.focus_new_windows,
        };
        self.desktops.open(desktop, found.id, found.info, options)
    }

    pub(super) fn try_manage(&mut self, id: WindowId) {
        if self.ignored.contains(&id) {
            return;
        }
        if let Some(found) = self.windows.discover(id) {
            // Windows shown on every desktop float above the tiling.
            if vd::is_pinned(id) {
                self.windows.forget(found.id);
                return;
            }
            let effects = self.manage(found, true);
            self.apply(effects);
        }
    }

    pub(super) fn unmanage(&mut self, id: WindowId) {
        tracing::debug!("unmanaging {id:?}");
        let alive = self.windows.is_alive(id);
        if self.undecorated.remove(&id) == Some(true) && alive {
            self.windows.set_decorated(id, true);
        }
        if self.desktops.is_hidden(id) && alive {
            self.windows.show(id);
        }
        self.decoration_rules.remove(&id);
        self.forced.remove(&id);
        let effects = self.desktops.close(id);
        self.windows.forget(id);
        self.placed.remove(&id);
        self.tweens.remove(&id);
        self.moving.remove(&id);
        self.pending_hide.remove(&id);
        self.pending_show.remove(&id);
        if self.bordered == Some(id) {
            self.bordered = None;
        }
        self.apply(effects);
    }

    /// Re-homes a window after it was moved to another native desktop or (un)pinned.
    pub(super) fn check_desktop(&mut self, id: WindowId) {
        if vd::is_pinned(id) {
            if self.desktops.is_managed(id) {
                self.unmanage(id);
            }
            return;
        }
        let Some(desktop) = vd::desktop_of(id) else {
            return;
        };
        match self.desktops.owner(id) {
            None => self.try_manage(id),
            Some(owner) if owner != desktop => self.migrate(id, desktop),
            Some(_) => {}
        }
    }

    pub(super) fn migrate(&mut self, id: WindowId, desktop: DesktopId) {
        tracing::debug!("{id:?} moved to desktop {desktop:032x}");
        let floating = self.desktops.is_floating(id);
        if self.desktops.is_hidden(id) {
            bump(&mut self.pending_show, id);
            self.windows.show(id);
        }
        let effects = self.desktops.close(id);
        self.apply(effects);
        self.placed.remove(&id);
        self.tweens.remove(&id);
        let info = self.windows.info(id).unwrap_or_default();
        let rect = self.windows.frame(id).unwrap_or_default();
        let options = ManageOptions {
            floating,
            rect,
            ..ManageOptions::default()
        };
        let effects = self.desktops.open(desktop, id, info, options);
        self.apply(effects);
    }

    pub(super) fn switch_desktop(&mut self, desktop: DesktopId) {
        if desktop == self.desktops.current_id() {
            return;
        }
        tracing::debug!("switched to desktop {desktop:032x}");
        let effects = self.desktops.switch_to(desktop);
        self.apply(effects);
        // Windows on this desktop we haven't seen yet (e.g. created while it was hidden).
        self.discover_existing();
        if let Some(id) = foreground() {
            self.on_foreground(id);
        }
    }

    pub(super) fn on_desktop(&mut self, change: DesktopChange) {
        if self.paused || !self.tiling {
            return;
        }
        match change {
            DesktopChange::Switched(desktop) => self.switch_desktop(desktop),
            DesktopChange::Window(id) => self.check_desktop(id),
            DesktopChange::Changed => {
                // A removed desktop's windows move to its neighbour.
                for id in self.desktops.managed() {
                    self.check_desktop(id);
                }
            }
        }
    }
}
