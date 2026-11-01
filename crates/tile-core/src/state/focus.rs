//! Moving focus between windows, workspaces and monitors.

use crate::{
    WindowId,
    geometry::{Direction, Rect, nearest_in_direction},
    layout::Tile,
};

use super::{Effect, State, workspace::Workspace};

impl State {
    pub(super) fn focus_window(&mut self, id: WindowId) -> Vec<Effect> {
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

    pub(super) fn focus_workspace(&mut self, n: u8) -> Vec<Effect> {
        self.focus_workspace_with(n, None)
    }

    pub(super) fn focus_workspace_with(
        &mut self,
        n: u8,
        preferred: Option<WindowId>,
    ) -> Vec<Effect> {
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

    pub(super) fn focus_monitor(&mut self, dir: Direction) -> Vec<Effect> {
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
    pub(super) fn active_rect(&mut self) -> Option<Rect> {
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
    pub(super) fn nearest_window(&mut self, n: u8, from: Rect, dir: Direction) -> Option<WindowId> {
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

    pub(super) fn monitor_in_direction(&self, from: usize, dir: Direction) -> Option<usize> {
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

    pub(super) fn move_focus(&mut self, dir: Direction) -> Vec<Effect> {
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
    pub(super) fn cycle(&mut self, forward: bool) -> Vec<Effect> {
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
}
