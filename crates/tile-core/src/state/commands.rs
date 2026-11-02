//! Dispatchers: what `Command`s do to the focused window and workspace.

use crate::{
    Command, WindowId,
    geometry::{Direction, scale},
    layout::{LayoutKind, LayoutState},
};

use super::{Effect, State, workspace::Workspace};

impl State {
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
            Command::ResetLayout => {
                let kind = self.active_layout().unwrap_or(self.default_layout);
                self.set_layout(kind)
            }
            Command::SwapWithMaster
            | Command::ToggleSplit
            | Command::ScrollerProportion(_)
            | Command::CycleProportion
            | Command::ConsumeWindow(_)
            | Command::ExpelWindow
            | Command::CenterColumn => self.layout_specific(command),
            // Handled by the daemon.
            Command::ExecHidden(_)
            | Command::FocusOrExec(_)
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
            | Command::ToggleDnd
            | Command::Exit => Vec::new(),
        }
    }

    /// The focused window if it lives on the focused workspace.
    pub(super) fn active(&self) -> Option<(WindowId, u8)> {
        let id = self.focused?;
        let n = self.windows.get(&id)?.workspace;
        Some((id, n))
    }

    pub(super) fn toggle_floating(&mut self) -> Vec<Effect> {
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

    pub(super) fn toggle_fullscreen(&mut self) -> Vec<Effect> {
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

    pub(super) fn move_window(&mut self, dir: Direction) -> Vec<Effect> {
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

    pub(super) fn resize_active(&mut self, dx: i32, dy: i32) -> Vec<Effect> {
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
            // The keys move an edge: right and down move it that way, even when it is the left or
            // top edge of a window against the far side that the divider is on.
            let (dx, dy) = match ws.layout.kind() {
                LayoutKind::Scrolling => (dx, dy),
                _ => {
                    let bounds = settings.tiling_area(area);
                    let own = ws
                        .layout
                        .arrange(area, Some(id), &settings)
                        .into_iter()
                        .find(|t| t.id == id)
                        .map(|t| t.rect);
                    own.map_or((dx, dy), |r| {
                        let flip = |at_far: bool, at_near: bool, d: i32| {
                            if at_far && !at_near { -d } else { d }
                        };
                        (
                            flip(r.x + r.w >= bounds.x + bounds.w, r.x <= bounds.x, dx),
                            flip(r.y + r.h >= bounds.y + bounds.h, r.y <= bounds.y, dy),
                        )
                    })
                }
            };
            ws.layout.resize(id, dx, dy, area, &settings);
        }
        self.sync()
    }

    /// Shows workspace `label` on the focused monitor, and on every monitor when they are synced.
    pub(super) fn switch_workspace(&mut self, label: u8) -> Vec<Effect> {
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

    pub(super) fn move_to_workspace(&mut self, label: u8, follow: bool) -> Vec<Effect> {
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

    pub(super) fn set_layout(&mut self, kind: LayoutKind) -> Vec<Effect> {
        let Some(n) = self.focused_key() else {
            return Vec::new();
        };
        let area = self.area_of(n);
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.layout = ws.layout.convert(kind, area, &self.settings);
        }
        self.sync()
    }

    pub(super) fn layout_specific(&mut self, command: &Command) -> Vec<Effect> {
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
            (LayoutState::Dwindle(d), Command::ToggleSplit) => {
                d.toggle_split(id, s.tiling_area(area), s);
            }
            (LayoutState::Scrolling(l), Command::ScrollerProportion(p)) => l.set_proportion(id, *p),
            (LayoutState::Scrolling(l), Command::CycleProportion) => l.cycle_proportion(id, s),
            (LayoutState::Scrolling(l), Command::ConsumeWindow(dir)) => l.consume(id, *dir),
            (LayoutState::Scrolling(l), Command::ExpelWindow) => l.expel(id, s),
            (LayoutState::Scrolling(l), Command::CenterColumn) => l.center(),
            _ => return Vec::new(),
        }
        self.sync()
    }
}
