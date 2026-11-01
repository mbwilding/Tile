//! Windows coming and going, and the user dragging them.

use crate::{WindowId, geometry::Rect};

use super::{Effect, ManageOptions, Managed, State, WindowInfo, workspace::Workspace};

impl State {
    /// Removes a window from its workspace structures, returning the workspace number.
    pub(super) fn detach(&mut self, id: WindowId) -> Option<u8> {
        let n = self.windows.get(&id)?.workspace;
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.forget(id);
        }
        Some(n)
    }

    /// Adds a managed window to workspace `n`, next to that workspace's focused window.
    pub(super) fn attach(&mut self, id: WindowId, n: u8) {
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

    pub(super) fn centered(&self, size: Rect, n: u8) -> Rect {
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

    /// Takes a minimised window out of its layout until it is restored.
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

    /// Puts a restored window back in its layout.
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
    pub(super) fn resize_to(&mut self, id: WindowId, rect: Rect) -> bool {
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
}
