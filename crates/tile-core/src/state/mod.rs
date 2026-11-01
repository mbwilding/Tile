mod commands;
mod focus;
mod snapshot;
#[cfg(test)]
mod tests;
mod windows;
mod workspace;

use std::collections::{BTreeMap, HashMap, HashSet};

pub use snapshot::{MonitorSnapshot, WindowSnapshot, WorkspaceSnapshot};
use workspace::Workspace;

use serde::{Deserialize, Serialize};

use crate::{
    MonitorId, WindowId,
    geometry::Rect,
    layout::{LayoutKind, LayoutSettings, Tile},
};

/// Descriptive information about a window, used for rules and IPC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// The title bar text.
    pub title: String,
    /// The window class.
    pub class: String,
    /// The program's file name, e.g. `code.exe`.
    pub exe: String,
}

/// How a newly discovered window should be managed (resolved from window rules).
#[derive(Debug, Clone, Default)]
pub struct ManageOptions {
    /// Float instead of tiling.
    pub floating: bool,
    /// Open on this workspace instead of the focused one.
    pub workspace: Option<u8>,
    /// Open on this monitor (by index) instead of the focused one.
    pub monitor: Option<usize>,
    /// The window's current visible frame, used as its floating geometry.
    pub rect: Rect,
    /// Whether the new window should receive focus.
    pub focus: bool,
}

/// A monitor as the platform reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Its handle.
    pub id: MonitorId,
    /// The whole monitor.
    pub rect: Rect,
    /// The monitor less the taskbar and other app bars.
    pub work_area: Rect,
}

/// A side effect for the platform layer to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Move and resize a window so its visible frame matches `rect`.
    Place {
        /// The window.
        id: WindowId,
        /// Where its visible frame goes.
        rect: Rect,
    },
    /// Show a window that was hidden.
    Show(WindowId),
    /// Hide a window, as its workspace isn't shown or it scrolled off screen.
    Hide(WindowId),
    /// Give a window the keyboard focus.
    Focus(WindowId),
    /// Nothing on the focused monitor can take focus: put it on the desktop, so Windows
    /// doesn't pick a window on another monitor.
    Defocus,
    /// Close a window, like Alt+F4.
    Close(WindowId),
    /// Run a command line.
    Exec(String),
}

#[derive(Debug, Clone)]
struct Monitor {
    info: MonitorInfo,
    workspace: u8,
}

#[derive(Debug, Clone)]
struct Managed {
    info: WindowInfo,
    workspace: u8,
    floating: bool,
    float_rect: Rect,
}

/// The window manager's model of the desktop.
#[derive(Debug, Clone)]
pub struct State {
    monitors: Vec<Monitor>,
    /// Keyed by an internal number, not the label: see `labels`.
    workspaces: BTreeMap<u8, Workspace>,
    /// The workspace a monitor has for a label.
    labels: HashMap<(MonitorId, u8), u8>,
    windows: HashMap<WindowId, Managed>,
    hidden: HashSet<WindowId>,
    focused: Option<WindowId>,
    focused_monitor: usize,
    settings: LayoutSettings,
    default_layout: LayoutKind,
}

impl State {
    /// No monitors or windows yet. Workspaces start with `default_layout`.
    pub fn new(settings: LayoutSettings, default_layout: LayoutKind) -> Self {
        Self {
            monitors: Vec::new(),
            workspaces: BTreeMap::new(),
            labels: HashMap::new(),
            windows: HashMap::new(),
            hidden: HashSet::new(),
            focused: None,
            focused_monitor: 0,
            settings,
            default_layout,
        }
    }

    /// The window with focus, if Tile manages it.
    pub const fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    /// Whether Tile manages `id`.
    pub fn is_managed(&self, id: WindowId) -> bool {
        self.windows.contains_key(&id)
    }

    /// Every managed window.
    pub fn managed(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.keys().copied()
    }

    /// Windows the manager has hidden (inactive workspaces, scrolled-off columns).
    pub fn hidden(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.hidden.iter().copied()
    }

    /// Whether Tile hid `id`.
    pub fn is_hidden(&self, id: WindowId) -> bool {
        self.hidden.contains(&id)
    }

    /// Whether `id` floats.
    pub fn is_floating(&self, id: WindowId) -> bool {
        self.windows.get(&id).is_some_and(|m| m.floating)
    }

    /// Updates the title, class and program of `id`.
    pub fn set_info(&mut self, id: WindowId, info: WindowInfo) {
        if let Some(m) = self.windows.get_mut(&id) {
            m.info = info;
        }
    }

    /// How many monitors there are.
    pub fn monitor_count(&self) -> usize {
        self.monitors.len()
    }

    /// The monitor with focus.
    pub fn focused_monitor(&self) -> Option<&MonitorInfo> {
        self.monitors.get(self.focused_monitor).map(|m| &m.info)
    }

    /// Whether window `id` is on the focused monitor's visible workspace.
    pub fn on_focused_monitor(&self, id: WindowId) -> bool {
        self.windows
            .get(&id)
            .and_then(|m| self.showing(m.workspace))
            .is_some_and(|i| i == self.focused_monitor)
    }

    /// The number of the focused monitor's workspace.
    pub fn focused_workspace(&self) -> Option<u8> {
        let key = self.focused_key()?;
        self.workspaces.get(&key).map(|w| w.label)
    }

    fn focused_key(&self) -> Option<u8> {
        self.monitors.get(self.focused_monitor).map(|m| m.workspace)
    }

    /// The layout of the focused monitor's workspace.
    pub fn active_layout(&self) -> Option<LayoutKind> {
        let ws = self.focused_key()?;
        self.workspaces.get(&ws).map(|w| w.layout.kind())
    }

    /// Applies new settings from a config reload.
    pub fn set_settings(
        &mut self,
        settings: LayoutSettings,
        default_layout: LayoutKind,
    ) -> Vec<Effect> {
        self.settings = settings;
        self.default_layout = default_layout;
        self.sync()
    }

    fn monitor_index(&self, id: MonitorId) -> Option<usize> {
        self.monitors.iter().position(|m| m.info.id == id)
    }

    fn showing(&self, ws: u8) -> Option<usize> {
        self.monitors.iter().position(|m| m.workspace == ws)
    }

    fn area_of(&self, ws: u8) -> Rect {
        let index = self
            .workspaces
            .get(&ws)
            .and_then(|w| self.monitor_index(w.monitor))
            .unwrap_or(self.focused_monitor);
        self.monitors
            .get(index)
            .map(|m| m.info.work_area)
            .unwrap_or_default()
    }

    /// The layout a workspace starts with on `area`: even on portrait, else the default.
    fn layout_for(&self, area: Rect) -> LayoutKind {
        if area.h > area.w {
            LayoutKind::Even
        } else {
            self.default_layout
        }
    }

    /// A monitor was rotated: portrait switches to even, and back from it on landscape.
    fn adopt_orientation(&mut self, n: u8, area: Rect) {
        let want = self.layout_for(area);
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return;
        };
        let kind = ws.layout.kind();
        if want != kind && (want == LayoutKind::Even || kind == LayoutKind::Even) {
            ws.layout = ws.layout.convert(want, area, &self.settings);
        }
    }

    /// The workspace `monitor` has for `label`, created on first use.
    fn workspace_for(&mut self, label: u8, monitor: MonitorId) -> u8 {
        if let Some(key) = self.labels.get(&(monitor, label)) {
            return *key;
        }
        let key = (1..=u8::MAX)
            .find(|k| !self.workspaces.contains_key(k))
            .unwrap_or(u8::MAX);
        let area = self
            .monitor_index(monitor)
            .map_or_else(Rect::default, |i| self.monitors[i].info.work_area);
        let ws = Workspace::new(monitor, label, self.layout_for(area), &self.settings);
        self.workspaces.insert(key, ws);
        self.labels.insert((monitor, label), key);
        key
    }

    /// Moves the windows of workspaces whose monitor is gone onto the first monitor's workspace
    /// with the same number.
    fn rehome_orphans(&mut self, present: &HashSet<MonitorId>) {
        let fallback = self.monitors[0].info.id;
        let orphans: Vec<u8> = self
            .workspaces
            .iter()
            .filter(|(_, w)| !present.contains(&w.monitor))
            .map(|(key, _)| *key)
            .collect();
        for key in orphans {
            let Some(old) = self.workspaces.remove(&key) else {
                continue;
            };
            self.labels.retain(|_, k| *k != key);
            let dest = self.workspace_for(old.label, fallback);
            for id in old
                .layout
                .windows()
                .into_iter()
                .chain(old.floating.iter().copied())
            {
                if self.is_floating(id) {
                    let rect = self.windows[&id].float_rect;
                    let centered = self.centered(rect, dest);
                    if let Some(m) = self.windows.get_mut(&id) {
                        m.float_rect = centered;
                    }
                }
                self.attach(id, dest);
            }
            for id in old.minimized {
                if let Some(m) = self.windows.get_mut(&id) {
                    m.workspace = dest;
                }
                if let Some(ws) = self.workspaces.get_mut(&dest) {
                    ws.minimized.push(id);
                }
            }
        }
    }

    fn focused_monitor_id(&self) -> Option<MonitorId> {
        self.monitors.get(self.focused_monitor).map(|m| m.info.id)
    }

    /// Replaces the monitor set. Workspaces stay with their monitor; those of a monitor that
    /// went away are merged into the first monitor's.
    pub fn set_monitors(&mut self, infos: Vec<MonitorInfo>) -> Vec<Effect> {
        if infos.is_empty() {
            return Vec::new();
        }
        let old = std::mem::take(&mut self.monitors);
        let present: HashSet<MonitorId> = infos.iter().map(|i| i.id).collect();
        for info in infos {
            let before = old.iter().find(|m| m.info.id == info.id);
            let rotated = before.is_some_and(|m| {
                (m.info.work_area.h > m.info.work_area.w) != (info.work_area.h > info.work_area.w)
            });
            let kept = before.map(|m| m.workspace);
            let (id, area) = (info.id, info.work_area);
            self.monitors.push(Monitor {
                info,
                workspace: kept.unwrap_or(0),
            });
            let ws = kept.unwrap_or_else(|| self.workspace_for(1, id));
            if let Some(m) = self.monitors.last_mut() {
                m.workspace = ws;
            }
            if rotated {
                self.adopt_orientation(ws, area);
            }
        }
        self.rehome_orphans(&present);
        self.focused_monitor = self.focused_monitor.min(self.monitors.len() - 1);
        self.sync()
    }

    /// Recomputes every window's placement and visibility.
    pub fn sync(&mut self) -> Vec<Effect> {
        let Self {
            monitors,
            workspaces,
            windows,
            hidden,
            focused,
            settings,
            ..
        } = self;
        let mut hides = Vec::new();
        let mut places = Vec::new();
        let mut shows = Vec::new();
        for (n, ws) in workspaces.iter_mut() {
            let Some(monitor) = monitors.iter().find(|m| m.workspace == *n) else {
                for id in ws.all_windows() {
                    if hidden.insert(id) {
                        hides.push(Effect::Hide(id));
                    }
                }
                continue;
            };
            let focus = focused
                .filter(|f| windows.get(f).is_some_and(|m| m.workspace == *n))
                .or_else(|| ws.history.last().copied());
            let mut tiles = ws.layout.arrange(monitor.info.work_area, focus, settings);
            tiles.extend(ws.floating.iter().filter_map(|id| {
                windows.get(id).map(|m| Tile {
                    id: *id,
                    rect: m.float_rect,
                    visible: true,
                })
            }));
            for tile in tiles {
                if tile.visible {
                    let rect = if ws.fullscreen == Some(tile.id) {
                        monitor.info.rect
                    } else {
                        tile.rect
                    };
                    places.push(Effect::Place { id: tile.id, rect });
                    if hidden.remove(&tile.id) {
                        shows.push(Effect::Show(tile.id));
                    }
                } else if hidden.insert(tile.id) {
                    hides.push(Effect::Hide(tile.id));
                }
            }
            for id in &ws.minimized {
                if hidden.remove(id) {
                    shows.push(Effect::Show(*id));
                }
            }
        }
        hides.extend(places);
        hides.extend(shows);
        hides
    }
}
