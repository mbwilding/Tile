use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    Command, MonitorId, WindowId,
    geometry::{Direction, Rect, nearest_in_direction, scale},
    layout::{LayoutKind, LayoutSettings, LayoutState, Tile},
};

/// Descriptive information about a window, used for rules and IPC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub title: String,
    pub class: String,
    pub exe: String,
}

/// How a newly discovered window should be managed (resolved from window rules).
#[derive(Debug, Clone, Default)]
pub struct ManageOptions {
    pub floating: bool,
    pub workspace: Option<u8>,
    pub monitor: Option<usize>,
    /// The window's current visible frame, used as its floating geometry.
    pub rect: Rect,
    /// Whether the new window should receive focus.
    pub focus: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub rect: Rect,
    pub work_area: Rect,
}

/// A side effect for the platform layer to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Move and resize a window so its visible frame matches `rect`.
    Place {
        id: WindowId,
        rect: Rect,
    },
    Show(WindowId),
    Hide(WindowId),
    Focus(WindowId),
    /// Nothing on the focused monitor can take focus: put it on the desktop, so Windows
    /// doesn't pick a window on another monitor.
    Defocus,
    Close(WindowId),
    Exec(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSnapshot {
    pub id: isize,
    pub title: String,
    pub class: String,
    pub exe: String,
    pub workspace: u8,
    pub floating: bool,
    pub fullscreen: bool,
    pub focused: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub id: u8,
    pub monitor: Option<usize>,
    pub visible: bool,
    pub focused: bool,
    pub layout: LayoutKind,
    pub windows: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorSnapshot {
    pub index: usize,
    pub rect: Rect,
    pub work_area: Rect,
    pub workspace: u8,
    pub focused: bool,
}

#[derive(Debug, Clone)]
struct Monitor {
    info: MonitorInfo,
    workspace: u8,
}

#[derive(Debug, Clone)]
struct Workspace {
    monitor: MonitorId,
    /// The number the user knows it by; several monitors can each have a workspace 1.
    label: u8,
    layout: LayoutState,
    floating: Vec<WindowId>,
    minimized: Vec<WindowId>,
    fullscreen: Option<WindowId>,
    /// Focus history, most recent last.
    history: Vec<WindowId>,
}

impl Workspace {
    fn new(monitor: MonitorId, label: u8, kind: LayoutKind, s: &LayoutSettings) -> Self {
        Self {
            monitor,
            label,
            layout: LayoutState::new(kind, s),
            floating: Vec::new(),
            minimized: Vec::new(),
            fullscreen: None,
            history: Vec::new(),
        }
    }

    fn all_windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.layout
            .windows()
            .into_iter()
            .chain(self.floating.iter().copied())
            .chain(self.minimized.iter().copied())
    }

    fn touch(&mut self, id: WindowId) {
        self.history.retain(|w| *w != id);
        self.history.push(id);
        // Another window got focus (new window, alt-tab, restore): it must be visible.
        if self.fullscreen.is_some_and(|f| f != id) {
            self.fullscreen = None;
        }
    }

    fn forget(&mut self, id: WindowId) {
        self.layout.remove(id);
        self.floating.retain(|w| *w != id);
        self.minimized.retain(|w| *w != id);
        self.history.retain(|w| *w != id);
        if self.fullscreen == Some(id) {
            self.fullscreen = None;
        }
    }

    fn focus_candidate(&self) -> Option<WindowId> {
        self.history
            .iter()
            .rev()
            .copied()
            .find(|w| !self.minimized.contains(w))
            .or_else(|| self.layout.windows().first().copied())
            .or_else(|| self.floating.first().copied())
    }

    fn len(&self) -> usize {
        self.layout.windows().len() + self.floating.len() + self.minimized.len()
    }
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

    pub const fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    pub fn is_managed(&self, id: WindowId) -> bool {
        self.windows.contains_key(&id)
    }

    pub fn managed(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.keys().copied()
    }

    /// Windows the manager has hidden (inactive workspaces, scrolled-off columns).
    pub fn hidden(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.hidden.iter().copied()
    }

    pub fn is_hidden(&self, id: WindowId) -> bool {
        self.hidden.contains(&id)
    }

    pub fn is_floating(&self, id: WindowId) -> bool {
        self.windows.get(&id).is_some_and(|m| m.floating)
    }

    pub fn set_info(&mut self, id: WindowId, info: WindowInfo) {
        if let Some(m) = self.windows.get_mut(&id) {
            m.info = info;
        }
    }

    pub fn monitor_count(&self) -> usize {
        self.monitors.len()
    }

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

    fn focus_window(&mut self, id: WindowId) -> Vec<Effect> {
        let Some(n) = self.windows.get(&id).map(|m| m.workspace) else {
            return Vec::new();
        };
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.touch(id);
        }
        if self.showing(n).is_none() {
            return self.focus_workspace(n);
        }
        if let Some(i) = self.showing(n) {
            self.focused_monitor = i;
        }
        self.focused = Some(id);
        let mut effects = self.sync();
        effects.push(Effect::Focus(id));
        effects
    }

    fn focus_workspace(&mut self, n: u8) -> Vec<Effect> {
        self.focus_workspace_with(n, None)
    }

    fn focus_workspace_with(&mut self, n: u8, preferred: Option<WindowId>) -> Vec<Effect> {
        let Some(owner) = self.workspaces.get(&n).map(|w| w.monitor) else {
            return Vec::new();
        };
        let index = self.monitor_index(owner).unwrap_or(self.focused_monitor);
        self.monitors[index].workspace = n;
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.monitor = self.monitors[index].info.id;
        }
        self.focused_monitor = index;
        let target = preferred.or_else(|| self.workspaces[&n].focus_candidate());
        if let (Some(id), Some(ws)) = (target, self.workspaces.get_mut(&n)) {
            ws.touch(id);
        }
        self.focused = target;
        let mut effects = self.sync();
        effects.push(target.map_or(Effect::Defocus, Effect::Focus));
        effects
    }

    fn focus_monitor(&mut self, dir: Direction) -> Vec<Effect> {
        let Some(current) = self.monitors.get(self.focused_monitor) else {
            return Vec::new();
        };
        let from = current.info.rect;
        let others = self
            .monitors
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.focused_monitor)
            .map(|(i, m)| (i, m.info.rect));
        let Some(index) = nearest_in_direction(&from, dir, others) else {
            return Vec::new();
        };
        let n = self.monitors[index].workspace;
        let from = self.active_rect().unwrap_or(from);
        let preferred = self.nearest_window(n, from, dir);
        self.focus_workspace_with(n, preferred)
    }

    /// The on-screen frame of the focused window on the focused workspace.
    fn active_rect(&mut self) -> Option<Rect> {
        let (id, n) = self.active().filter(|(_, n)| self.showing(*n).is_some())?;
        if let Some(m) = self.windows.get(&id).filter(|m| m.floating) {
            return Some(m.float_rect);
        }
        let area = self.area_of(n);
        let settings = self.settings.clone();
        let ws = self.workspaces.get_mut(&n)?;
        if ws.fullscreen == Some(id) {
            return None;
        }
        let tiles = ws.layout.arrange(area, Some(id), &settings);
        tiles.iter().find(|t| t.id == id).map(|t| t.rect)
    }

    /// The visible window on workspace `n` closest to `from` when entering from `dir`.
    fn nearest_window(&mut self, n: u8, from: Rect, dir: Direction) -> Option<WindowId> {
        let area = self.area_of(n);
        let settings = self.settings.clone();
        let focus = self.workspaces.get(&n)?.focus_candidate();
        let ws = self.workspaces.get_mut(&n)?;
        if let Some(full) = ws.fullscreen {
            return Some(full);
        }
        let mut tiles = ws.layout.arrange(area, focus, &settings);
        tiles.extend(ws.floating.iter().filter_map(|id| {
            self.windows.get(id).map(|m| Tile {
                id: *id,
                rect: m.float_rect,
                visible: true,
            })
        }));
        // A thin probe on the edge we enter through, aligned with the window we left.
        let probe = match dir {
            Direction::Right => Rect::new(area.x - 1, from.y, 1, from.h),
            Direction::Left => Rect::new(area.right(), from.y, 1, from.h),
            Direction::Down => Rect::new(from.x, area.y - 1, from.w, 1),
            Direction::Up => Rect::new(from.x, area.bottom(), from.w, 1),
        };
        nearest_in_direction(
            &probe,
            dir,
            tiles.iter().filter(|t| t.visible).map(|t| (t.id, t.rect)),
        )
    }

    fn monitor_in_direction(&self, from: usize, dir: Direction) -> Option<usize> {
        let rect = self.monitors.get(from)?.info.rect;
        nearest_in_direction(
            &rect,
            dir,
            self.monitors
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != from)
                .map(|(i, m)| (i, m.info.rect)),
        )
    }

    /// Removes a window from its workspace structures, returning the workspace number.
    fn detach(&mut self, id: WindowId) -> Option<u8> {
        let n = self.windows.get(&id)?.workspace;
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.forget(id);
        }
        Some(n)
    }

    /// Adds a managed window to workspace `n`, next to that workspace's focused window.
    fn attach(&mut self, id: WindowId, n: u8) {
        let area = self.area_of(n);
        let Some(m) = self.windows.get_mut(&id) else {
            return;
        };
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return;
        };
        m.workspace = n;
        if m.floating {
            ws.floating.push(id);
        } else {
            let near = ws.history.last().copied();
            ws.layout.insert(id, near, area, &self.settings);
        }
        ws.touch(id);
    }

    fn centered(&self, size: Rect, n: u8) -> Rect {
        let area = self.area_of(n);
        let w = size.w.clamp(100, area.w.max(100));
        let h = size.h.clamp(100, area.h.max(100));
        Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h)
    }

    /// Starts managing a window. Returns no effects if it was already managed.
    pub fn window_opened(
        &mut self,
        id: WindowId,
        info: WindowInfo,
        opts: ManageOptions,
    ) -> Vec<Effect> {
        if self.windows.contains_key(&id) || self.monitors.is_empty() {
            return Vec::new();
        }
        let monitor = opts
            .monitor
            .filter(|i| *i < self.monitors.len())
            .unwrap_or(self.focused_monitor);
        let monitor_id = self.monitors[monitor].info.id;
        let n = match opts.workspace {
            Some(label) => self.workspace_for(label, monitor_id),
            None => self.monitors[monitor].workspace,
        };
        self.windows.insert(
            id,
            Managed {
                info,
                workspace: n,
                floating: opts.floating,
                float_rect: opts.rect,
            },
        );
        self.attach(id, n);
        if opts.focus && self.showing(n).is_some() {
            return self.focus_window(id);
        }
        self.sync()
    }

    /// Stops managing a window (destroyed, hidden by its app, or became unmanageable).
    pub fn window_closed(&mut self, id: WindowId) -> Vec<Effect> {
        let Some(n) = self.detach(id) else {
            return Vec::new();
        };
        self.windows.remove(&id);
        self.hidden.remove(&id);
        if self.focused == Some(id) {
            self.focused = None;
            if self.showing(n).is_some() {
                if let Some(next) = self.workspaces.get(&n).and_then(Workspace::focus_candidate) {
                    return self.focus_window(next);
                }
                let mut effects = self.sync();
                effects.push(Effect::Defocus);
                return effects;
            }
        }
        self.sync()
    }

    /// The OS reported a new foreground window.
    pub fn window_focused(&mut self, id: WindowId) -> Vec<Effect> {
        let Some(n) = self.windows.get(&id).map(|m| m.workspace) else {
            self.focused = None;
            return Vec::new();
        };
        if self.focused == Some(id) && self.showing(n).is_some() {
            return Vec::new();
        }
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.touch(id);
        }
        if self.showing(n).is_none() {
            return self.focus_workspace(n);
        }
        self.focused = Some(id);
        if let Some(i) = self.showing(n) {
            self.focused_monitor = i;
        }
        self.sync()
    }

    pub fn window_minimized(&mut self, id: WindowId) -> Vec<Effect> {
        let Some(n) = self.detach(id) else {
            return Vec::new();
        };
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.minimized.push(id);
        }
        let was_focused = self.focused == Some(id);
        if was_focused {
            self.focused = None;
        }
        let mut effects = self.sync();
        if was_focused
            && self.showing(n).is_some()
            && self.workspaces[&n].focus_candidate().is_none()
        {
            effects.push(Effect::Defocus);
        }
        effects
    }

    pub fn window_restored(&mut self, id: WindowId) -> Vec<Effect> {
        let Some(n) = self.windows.get(&id).map(|m| m.workspace) else {
            return Vec::new();
        };
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return Vec::new();
        };
        if !ws.minimized.contains(&id) {
            return Vec::new();
        }
        ws.minimized.retain(|w| *w != id);
        self.attach(id, n);
        self.focus_window(id)
    }

    /// The user finished dragging or resizing a window with the mouse.
    pub fn window_moved(&mut self, id: WindowId, rect: Rect, cursor: (i32, i32)) -> Vec<Effect> {
        let Some(m) = self.windows.get(&id) else {
            return Vec::new();
        };
        let n = m.workspace;
        let floating = m.floating;
        let target_monitor = self
            .monitors
            .iter()
            .position(|m| m.info.rect.contains_point(cursor.0, cursor.1));
        let current_monitor = self.showing(n);

        if floating && let Some(m) = self.windows.get_mut(&id) {
            m.float_rect = rect;
        }
        if let (Some(target), Some(current)) = (target_monitor, current_monitor)
            && target != current
        {
            let dest = self.monitors[target].workspace;
            self.detach(id);
            self.attach(id, dest);
            return self.focus_window(id);
        }
        if floating {
            return self.sync();
        }

        let area = self.area_of(n);
        let settings = self.settings.clone();
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return Vec::new();
        };
        let tiles = ws.layout.arrange(area, Some(id), &settings);
        let own = tiles.iter().find(|t| t.id == id);
        // An edge dragged past its neighbour ends with the pointer over it, so a change of size
        // is a resize and never a swap; only a window moved at its own size swaps places.
        let resized =
            own.is_some_and(|t| (rect.w - t.rect.w).abs() > 2 || (rect.h - t.rect.h).abs() > 2);
        let under = tiles
            .iter()
            .find(|t| t.visible && t.id != id && t.rect.contains_point(cursor.0, cursor.1));
        if resized && let Some(own) = own {
            ws.layout.resize(
                id,
                rect.w - own.rect.w,
                rect.h - own.rect.h,
                area,
                &settings,
            );
        } else if let Some(other) = under {
            ws.layout.swap(id, other.id);
        }
        self.sync()
    }

    /// Resizes the split of a tiled window to the size it has been dragged to. `false` if it
    /// isn't a tiled window, or is already that size.
    fn resize_to(&mut self, id: WindowId, rect: Rect) -> bool {
        let Some(m) = self.windows.get(&id) else {
            return false;
        };
        if m.floating {
            return false;
        }
        let n = m.workspace;
        let area = self.area_of(n);
        let settings = self.settings.clone();
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return false;
        };
        let Some(own) = ws
            .layout
            .arrange(area, Some(id), &settings)
            .into_iter()
            .find(|t| t.id == id)
        else {
            return false;
        };
        let (dx, dy) = (rect.w - own.rect.w, rect.h - own.rect.h);
        if dx.abs() <= 2 && dy.abs() <= 2 {
            return false;
        }
        ws.layout.resize(id, dx, dy, area, &settings);
        true
    }

    /// An edge of a tiled window is being dragged: the others fit around the size it has now.
    /// Empty while its size is unchanged, as in a plain move.
    pub fn window_resizing(&mut self, id: WindowId, rect: Rect) -> Vec<Effect> {
        if self.resize_to(id, rect) {
            self.sync()
        } else {
            Vec::new()
        }
    }

    /// The edge drag of a tiled window ended: everything settles into its tiles.
    pub fn window_resized(&mut self, id: WindowId, rect: Rect) -> Vec<Effect> {
        self.resize_to(id, rect);
        self.sync()
    }

    /// Executes a dispatcher against the focused window / workspace.
    pub fn apply(&mut self, command: &Command) -> Vec<Effect> {
        if self.monitors.is_empty() {
            return Vec::new();
        }
        match command {
            Command::Exec(cmd) => vec![Effect::Exec(cmd.clone())],
            Command::KillActive => self.focused.map(Effect::Close).into_iter().collect(),
            Command::ToggleFloating => self.toggle_floating(),
            Command::Fullscreen => self.toggle_fullscreen(),
            Command::MoveFocus(dir) => self.move_focus(*dir),
            Command::MoveWindow(dir) => self.move_window(*dir),
            Command::ResizeActive(dx, dy) => self.resize_active(*dx, *dy),
            Command::CycleNext => self.cycle(true),
            Command::CyclePrev => self.cycle(false),
            Command::Workspace(n) => self.switch_workspace(*n),
            Command::MoveToWorkspace(n) => self.move_to_workspace(*n, true),
            Command::MoveToWorkspaceSilent(n) => self.move_to_workspace(*n, false),
            Command::FocusMonitor(dir) => self.focus_monitor(*dir),
            Command::Layout(kind) => self.set_layout(*kind),
            Command::CycleLayout => {
                let next = self.active_layout().unwrap_or(self.default_layout).next();
                self.set_layout(next)
            }
            Command::SwapWithMaster
            | Command::ToggleSplit
            | Command::ScrollerProportion(_)
            | Command::CycleProportion
            | Command::ConsumeWindow(_)
            | Command::ExpelWindow
            | Command::CenterColumn => self.layout_specific(command),
            // Handled by the daemon.
            Command::FocusOrExec(_)
            | Command::Desktop(_)
            | Command::MoveToDesktop(_)
            | Command::Pin
            | Command::Lock
            | Command::Launcher
            | Command::Reload
            | Command::TogglePause
            | Command::ToggleTile
            | Command::ToggleRemap
            | Command::ToggleShell
            | Command::ToggleLog
            | Command::Exit => Vec::new(),
        }
    }

    /// The focused window if it lives on the focused workspace.
    fn active(&self) -> Option<(WindowId, u8)> {
        let id = self.focused?;
        let n = self.windows.get(&id)?.workspace;
        Some((id, n))
    }

    fn toggle_floating(&mut self) -> Vec<Effect> {
        let Some((id, n)) = self.active() else {
            return Vec::new();
        };
        let float_rect = self.windows[&id].float_rect;
        let centered = self.centered(float_rect, n);
        let area = self.area_of(n);
        let Some(m) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return Vec::new();
        };
        if m.floating {
            m.floating = false;
            ws.floating.retain(|w| *w != id);
            let near = ws
                .history
                .iter()
                .rev()
                .copied()
                .find(|w| ws.layout.contains(*w));
            ws.layout.insert(id, near, area, &self.settings);
        } else {
            m.floating = true;
            m.float_rect = centered;
            ws.layout.remove(id);
            ws.floating.push(id);
        }
        let mut effects = self.sync();
        effects.push(Effect::Focus(id));
        effects
    }

    fn toggle_fullscreen(&mut self) -> Vec<Effect> {
        let Some((id, n)) = self.active() else {
            return Vec::new();
        };
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return Vec::new();
        };
        ws.fullscreen = if ws.fullscreen == Some(id) {
            None
        } else {
            Some(id)
        };
        let mut effects = self.sync();
        effects.push(Effect::Focus(id));
        effects
    }

    fn move_focus(&mut self, dir: Direction) -> Vec<Effect> {
        let Some(n) = self.focused_key() else {
            return Vec::new();
        };
        let current = self.active().filter(|(_, ws)| *ws == n).map(|(id, _)| id);
        let Some(id) = current else {
            // Nothing focused here (e.g. the desktop has focus): start from a window.
            return match self.workspaces.get(&n).and_then(Workspace::focus_candidate) {
                Some(candidate) => self.focus_window(candidate),
                None => self.focus_monitor(dir),
            };
        };
        // A fullscreen window owns its workspace; focus can only leave to another monitor.
        if self
            .workspaces
            .get(&n)
            .is_some_and(|ws| ws.fullscreen.is_some())
        {
            return self.focus_monitor(dir);
        }
        let area = self.area_of(n);
        let settings = self.settings.clone();
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return Vec::new();
        };
        let mut tiles = ws.layout.arrange(area, Some(id), &settings);
        let target = if ws.floating.contains(&id) {
            tiles.extend(ws.floating.iter().filter_map(|f| {
                self.windows.get(f).map(|m| Tile {
                    id: *f,
                    rect: m.float_rect,
                    visible: true,
                })
            }));
            let from = self.windows[&id].float_rect;
            nearest_in_direction(
                &from,
                dir,
                tiles
                    .iter()
                    .filter(|t| t.visible && t.id != id)
                    .map(|t| (t.id, t.rect)),
            )
        } else {
            ws.layout.neighbor(id, dir, &tiles)
        };
        match target {
            Some(t) => self.focus_window(t),
            None => self.focus_monitor(dir),
        }
    }

    /// Focuses the next / previous window of the focused workspace in layout order, wrapping.
    fn cycle(&mut self, forward: bool) -> Vec<Effect> {
        let Some(n) = self.focused_key() else {
            return Vec::new();
        };
        let Some(ws) = self.workspaces.get(&n) else {
            return Vec::new();
        };
        if ws.fullscreen.is_some() {
            return Vec::new();
        }
        let order: Vec<WindowId> = ws
            .layout
            .windows()
            .into_iter()
            .chain(ws.floating.iter().copied())
            .collect();
        let current = self.active().filter(|(_, w)| *w == n).map(|(id, _)| id);
        let target = match current.and_then(|id| order.iter().position(|w| *w == id)) {
            Some(i) if forward => order.get((i + 1) % order.len()),
            Some(i) => order.get((i + order.len() - 1) % order.len()),
            None => order.first(),
        };
        match target.copied() {
            Some(id) => self.focus_window(id),
            None => Vec::new(),
        }
    }

    fn move_window(&mut self, dir: Direction) -> Vec<Effect> {
        let Some((id, n)) = self.active() else {
            return Vec::new();
        };
        if self
            .workspaces
            .get(&n)
            .is_some_and(|ws| ws.fullscreen.is_some())
        {
            return Vec::new();
        }
        let area = self.area_of(n);
        if self.is_floating(id) {
            let step_x = scale(area.w, 0.05);
            let step_y = scale(area.h, 0.05);
            if let Some(m) = self.windows.get_mut(&id) {
                match dir {
                    Direction::Left => m.float_rect.x -= step_x,
                    Direction::Right => m.float_rect.x += step_x,
                    Direction::Up => m.float_rect.y -= step_y,
                    Direction::Down => m.float_rect.y += step_y,
                }
            }
            return self.sync();
        }
        let settings = self.settings.clone();
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return Vec::new();
        };
        let tiles = ws.layout.arrange(area, Some(id), &settings);
        if ws.layout.move_window(id, dir, &tiles) {
            return self.sync();
        }
        let Some(from) = self.showing(n) else {
            return Vec::new();
        };
        let Some(target) = self.monitor_in_direction(from, dir) else {
            return Vec::new();
        };
        let dest = self.monitors[target].workspace;
        self.detach(id);
        self.attach(id, dest);
        self.focus_window(id)
    }

    fn resize_active(&mut self, dx: i32, dy: i32) -> Vec<Effect> {
        let Some((id, n)) = self.active() else {
            return Vec::new();
        };
        if self.is_floating(id) {
            if let Some(m) = self.windows.get_mut(&id) {
                m.float_rect.w = (m.float_rect.w + dx).max(100);
                m.float_rect.h = (m.float_rect.h + dy).max(100);
            }
            return self.sync();
        }
        let area = self.area_of(n);
        let settings = self.settings.clone();
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.layout.resize(id, dx, dy, area, &settings);
        }
        self.sync()
    }

    /// Shows workspace `label` on the focused monitor, and on every monitor when they are synced.
    fn switch_workspace(&mut self, label: u8) -> Vec<Effect> {
        let Some(focused) = self.focused_monitor_id() else {
            return Vec::new();
        };
        if !self.settings.independent_workspaces {
            for i in 0..self.monitors.len() {
                let id = self.monitors[i].info.id;
                self.monitors[i].workspace = self.workspace_for(label, id);
            }
        }
        let key = self.workspace_for(label, focused);
        self.focus_workspace(key)
    }

    fn move_to_workspace(&mut self, label: u8, follow: bool) -> Vec<Effect> {
        let Some((id, current)) = self.active() else {
            return Vec::new();
        };
        let Some(monitor) = self.focused_monitor_id() else {
            return Vec::new();
        };
        let n = self.workspace_for(label, monitor);
        if current == n {
            return Vec::new();
        }
        let old_area = self.area_of(current);
        let new_area = self.area_of(n);
        self.detach(id);
        if let Some(m) = self.windows.get_mut(&id) {
            m.float_rect.x += new_area.x - old_area.x;
            m.float_rect.y += new_area.y - old_area.y;
        }
        self.attach(id, n);
        if follow {
            return self.switch_workspace(label);
        }
        self.focused = None;
        match self
            .workspaces
            .get(&current)
            .and_then(Workspace::focus_candidate)
        {
            Some(next) => self.focus_window(next),
            None => self.sync(),
        }
    }

    fn set_layout(&mut self, kind: LayoutKind) -> Vec<Effect> {
        let Some(n) = self.focused_key() else {
            return Vec::new();
        };
        let area = self.area_of(n);
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.layout = ws.layout.convert(kind, area, &self.settings);
        }
        self.sync()
    }

    fn layout_specific(&mut self, command: &Command) -> Vec<Effect> {
        let Some((id, n)) = self.active() else {
            return Vec::new();
        };
        let area = self.area_of(n);
        let s = &self.settings;
        let Some(ws) = self.workspaces.get_mut(&n) else {
            return Vec::new();
        };
        match (&mut ws.layout, command) {
            (LayoutState::Master(m), Command::SwapWithMaster) => m.swap_with_master(id, s),
            (LayoutState::Dwindle(d), Command::SwapWithMaster) => d.swap_with_main(id),
            (LayoutState::Dwindle(d), Command::ToggleSplit) => d.toggle_split(id, area, s),
            (LayoutState::Scrolling(l), Command::ScrollerProportion(p)) => l.set_proportion(id, *p),
            (LayoutState::Scrolling(l), Command::CycleProportion) => l.cycle_proportion(id, s),
            (LayoutState::Scrolling(l), Command::ConsumeWindow(dir)) => l.consume(id, *dir),
            (LayoutState::Scrolling(l), Command::ExpelWindow) => l.expel(id, s),
            (LayoutState::Scrolling(l), Command::CenterColumn) => l.center(),
            _ => return Vec::new(),
        }
        self.sync()
    }

    pub fn snapshot_windows(&self) -> Vec<WindowSnapshot> {
        let mut out: Vec<_> = self
            .windows
            .iter()
            .map(|(id, m)| WindowSnapshot {
                id: id.0,
                title: m.info.title.clone(),
                class: m.info.class.clone(),
                exe: m.info.exe.clone(),
                workspace: self.workspaces.get(&m.workspace).map_or(0, |w| w.label),
                floating: m.floating,
                fullscreen: self.workspaces.get(&m.workspace).and_then(|w| w.fullscreen)
                    == Some(*id),
                focused: self.focused == Some(*id),
                hidden: self.hidden.contains(id),
            })
            .collect();
        out.sort_by_key(|w| (w.workspace, w.id));
        out
    }

    pub fn snapshot_active_window(&self) -> Option<WindowSnapshot> {
        let id = self.focused?;
        self.snapshot_windows().into_iter().find(|w| w.id == id.0)
    }

    pub fn snapshot_workspaces(&self) -> Vec<WorkspaceSnapshot> {
        let focused_ws = self.focused_key();
        self.workspaces
            .iter()
            .map(|(n, ws)| WorkspaceSnapshot {
                id: ws.label,
                monitor: self.monitor_index(ws.monitor),
                visible: self.showing(*n).is_some(),
                focused: focused_ws == Some(*n),
                layout: ws.layout.kind(),
                windows: ws.len(),
            })
            .collect()
    }

    pub fn snapshot_monitors(&self) -> Vec<MonitorSnapshot> {
        self.monitors
            .iter()
            .enumerate()
            .map(|(i, m)| MonitorSnapshot {
                index: i,
                rect: m.info.rect,
                work_area: m.info.work_area,
                workspace: self.workspaces.get(&m.workspace).map_or(0, |w| w.label),
                focused: i == self.focused_monitor,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: WindowId = WindowId(1);
    const X: WindowId = WindowId(2);
    const Y: WindowId = WindowId(3);

    fn monitor(id: isize, x: i32) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId(id),
            rect: Rect::new(x, 0, 1000, 1000),
            work_area: Rect::new(x, 0, 1000, 960),
        }
    }

    fn state(monitors: usize) -> State {
        state_with(monitors, true)
    }

    fn state_with(monitors: usize, independent: bool) -> State {
        let settings = LayoutSettings {
            gaps_in: 0,
            gaps_out: 0,
            independent_workspaces: independent,
            ..LayoutSettings::default()
        };
        let mut s = State::new(settings, LayoutKind::Dwindle);
        let infos = (0..monitors)
            .map(|i| monitor(i as isize + 1, i as i32 * 1000))
            .collect();
        s.set_monitors(infos);
        s
    }

    fn open(s: &mut State, id: WindowId) -> Vec<Effect> {
        s.window_opened(
            id,
            WindowInfo::default(),
            ManageOptions {
                focus: true,
                ..Default::default()
            },
        )
    }

    fn placed(effects: &[Effect], id: WindowId) -> Option<Rect> {
        effects.iter().rev().find_map(|e| match e {
            Effect::Place { id: i, rect } if *i == id => Some(*rect),
            _ => None,
        })
    }

    #[test]
    fn monitors_have_workspaces_of_their_own() {
        let s = state(2);
        let ws: Vec<_> = s.snapshot_monitors().iter().map(|m| m.workspace).collect();
        assert_eq!(ws, vec![1, 1]);
        assert_eq!(s.snapshot_workspaces().len(), 2);
    }

    #[test]
    fn switching_workspace_stays_on_the_monitor() {
        let mut s = state(2);
        open(&mut s, W);
        let effects = s.apply(&Command::Workspace(2));
        assert!(effects.contains(&Effect::Hide(W)));
        let monitors = s.snapshot_monitors();
        assert!(monitors[0].focused);
        assert_eq!((monitors[0].workspace, monitors[1].workspace), (2, 1));
    }

    #[test]
    fn an_empty_workspace_keeps_focus_on_its_monitor() {
        let mut s = state(2);
        open(&mut s, W);
        s.apply(&Command::MoveFocus(Direction::Right));
        open(&mut s, X);
        s.apply(&Command::MoveFocus(Direction::Left));
        let effects = s.apply(&Command::Workspace(2));
        assert!(effects.contains(&Effect::Defocus));
        assert!(s.snapshot_monitors()[0].focused);
        assert_eq!(s.focused(), None);
    }

    #[test]
    fn synced_monitors_switch_together() {
        let mut s = state_with(2, false);
        s.apply(&Command::Workspace(3));
        let ws: Vec<_> = s.snapshot_monitors().iter().map(|m| m.workspace).collect();
        assert_eq!(ws, vec![3, 3]);
    }

    #[test]
    fn opening_tiles_and_focuses() {
        let mut s = state(1);
        open(&mut s, W);
        let effects = open(&mut s, X);
        assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 500, 960)));
        assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
        assert_eq!(s.focused(), Some(X));
    }

    #[test]
    fn workspace_switch_hides_and_shows() {
        let mut s = state(1);
        open(&mut s, W);
        let effects = s.apply(&Command::Workspace(2));
        assert!(effects.contains(&Effect::Hide(W)));
        assert_eq!(s.focused(), None);
        let effects = s.apply(&Command::Workspace(1));
        assert!(effects.contains(&Effect::Show(W)));
        assert!(effects.contains(&Effect::Focus(W)));
    }

    #[test]
    fn move_to_workspace_follows_the_window() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.apply(&Command::MoveToWorkspace(3));
        assert!(effects.contains(&Effect::Focus(X)));
        assert!(effects.contains(&Effect::Hide(W)));
        assert_eq!(s.focused(), Some(X));
        assert_eq!(s.focused_workspace(), Some(3));
    }

    #[test]
    fn move_to_workspace_silent_keeps_view() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.apply(&Command::MoveToWorkspaceSilent(3));
        assert!(effects.contains(&Effect::Hide(X)));
        assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
        assert_eq!(s.focused(), Some(W));
        assert_eq!(s.focused_workspace(), Some(1));
    }

    #[test]
    fn focus_crosses_monitors() {
        let mut s = state(2);
        open(&mut s, W);
        let effects = s.apply(&Command::MoveFocus(Direction::Right));
        assert!(s.snapshot_monitors()[1].focused);
        assert!(!effects.iter().any(|e| matches!(e, Effect::Focus(_))));
        s.apply(&Command::MoveFocus(Direction::Left));
        assert_eq!(s.focused(), Some(W));
    }

    #[test]
    fn focus_enters_the_nearest_window_on_the_next_monitor() {
        let mut s = state(2);
        open(&mut s, W);
        s.apply(&Command::MoveFocus(Direction::Right));
        open(&mut s, X);
        open(&mut s, Y);
        s.apply(&Command::MoveFocus(Direction::Left));
        s.apply(&Command::MoveFocus(Direction::Left));
        assert_eq!(s.focused(), Some(W));
        let effects = s.apply(&Command::MoveFocus(Direction::Right));
        assert!(effects.contains(&Effect::Focus(X)));
        assert_eq!(s.focused(), Some(X));
        s.apply(&Command::MoveFocus(Direction::Right));
        s.apply(&Command::MoveFocus(Direction::Left));
        assert_eq!(s.focused(), Some(X));
    }

    #[test]
    fn portrait_monitors_start_with_even() {
        let mut s = State::new(LayoutSettings::default(), LayoutKind::Dwindle);
        s.set_monitors(vec![
            monitor(1, 0),
            MonitorInfo {
                id: MonitorId(2),
                rect: Rect::new(1000, 0, 1080, 1920),
                work_area: Rect::new(1000, 0, 1080, 1880),
            },
        ]);
        let layouts: Vec<_> = s.snapshot_workspaces().iter().map(|w| w.layout).collect();
        assert_eq!(layouts, vec![LayoutKind::Dwindle, LayoutKind::Even]);
        s.set_monitors(vec![monitor(1, 0), monitor(2, 1000)]);
        assert_eq!(s.snapshot_workspaces()[1].layout, LayoutKind::Dwindle);
    }

    #[test]
    fn move_window_to_other_monitor_at_edge() {
        let mut s = state(2);
        open(&mut s, W);
        let effects = s.apply(&Command::MoveWindow(Direction::Right));
        assert_eq!(placed(&effects, W), Some(Rect::new(1000, 0, 1000, 960)));
        assert!(s.snapshot_monitors()[1].focused);
    }

    #[test]
    fn closing_focuses_previous() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        open(&mut s, Y);
        s.window_focused(X);
        let effects = s.window_closed(X);
        assert!(effects.contains(&Effect::Focus(Y)));
    }

    #[test]
    fn cycle_wraps_in_layout_order() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        open(&mut s, Y);
        assert!(s.apply(&Command::CycleNext).contains(&Effect::Focus(W)));
        assert!(s.apply(&Command::CyclePrev).contains(&Effect::Focus(Y)));
    }

    #[test]
    fn floating_and_fullscreen() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.apply(&Command::ToggleFloating);
        assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
        assert!(s.is_floating(X));
        s.window_focused(W);
        let effects = s.apply(&Command::Fullscreen);
        assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 1000)));
    }

    #[test]
    fn fullscreen_owns_the_workspace() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        s.apply(&Command::Fullscreen);
        // Focus can't wander to windows hidden behind the fullscreen one.
        assert!(s.apply(&Command::MoveFocus(Direction::Left)).is_empty());
        assert!(s.apply(&Command::CycleNext).is_empty());
        assert!(s.apply(&Command::MoveWindow(Direction::Left)).is_empty());
        assert_eq!(s.focused(), Some(X));
        // A new window taking focus ends fullscreen, so it is actually visible.
        let effects = open(&mut s, Y);
        assert_ne!(placed(&effects, X), Some(Rect::new(0, 0, 1000, 1000)));
    }

    #[test]
    fn minimize_and_restore() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.window_minimized(X);
        assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
        let effects = s.window_restored(X);
        assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
    }

    #[test]
    fn scrolling_hides_offscreen_columns() {
        let mut s = state(1);
        s.apply(&Command::Layout(LayoutKind::Scrolling));
        open(&mut s, W);
        open(&mut s, X);
        let effects = open(&mut s, Y);
        assert!(effects.contains(&Effect::Hide(W)));
        let effects = s.apply(&Command::MoveFocus(Direction::Left));
        let effects = [effects, s.apply(&Command::MoveFocus(Direction::Left))].concat();
        assert!(effects.contains(&Effect::Show(W)));
        assert_eq!(s.focused(), Some(W));
    }

    #[test]
    fn monitor_removal_keeps_windows() {
        let mut s = state(2);
        s.apply(&Command::FocusMonitor(Direction::Right));
        open(&mut s, W);
        let effects = s.set_monitors(vec![monitor(1, 0)]);
        assert_eq!(s.monitor_count(), 1);
        assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
    }

    #[test]
    fn dragging_an_edge_over_a_neighbour_resizes_instead_of_swapping() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        // W's right edge dragged well into X, the pointer ending over X.
        let effects = s.window_moved(W, Rect::new(0, 0, 700, 960), (690, 400));
        assert_eq!(placed(&effects, W).map(|r| (r.x, r.w)), Some((0, 700)));
        assert_eq!(placed(&effects, X).map(|r| r.x), Some(700));
    }

    #[test]
    fn dragging_an_edge_moves_the_others_as_it_goes() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        assert!(s.window_resizing(W, Rect::new(0, 0, 500, 960)).is_empty());
        let effects = s.window_resizing(W, Rect::new(0, 0, 600, 960));
        assert_eq!(placed(&effects, X).map(|r| r.x), Some(600));
        let settled = s.window_resized(W, Rect::new(0, 0, 600, 960));
        assert_eq!(placed(&settled, W).map(|r| r.w), Some(600));
    }

    #[test]
    fn drag_onto_other_tile_swaps() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.window_moved(W, Rect::new(600, 10, 500, 960), (700, 100));
        assert_eq!(placed(&effects, W), Some(Rect::new(500, 0, 500, 960)));
    }

    fn float(s: &mut State, id: WindowId) {
        s.window_opened(
            id,
            WindowInfo::default(),
            ManageOptions {
                floating: true,
                rect: Rect::new(100, 100, 400, 300),
                focus: true,
                ..Default::default()
            },
        );
    }

    #[test]
    fn opening_a_managed_window_again_does_nothing() {
        let mut s = state(1);
        open(&mut s, W);
        assert!(open(&mut s, W).is_empty());
        assert_eq!(s.managed().count(), 1);
    }

    #[test]
    fn nothing_happens_without_monitors() {
        let mut s = State::new(LayoutSettings::default(), LayoutKind::Dwindle);
        assert!(open(&mut s, W).is_empty());
        assert!(s.apply(&Command::Workspace(2)).is_empty());
        assert!(s.focused_monitor().is_none());
        assert!(s.focused_workspace().is_none());
        assert!(s.active_layout().is_none());
    }

    #[test]
    fn exec_and_kill_become_effects() {
        let mut s = state(1);
        assert_eq!(
            s.apply(&Command::Exec("calc".into())),
            vec![Effect::Exec("calc".into())]
        );
        assert!(s.apply(&Command::KillActive).is_empty());
        open(&mut s, W);
        assert_eq!(s.apply(&Command::KillActive), vec![Effect::Close(W)]);
    }

    #[test]
    fn commands_for_the_daemon_have_no_effects() {
        let mut s = state(1);
        open(&mut s, W);
        for command in [
            Command::FocusOrExec("x".into()),
            Command::Pin,
            Command::Lock,
            Command::Launcher,
            Command::Reload,
            Command::TogglePause,
            Command::ToggleTile,
            Command::ToggleRemap,
            Command::ToggleShell,
            Command::ToggleLog,
            Command::Exit,
        ] {
            assert!(s.apply(&command).is_empty(), "{command:?}");
        }
    }

    #[test]
    fn on_focused_monitor_follows_focus() {
        let mut s = state(2);
        open(&mut s, W);
        s.apply(&Command::FocusMonitor(Direction::Right));
        open(&mut s, X);
        assert!(s.on_focused_monitor(X));
        assert!(!s.on_focused_monitor(W));
        assert!(!s.on_focused_monitor(Y));
    }

    #[test]
    fn a_window_opened_for_another_workspace_is_hidden() {
        let mut s = state(1);
        let effects = s.window_opened(
            W,
            WindowInfo::default(),
            ManageOptions {
                workspace: Some(4),
                focus: true,
                ..Default::default()
            },
        );
        assert!(!effects.iter().any(|e| matches!(e, Effect::Focus(_))));
        assert!(s.is_hidden(W));
        assert_eq!(s.focused_workspace(), Some(1));
    }

    #[test]
    fn a_window_opened_for_another_monitor_goes_there() {
        let mut s = state(2);
        let effects = s.window_opened(
            W,
            WindowInfo::default(),
            ManageOptions {
                monitor: Some(1),
                focus: true,
                ..Default::default()
            },
        );
        assert_eq!(placed(&effects, W), Some(Rect::new(1000, 0, 1000, 960)));
        assert!(s.snapshot_monitors()[1].focused);
    }

    #[test]
    fn focus_from_the_os_on_a_hidden_workspace_switches_to_it() {
        let mut s = state(1);
        open(&mut s, W);
        s.apply(&Command::Workspace(2));
        assert!(s.is_hidden(W));
        let effects = s.window_focused(W);
        assert!(effects.contains(&Effect::Focus(W)));
        assert_eq!(s.focused_workspace(), Some(1));
    }

    #[test]
    fn focus_on_an_unmanaged_window_clears_focus() {
        let mut s = state(1);
        open(&mut s, W);
        assert!(s.window_focused(Y).is_empty());
        assert_eq!(s.focused(), None);
    }

    #[test]
    fn focus_on_the_focused_window_is_a_no_op() {
        let mut s = state(1);
        open(&mut s, W);
        assert!(s.window_focused(W).is_empty());
    }

    #[test]
    fn focus_from_the_os_moves_the_focused_monitor() {
        let mut s = state(2);
        open(&mut s, W);
        s.apply(&Command::FocusMonitor(Direction::Right));
        open(&mut s, X);
        s.window_focused(W);
        assert!(s.snapshot_monitors()[0].focused);
    }

    #[test]
    fn closing_the_last_window_defocuses() {
        let mut s = state(1);
        open(&mut s, W);
        let effects = s.window_closed(W);
        assert!(effects.contains(&Effect::Defocus));
        assert_eq!(s.focused(), None);
        assert!(!s.is_managed(W));
    }

    #[test]
    fn closing_an_unfocused_window_keeps_focus() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        s.window_closed(W);
        assert_eq!(s.focused(), Some(X));
        assert!(s.window_closed(Y).is_empty());
    }

    #[test]
    fn closing_a_window_on_a_hidden_workspace_keeps_focus() {
        let mut s = state(1);
        open(&mut s, W);
        s.apply(&Command::Workspace(2));
        open(&mut s, X);
        s.apply(&Command::MoveToWorkspaceSilent(1));
        s.window_closed(X);
        assert!(!s.is_managed(X));
        assert!(s.is_managed(W));
    }

    #[test]
    fn minimising_the_last_window_defocuses() {
        let mut s = state(1);
        open(&mut s, W);
        let effects = s.window_minimized(W);
        assert!(effects.contains(&Effect::Defocus));
        assert!(s.window_minimized(Y).is_empty());
    }

    #[test]
    fn restoring_a_window_that_is_not_minimised_does_nothing() {
        let mut s = state(1);
        open(&mut s, W);
        assert!(s.window_restored(W).is_empty());
        assert!(s.window_restored(Y).is_empty());
    }

    #[test]
    fn dragging_to_another_monitor_moves_the_window() {
        let mut s = state(2);
        open(&mut s, W);
        let effects = s.window_moved(W, Rect::new(1200, 100, 400, 300), (1300, 200));
        assert_eq!(placed(&effects, W), Some(Rect::new(1000, 0, 1000, 960)));
        assert!(s.snapshot_monitors()[1].focused);
    }

    #[test]
    fn dragging_a_floating_window_keeps_its_place() {
        let mut s = state(1);
        float(&mut s, W);
        let effects = s.window_moved(W, Rect::new(200, 220, 400, 300), (300, 300));
        assert_eq!(placed(&effects, W), Some(Rect::new(200, 220, 400, 300)));
        assert!(s.window_moved(Y, Rect::default(), (0, 0)).is_empty());
    }

    #[test]
    fn floating_windows_move_and_resize_in_steps() {
        let mut s = state(1);
        float(&mut s, W);
        let start = placed(&s.sync(), W).unwrap();
        let right = s.apply(&Command::MoveWindow(Direction::Right));
        assert_eq!(placed(&right, W).unwrap().x, start.x + 50);
        let down = s.apply(&Command::MoveWindow(Direction::Down));
        assert!(placed(&down, W).unwrap().y > start.y);
        s.apply(&Command::MoveWindow(Direction::Left));
        s.apply(&Command::MoveWindow(Direction::Up));
        let grown = s.apply(&Command::ResizeActive(40, 20));
        assert_eq!(placed(&grown, W).map(|r| (r.w, r.h)), Some((440, 320)));
        let floor = s.apply(&Command::ResizeActive(-1000, -1000));
        assert_eq!(placed(&floor, W).map(|r| (r.w, r.h)), Some((100, 100)));
    }

    #[test]
    fn resizing_a_tiled_window_changes_its_split() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.apply(&Command::ResizeActive(100, 0));
        let (w, x) = (placed(&effects, W).unwrap(), placed(&effects, X).unwrap());
        assert_ne!(w.w, 500);
        assert_eq!(w.w + x.w, 1000);
        assert!(!s.apply(&Command::ResizeActive(1, 1)).is_empty());
    }

    #[test]
    fn resizing_with_nothing_focused_does_nothing() {
        let mut s = state(1);
        assert!(s.apply(&Command::ResizeActive(10, 10)).is_empty());
        assert!(s.apply(&Command::MoveWindow(Direction::Left)).is_empty());
        assert!(s.apply(&Command::ToggleFloating).is_empty());
        assert!(s.apply(&Command::Fullscreen).is_empty());
    }

    #[test]
    fn a_tiled_window_swaps_with_its_neighbour() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.apply(&Command::MoveWindow(Direction::Left));
        assert_eq!(placed(&effects, X), Some(Rect::new(0, 0, 500, 960)));
        assert_eq!(placed(&effects, W), Some(Rect::new(500, 0, 500, 960)));
    }

    #[test]
    fn moving_a_window_past_the_last_monitor_does_nothing() {
        let mut s = state(1);
        open(&mut s, W);
        assert!(s.apply(&Command::MoveWindow(Direction::Right)).is_empty());
    }

    #[test]
    fn a_floating_window_tiles_again() {
        let mut s = state(1);
        open(&mut s, W);
        float(&mut s, X);
        assert!(s.is_floating(X));
        let effects = s.apply(&Command::ToggleFloating);
        assert!(!s.is_floating(X));
        assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
        assert!(effects.contains(&Effect::Focus(X)));
    }

    #[test]
    fn fullscreen_toggles_off() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        s.apply(&Command::Fullscreen);
        let effects = s.apply(&Command::Fullscreen);
        assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
        assert!(!s.snapshot_windows().iter().any(|w| w.fullscreen));
    }

    #[test]
    fn focus_moves_between_floating_windows() {
        let mut s = state(1);
        float(&mut s, W);
        float(&mut s, X);
        for _ in 0..10 {
            s.apply(&Command::MoveWindow(Direction::Right));
        }
        assert_eq!(s.focused(), Some(X));
        s.apply(&Command::MoveFocus(Direction::Left));
        assert_eq!(s.focused(), Some(W));
    }

    #[test]
    fn focus_starts_from_a_window_when_none_is_focused() {
        let mut s = state(1);
        open(&mut s, W);
        s.apply(&Command::Workspace(2));
        s.apply(&Command::Workspace(1));
        s.window_focused(Y);
        assert_eq!(s.focused(), None);
        let effects = s.apply(&Command::MoveFocus(Direction::Left));
        assert!(effects.contains(&Effect::Focus(W)));
    }

    #[test]
    fn cycling_an_empty_workspace_does_nothing() {
        let mut s = state(1);
        assert!(s.apply(&Command::CycleNext).is_empty());
        assert!(s.apply(&Command::CyclePrev).is_empty());
    }

    #[test]
    fn cycling_with_nothing_focused_starts_at_the_first() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        s.window_focused(Y);
        assert!(s.apply(&Command::CycleNext).contains(&Effect::Focus(W)));
    }

    #[test]
    fn moving_to_the_current_workspace_does_nothing() {
        let mut s = state(1);
        open(&mut s, W);
        assert!(s.apply(&Command::MoveToWorkspace(1)).is_empty());
        let mut empty = state(1);
        assert!(empty.apply(&Command::MoveToWorkspace(2)).is_empty());
    }

    #[test]
    fn moving_the_only_window_away_silently_leaves_the_workspace_empty() {
        let mut s = state(1);
        open(&mut s, W);
        let effects = s.apply(&Command::MoveToWorkspaceSilent(2));
        assert!(effects.contains(&Effect::Hide(W)));
        assert_eq!(s.focused(), None);
    }

    #[test]
    fn layouts_switch_and_cycle() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        s.apply(&Command::Layout(LayoutKind::Monocle));
        assert_eq!(s.active_layout(), Some(LayoutKind::Monocle));
        let effects = s.sync();
        assert_eq!(placed(&effects, W), placed(&effects, X));
        s.apply(&Command::CycleLayout);
        assert_eq!(s.active_layout(), Some(LayoutKind::Scrolling));
        assert_eq!(s.snapshot_windows().len(), 2);
    }

    #[test]
    fn layout_commands_ignore_other_layouts() {
        let mut s = state(1);
        open(&mut s, W);
        s.apply(&Command::Layout(LayoutKind::Monocle));
        for command in [
            Command::CycleProportion,
            Command::ScrollerProportion(0.5),
            Command::ExpelWindow,
            Command::CenterColumn,
            Command::ConsumeWindow(Direction::Left),
            Command::ToggleSplit,
            Command::SwapWithMaster,
        ] {
            assert!(s.apply(&command).is_empty(), "{command:?}");
        }
    }

    #[test]
    fn scrolling_commands_change_the_columns() {
        let mut s = state(1);
        s.apply(&Command::Layout(LayoutKind::Scrolling));
        open(&mut s, W);
        open(&mut s, X);
        let before = s.sync();
        let after = s.apply(&Command::ScrollerProportion(1.0));
        assert_ne!(placed(&before, X), placed(&after, X));
        assert!(!s.apply(&Command::CycleProportion).is_empty());
        assert!(!s.apply(&Command::CenterColumn).is_empty());
        assert!(!s.apply(&Command::ConsumeWindow(Direction::Left)).is_empty());
        assert!(!s.apply(&Command::ExpelWindow).is_empty());
    }

    #[test]
    fn dwindle_commands_rearrange() {
        let mut s = state(1);
        open(&mut s, W);
        open(&mut s, X);
        let before = placed(&s.sync(), X);
        let effects = s.apply(&Command::ToggleSplit);
        assert_ne!(placed(&effects, X), before);
        let effects = s.apply(&Command::SwapWithMaster);
        assert!(placed(&effects, W).is_some());
    }

    #[test]
    fn master_swap_puts_the_focused_window_first() {
        let mut s = state(1);
        s.apply(&Command::Layout(LayoutKind::Master));
        open(&mut s, W);
        open(&mut s, X);
        let effects = s.apply(&Command::SwapWithMaster);
        assert!(placed(&effects, X).unwrap().x < placed(&effects, W).unwrap().x);
    }

    #[test]
    fn settings_changes_reflow_and_info_updates() {
        let mut s = state(1);
        open(&mut s, W);
        let settings = LayoutSettings {
            gaps_in: 0,
            gaps_out: 20,
            ..LayoutSettings::default()
        };
        let effects = s.set_settings(settings, LayoutKind::Dwindle);
        assert_eq!(placed(&effects, W), Some(Rect::new(20, 20, 960, 920)));
        s.set_info(
            W,
            WindowInfo {
                title: "t".into(),
                class: "c".into(),
                exe: "e.exe".into(),
            },
        );
        let snap = s.snapshot_active_window().unwrap();
        assert_eq!((snap.title.as_str(), snap.exe.as_str()), ("t", "e.exe"));
        s.set_info(Y, WindowInfo::default());
    }

    #[test]
    fn snapshots_describe_the_desktop() {
        let mut s = state(2);
        open(&mut s, W);
        s.apply(&Command::Workspace(2));
        open(&mut s, X);
        let windows = s.snapshot_windows();
        assert_eq!(windows.len(), 2);
        assert!(windows.windows(2).all(|p| p[0].workspace <= p[1].workspace));
        assert_eq!(s.snapshot_active_window().map(|w| w.id), Some(X.0));
        assert!(windows.iter().any(|w| w.id == W.0 && w.hidden));
        let workspaces = s.snapshot_workspaces();
        assert!(
            workspaces
                .iter()
                .any(|w| w.id == 2 && w.focused && w.visible)
        );
        assert!(workspaces.iter().any(|w| w.id == 1 && !w.visible));
        assert_eq!(s.snapshot_monitors().len(), 2);
        assert_eq!(s.hidden().count(), 1);
    }

    #[test]
    fn reopening_a_monitor_restores_its_workspaces() {
        let mut s = state(2);
        s.apply(&Command::FocusMonitor(Direction::Right));
        open(&mut s, W);
        s.set_monitors(vec![monitor(1, 0)]);
        s.set_monitors(vec![monitor(1, 0), monitor(2, 1000)]);
        assert_eq!(s.monitor_count(), 2);
        assert!(s.is_managed(W));
    }
}
