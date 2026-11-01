//! One workspace: its layout, floating and minimised windows and focus history.

use crate::{
    MonitorId, WindowId,
    layout::{LayoutKind, LayoutSettings, LayoutState},
};

#[derive(Debug, Clone)]
pub(super) struct Workspace {
    pub(super) monitor: MonitorId,
    /// The number the user knows it by; several monitors can each have a workspace 1.
    pub(super) label: u8,
    pub(super) layout: LayoutState,
    pub(super) floating: Vec<WindowId>,
    pub(super) minimized: Vec<WindowId>,
    pub(super) fullscreen: Option<WindowId>,
    /// Focus history, most recent last.
    pub(super) history: Vec<WindowId>,
}

impl Workspace {
    pub(super) fn new(monitor: MonitorId, label: u8, kind: LayoutKind, s: &LayoutSettings) -> Self {
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

    pub(super) fn all_windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.layout
            .windows()
            .into_iter()
            .chain(self.floating.iter().copied())
            .chain(self.minimized.iter().copied())
    }

    pub(super) fn touch(&mut self, id: WindowId) {
        self.history.retain(|w| *w != id);
        self.history.push(id);
        // Another window got focus (new window, alt-tab, restore): it must be visible.
        if self.fullscreen.is_some_and(|f| f != id) {
            self.fullscreen = None;
        }
    }

    pub(super) fn forget(&mut self, id: WindowId) {
        self.layout.remove(id);
        self.floating.retain(|w| *w != id);
        self.minimized.retain(|w| *w != id);
        self.history.retain(|w| *w != id);
        if self.fullscreen == Some(id) {
            self.fullscreen = None;
        }
    }

    pub(super) fn focus_candidate(&self) -> Option<WindowId> {
        self.history
            .iter()
            .rev()
            .copied()
            .find(|w| !self.minimized.contains(w))
            .or_else(|| self.layout.windows().first().copied())
            .or_else(|| self.floating.first().copied())
    }

    pub(super) fn len(&self) -> usize {
        self.layout.windows().len() + self.floating.len() + self.minimized.len()
    }
}
