//! A serialisable picture of the state, for IPC.

use serde::{Deserialize, Serialize};

use crate::{geometry::Rect, layout::LayoutKind};

use super::State;

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

impl State {
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
