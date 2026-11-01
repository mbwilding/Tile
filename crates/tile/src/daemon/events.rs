//! Window, tray and IPC events.

use tile_core::{Command, Effect, Rect, WindowId};
use tile_ipc::{Incoming, Query, Request, Response};
use tile_platform::{
    TrayEvent, WinEvent, cursor_pos, desktops as vd, focus_desktop, foreground, is_shell_window,
};
use tokio::time::Instant;

use super::{DEFOCUS_GRACE, Daemon, FORCE_GRACE, consume, on_other_desktop};
use crate::{launch, paths, remap};

impl Daemon {
    pub(super) fn on_foreground(&mut self, raw: isize) {
        let id = WindowId(raw);
        tracing::info!(
            "foreground {raw:#x} {:?} managed={}",
            self.windows.info(id).map(|i| i.exe),
            self.desktops.is_managed(id)
        );
        if !self.desktops.is_managed(id) {
            self.try_manage(raw);
            if !self.desktops.is_managed(id) {
                if !is_shell_window(raw) {
                    let effects = self.desktops.current().window_focused(id);
                    self.apply(effects);
                }
                return;
            }
        }
        // Windows picks a window on another monitor when the desktop can't hold focus; don't follow.
        let state = self.desktops.current_ref();
        if self
            .defocused_at
            .is_some_and(|at| at.elapsed() < DEFOCUS_GRACE)
            && state.focused().is_none()
            && !state.on_focused_monitor(id)
        {
            tracing::info!("ignoring focus moved to another monitor after a defocus");
            focus_desktop();
            return;
        }
        // Focus can arrive before the desktop switch notification.
        if self.desktops.owner(id) != Some(self.desktops.current_id())
            && let Some(live) = vd::current_desktop()
        {
            self.switch_desktop(live);
        }
        let effects = self.desktops.with_owner(id, |s| s.window_focused(id));
        self.apply(effects);
    }

    pub(super) fn on_event(&mut self, event: WinEvent) {
        if self.paused || !self.tiling {
            return;
        }
        match event {
            WinEvent::Show(raw) | WinEvent::Uncloaked(raw) => {
                let id = WindowId(raw);
                if !self.desktops.is_managed(id) {
                    self.try_manage(raw);
                } else if !consume(&mut self.pending_show, id)
                    && self.desktops.is_hidden(id)
                    && self.desktops.owner(id) == Some(self.desktops.current_id())
                {
                    // The app re-showed a window we hid: follow it to its workspace.
                    let effects = self.desktops.with_owner(id, |s| s.window_focused(id));
                    self.apply(effects);
                }
            }
            WinEvent::Hide(raw) | WinEvent::Cloaked(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id)
                    && !consume(&mut self.pending_hide, id)
                    && !self.windows.is_visible(id)
                {
                    // The shell cloaks windows on other desktops: keep managing those.
                    if on_other_desktop(raw) {
                        self.check_desktop(id);
                    } else {
                        self.unmanage(id);
                    }
                }
            }
            WinEvent::Destroy(raw) => {
                let id = WindowId(raw);
                self.ignored.remove(&id);
                if self.desktops.is_managed(id) {
                    self.unmanage(id);
                }
            }
            WinEvent::Foreground(raw) => self.on_foreground(raw),
            WinEvent::MinimizeStart(raw) => {
                let id = WindowId(raw);
                let effects = self.desktops.with_owner(id, |s| s.window_minimized(id));
                self.apply(effects);
            }
            WinEvent::MinimizeEnd(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id) {
                    let effects = self.desktops.with_owner(id, |s| s.window_restored(id));
                    self.apply(effects);
                } else {
                    self.try_manage(raw);
                }
            }
            WinEvent::MoveSizeStart(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id) {
                    self.moving.insert(id);
                    self.resizing.remove(&id);
                    self.tweens.remove(&id);
                }
            }
            WinEvent::MoveSizeEnd(raw) => {
                let id = WindowId(raw);
                if self.moving.remove(&id) {
                    self.placed.remove(&id);
                    let resized = self.resizing.remove(&id);
                    if let Some(rect) = self.windows.frame(id) {
                        let cursor = cursor_pos();
                        let effects = self.desktops.with_owner(id, |s| {
                            if resized {
                                s.window_resized(id, rect)
                            } else {
                                s.window_moved(id, rect, cursor)
                            }
                        });
                        self.apply(effects);
                    }
                }
            }
            WinEvent::NameChange(raw) => {
                let id = WindowId(raw);
                if self.desktops.is_managed(id) {
                    if self.focused() == Some(id)
                        && let Some(info) = self.windows.info(id)
                    {
                        self.desktops.current().set_info(id, info);
                        self.publish();
                    }
                } else if foreground() == Some(raw) {
                    // Some apps only set a title after showing their window.
                    self.try_manage(raw);
                }
            }
            WinEvent::LocationChange(raw) => {
                let id = WindowId(raw);
                if self.moving.contains(&id) {
                    self.follow_resize(id);
                } else {
                    self.enforce_size(id);
                }
            }
        }
    }

    /// While an edge of a tiled window is dragged, fits the other windows around it as it goes,
    /// without animating them, so they keep up with the pointer.
    pub(super) fn follow_resize(&mut self, id: WindowId) {
        let Some(rect) = self.windows.frame(id) else {
            return;
        };
        let effects = self
            .desktops
            .with_owner(id, |s| s.window_resizing(id, rect));
        if effects.is_empty() {
            return;
        }
        self.resizing.insert(id);
        for effect in effects {
            if let Effect::Place { id: other, rect } = effect {
                // The window being dragged is skipped by `place`: the user holds it.
                if let Err(gone) = self.place(other, rect, false) {
                    self.ignored.insert(gone);
                    self.unmanage(gone);
                }
            }
        }
        self.update_borders();
    }

    /// Some apps enforce a minimum size larger than their tile. Re-place those with force so
    /// they fit; if one still refuses, at least slide it back inside the work area.
    pub(super) fn enforce_size(&mut self, id: WindowId) {
        if self.moving.contains(&id)
            || self.tweens.contains_key(&id)
            || self.desktops.is_hidden(id)
            || self.desktops.is_floating(id)
        {
            return;
        }
        let Some(target) = self.placed.get(&id).copied() else {
            return;
        };
        let Some(frame) = self.windows.frame(id) else {
            return;
        };
        // Placement is asynchronous: only a window already at its tile's position but larger
        // than the tile has refused the size; anything else may simply not have moved yet.
        let at_target = (frame.x - target.x).abs() <= 2 && (frame.y - target.y).abs() <= 2;
        let refused = frame.w > target.w + 2 || frame.h > target.h + 2;
        if !at_target || !refused {
            return;
        }
        match self.forced.get(&id) {
            None => {
                tracing::debug!(
                    "{id:?} wants at least {}x{}; forcing its tile size",
                    frame.w,
                    frame.h
                );
                self.forced.insert(id, Instant::now());
                if self.place_now(id, target, true).is_err() {
                    self.unmanage(id);
                }
                return;
            }
            // Give the forced placement time to land before judging it.
            Some(at) if at.elapsed() < FORCE_GRACE => return,
            Some(_) => {}
        }
        let (cx, cy) = target.center();
        let Some(area) = self
            .monitors
            .iter()
            .find(|m| m.rect.contains_point(cx, cy))
            .map(|m| m.work_area)
        else {
            return;
        };
        if frame.right() <= area.right() && frame.bottom() <= area.bottom() {
            return;
        }
        let x = target.x.min(area.right() - frame.w).max(area.x);
        let y = target.y.min(area.bottom() - frame.h).max(area.y);
        tracing::debug!(
            "{id:?} resized itself to {}x{} even when forced; keeping it on screen",
            frame.w,
            frame.h
        );
        if self
            .place_now(id, Rect::new(x, y, frame.w, frame.h), true)
            .is_err()
        {
            self.unmanage(id);
        }
    }

    pub(super) fn on_tray(&mut self, event: TrayEvent) {
        match event {
            TrayEvent::ToggleTile => self.set_paused(!self.paused),
            TrayEvent::ToggleRemap => self.set_remap_paused(!self.remap_paused),
            TrayEvent::ToggleShell => self.toggle_shell(),
            TrayEvent::ToggleLogging => self.toggle_logging(),
            // If the config file couldn't be created, open its folder instead.
            TrayEvent::OpenConfig => {
                if let Err(e) = tile_config::create_if_missing(&self.config_path) {
                    tracing::warn!("could not create {}: {e}", self.config_path.display());
                }
                launch::open(&self.config_path);
            }
            TrayEvent::OpenLogs => {
                // Nothing may have been logged yet.
                let _ = std::fs::create_dir_all(paths::log_dir());
                launch::open(&paths::log_dir());
            }
            TrayEvent::Quit => self.quit = true,
            TrayEvent::DisplayChanged => {
                if let Some(backdrop) = &self.backdrop {
                    backdrop.refresh();
                }
                if self.tiling {
                    self.refresh_monitors();
                }
            }
            TrayEvent::SessionChanged => {
                if let Some(remap) = &self.remap {
                    remap.send(remap::Command::Reset);
                }
                self.show_cursor();
            }
        }
    }

    pub(super) fn on_ipc(&mut self, incoming: Incoming) {
        let Incoming { request, reply } = incoming;
        let response = match request {
            Request::Dispatch { command } => match command.parse::<Command>() {
                Ok(command) => {
                    self.run_command(command);
                    Response::Ok
                }
                Err(e) => Response::Error {
                    message: e.to_string(),
                },
            },
            Request::Query { what } => {
                let state = self.desktops.current_ref();
                match what {
                    Query::Monitors => Response::Monitors {
                        monitors: state.snapshot_monitors(),
                    },
                    Query::Workspaces => Response::Workspaces {
                        workspaces: state.snapshot_workspaces(),
                    },
                    Query::Windows => Response::Windows {
                        windows: state.snapshot_windows(),
                    },
                    Query::ActiveWindow => Response::ActiveWindow {
                        window: state.snapshot_active_window(),
                    },
                }
            }
            Request::Subscribe => {
                for event in self.current_events() {
                    let _ = reply.send(Response::Event { event });
                }
                self.subscribers.push(reply);
                return;
            }
        };
        let _ = reply.send(response);
    }
}
