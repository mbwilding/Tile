//! A serialisable picture of the state, for IPC.

use serde::{Deserialize, Serialize};

use crate::{geometry::Rect, layout::LayoutKind};

use super::State;

/// A managed window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSnapshot {
    /// Its handle.
    pub id: isize,
    /// Its title.
    pub title: String,
    /// Its window class.
    pub class: String,
    /// Its program's file name.
    pub exe: String,
    /// The workspace it is on.
    pub workspace: u8,
    /// Whether it floats.
    pub floating: bool,
    /// Whether it fills its monitor.
    pub fullscreen: bool,
    /// Whether it has focus.
    pub focused: bool,
    /// Whether Tile hid it.
    pub hidden: bool,
}

/// A workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Its number.
    pub id: u8,
    /// The index of the monitor it belongs to, if that is connected.
    pub monitor: Option<usize>,
    /// Whether a monitor shows it.
    pub visible: bool,
    /// Whether it is on the focused monitor.
    pub focused: bool,
    /// Its layout.
    pub layout: LayoutKind,
    /// How many windows it holds.
    pub windows: usize,
}

/// A monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorSnapshot {
    /// Its index in the monitor list.
    pub index: usize,
    /// The whole monitor.
    pub rect: Rect,
    /// The monitor less the taskbar and other app bars.
    pub work_area: Rect,
    /// The workspace it shows.
    pub workspace: u8,
    /// Whether it has focus.
    pub focused: bool,
}

impl State {
    /// Every managed window, by workspace.
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

    /// The focused window.
    pub fn snapshot_active_window(&self) -> Option<WindowSnapshot> {
        let id = self.focused?;
        self.snapshot_windows().into_iter().find(|w| w.id == id.0)
    }

    /// Every workspace made so far.
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

    /// Every monitor.
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
