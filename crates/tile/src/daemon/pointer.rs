//! Hiding the pointer while typing, and the Win+drag mouse hook.

use tile_platform::{DragEvent, cursor_pos, mouse_button_down};
use tokio::time::Instant;

use super::{CURSOR_POLL, Daemon};

impl Daemon {
    /// Starts or stops hiding the pointer while typing, to match `cursor.hide_on_key`.
    pub(super) fn update_cursor(&mut self) {
        self.win_drag
            .set_enabled(self.loaded.config.cursor.drag && self.tiling && !self.paused);
        let hide = self.loaded.config.cursor.hide_on_key && !self.paused;
        self.hotkeys.set_report_typing(hide);
        if !hide {
            self.show_cursor();
        }
    }

    /// A key was typed: hide the pointer until the mouse moves.
    pub(super) fn hide_cursor(&mut self) {
        if self.cursor_hidden.is_some() || self.paused || !self.loaded.config.cursor.hide_on_key {
            return;
        }
        // Recorded first: if we die mid-way, the watchdog still restores the cursors.
        self.cursor_hidden = Some(cursor_pos());
        self.persist_restore();
        if !tile_input::hide_cursor() {
            tracing::debug!("some system cursors couldn't be hidden");
        }
        self.cursor_check_at = Some(Instant::now() + CURSOR_POLL);
    }

    /// Win + left-drag: the hook moves the window; here it is kept clear of tiling while it
    /// moves, and re-tiled where it is dropped.
    pub(super) fn on_drag(&mut self, event: DragEvent) {
        match event {
            DragEvent::Start { window: id } => {
                if !self.desktops.is_managed(id) || self.desktops.is_hidden(id) {
                    return;
                }
                self.windows.focus(id);
                let tracked = self.track(id);
                tracked.moving = true;
                tracked.tween = None;
                self.drag = Some(id);
            }
            DragEvent::End { x, y } => {
                let Some(id) = self.drag.take() else {
                    return;
                };
                let tracked = self.track(id);
                tracked.moving = false;
                tracked.placed = None;
                if let Some(rect) = self.windows.frame(id) {
                    let effects = self
                        .desktops
                        .with_owner(id, |s| s.window_moved(id, rect, (x, y)));
                    self.apply(effects);
                }
            }
        }
    }

    /// Brings a hidden pointer back.
    pub(super) fn show_cursor(&mut self) {
        if self.cursor_hidden.take().is_none() {
            return;
        }
        self.cursor_check_at = None;
        if !tile_input::restore_cursors() {
            tracing::warn!("failed to restore the system cursors");
        }
        self.persist_restore();
    }

    /// Shows the hidden pointer once the mouse moved or a button is pressed.
    pub(super) fn check_cursor(&mut self) {
        if self.cursor_hidden != Some(cursor_pos()) || mouse_button_down() {
            self.show_cursor();
        } else {
            self.cursor_check_at = Some(Instant::now() + CURSOR_POLL);
        }
    }
}
