//! Applying the core state's effects to real windows, borders and published state.

use std::{collections::HashSet, time::Duration};
use tile_core::{Effect, Rect, WindowId};
use tile_ipc::{Event, Response};
use tile_platform::{Animation, Mover, PlaceError, focus_desktop, foreground};
use tokio::time::Instant;

use super::{Daemon, HEAL_DELAY, Published, tracked::Tween};
use crate::{
    launch,
    paths::{self, Restore},
};

impl Daemon {
    /// Applies effects from the core state to the real windows.
    pub(super) fn apply(&mut self, effects: Vec<Effect>) {
        let shown: HashSet<WindowId> = effects
            .iter()
            .filter_map(|e| {
                if let Effect::Show(id) = e {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        // Before the old window is hidden: hiding the focused window makes Windows pick another
        // one, possibly on another monitor, and we would follow it.
        if effects.contains(&Effect::Defocus) {
            tracing::info!("nothing to focus here: focusing the desktop");
            focus_desktop();
            self.defocused_at = Some(Instant::now());
        }
        let mut failed = Vec::new();
        let mut focus_refused = false;
        for effect in effects {
            match effect {
                Effect::Place { id, rect } => {
                    if let Err(id) = self.place(id, rect, !shown.contains(&id)) {
                        failed.push(id);
                    }
                }
                Effect::Show(id) => {
                    self.track(id).pending_show.bump();
                    self.windows.show(id);
                }
                Effect::Hide(id) => {
                    let tracked = self.track(id);
                    tracked.pending_hide.bump();
                    tracked.tween = None;
                    self.windows.hide(id);
                }
                Effect::Focus(id) => {
                    if !self.windows.focus(id) {
                        tracing::debug!("Windows refused to focus {id:?}");
                        focus_refused = true;
                    }
                }
                Effect::Defocus => {}
                Effect::Close(id) => self.windows.close(id),
                Effect::Exec(line) => launch::exec(&line),
            }
        }
        for id in failed {
            self.ignored.insert(id);
            self.unmanage(id);
        }
        self.update_decorations();
        self.update_borders();
        self.persist_restore();
        self.publish();
        // Keep our notion of focus (and the highlight) truthful when Windows said no.
        if focus_refused && !self.resyncing_focus {
            self.resyncing_focus = true;
            if let Some(id) = foreground() {
                self.on_foreground(id);
            }
            self.resyncing_focus = false;
        }
    }

    pub(super) fn wants_decorations(&self, id: WindowId) -> bool {
        let rule = self.tracked.get(&id).and_then(|t| t.decoration_rule);
        rule.unwrap_or_else(|| {
            self.loaded.config.general.decorations || self.desktops.is_floating(id)
        })
    }

    /// Adds or removes title bars so every managed window matches the config.
    pub(super) fn update_decorations(&mut self) {
        for id in self.desktops.managed() {
            let want = self.wants_decorations(id);
            match (want, self.tracked.get(&id).and_then(|t| t.undecorated)) {
                (false, None) => {
                    let changed = self.windows.set_decorated(id, false);
                    self.track(id).undecorated = Some(changed);
                }
                (true, Some(changed)) => {
                    if changed {
                        self.windows.set_decorated(id, true);
                    }
                    self.track(id).undecorated = None;
                }
                _ => {}
            }
        }
    }

    pub(super) fn place(
        &mut self,
        id: WindowId,
        rect: Rect,
        may_animate: bool,
    ) -> Result<(), WindowId> {
        if self.is_moving(id) {
            return Ok(());
        }
        let previous = self.track(id).placed.replace(rect);
        if previous == Some(rect) {
            return Ok(());
        }
        let animations = &self.loaded.config.animations;
        if animations.enabled
            && may_animate
            && !self.desktops.is_hidden(id)
            && self.desktops.is_current(id)
            && let Some(previous) = previous
        {
            let now = std::time::Instant::now();
            // Retargeting continues from wherever the running animation is right now.
            let existing = self
                .track(id)
                .tween
                .take()
                .map(|t| (t.animation.at(now).0, t.mover));
            if let Some((from, mover)) =
                existing.or_else(|| Mover::spawn(id).map(|m| (previous, m)))
                && let Some(insets) = self.windows.insets_of(id)
            {
                let config = &self.loaded.config.animations;
                let animation = Animation {
                    from,
                    to: rect,
                    start: now,
                    duration: Duration::from_millis(config.duration_ms.max(1)),
                    curve: config.curve(),
                    insets,
                    force: self.is_forced(id),
                };
                mover.animate(animation);
                self.track(id).tween = Some(Tween { animation, mover });
                return Ok(());
            }
        }
        self.track(id).tween = None;
        self.place_now(id, rect, true)
    }

    /// `settled`: the final placement of a move (not an animation frame), which repaints.
    pub(super) fn place_now(
        &mut self,
        id: WindowId,
        rect: Rect,
        settled: bool,
    ) -> Result<(), WindowId> {
        match self.windows.place(id, rect, self.is_forced(id), settled) {
            Ok(()) => Ok(()),
            Err(PlaceError::Gone) => Err(id),
            Err(PlaceError::AccessDenied) => {
                tracing::warn!("cannot control {id:?} (elevated?), ignoring it");
                Err(id)
            }
            Err(PlaceError::Other(e)) => {
                tracing::debug!("placing {id:?} failed: {e}");
                Ok(())
            }
        }
    }

    /// Drops windows that disappeared without us noticing.
    pub(super) fn sweep(&mut self) {
        self.sync_shell();
        if !self.tiling {
            return;
        }
        let dead: Vec<_> = self
            .desktops
            .managed()
            .into_iter()
            .filter(|id| !self.windows.is_alive(*id))
            .collect();
        for id in dead {
            self.unmanage(id);
        }
        for id in self.desktops.managed() {
            if self.desktops.is_current(id) {
                self.enforce_size(id);
            }
        }
        self.retry_lock_override();
        self.heal_borders();
    }

    pub(super) fn update_borders(&mut self) {
        let general = &self.loaded.config.general;
        if !general.borders {
            return;
        }
        let focused = self.desktops.current_ref().focused();
        if focused == self.bordered {
            return;
        }
        if let Some(old) = self.bordered {
            self.windows
                .set_border(old, Some(self.loaded.config.border_inactive()));
        }
        if let Some(new) = focused {
            self.windows
                .set_border(new, Some(self.loaded.config.border_active()));
        }
        self.bordered = focused;
        self.heal_at = Some(Instant::now() + HEAL_DELAY);
    }

    /// Re-applies border colors that apps overwrote (Chromium/Electron repaint their frame
    /// on activation and theme changes).
    pub(super) fn heal_borders(&mut self) {
        let general = &self.loaded.config.general;
        if !general.borders {
            return;
        }
        let focused = self.desktops.current_ref().focused();
        for id in self.desktops.managed() {
            if !self.desktops.is_current(id) || self.desktops.is_hidden(id) {
                continue;
            }
            let want = Some(if Some(id) == focused {
                self.loaded.config.border_active()
            } else {
                self.loaded.config.border_inactive()
            });
            // DWM doesn't report border colors on current Windows builds (E_INVALIDARG), so
            // when it can't say, just re-assert ours: a no-op when unchanged.
            if self.windows.border_is(id, want) != Some(true) {
                self.windows.set_border(id, want);
            }
        }
    }

    pub(super) fn repaint_borders(&mut self) {
        let general = &self.loaded.config.general;
        let color = general
            .borders
            .then_some(self.loaded.config.border_inactive());
        for id in self.desktops.managed() {
            self.windows.set_border(id, color);
        }
        self.bordered = None;
    }

    /// Records what must be undone if we die, for the panic hook and the watchdog.
    pub(super) fn persist_restore(&mut self) {
        let mut restore = Restore {
            hidden: self.desktops.hidden().into_iter().map(|id| id.0).collect(),
            undecorated: self
                .tracked
                .iter()
                .filter(|(_, t)| t.undecorated == Some(true))
                .map(|(id, _)| id.0)
                .collect(),
            lock_disabled: self.lock_taken,
            cursor_hidden: self.cursor_hidden.is_some(),
        };
        restore.hidden.sort_unstable();
        restore.undecorated.sort_unstable();
        if restore != self.saved_restore {
            paths::save_restore(&restore);
            self.saved_restore = restore;
        }
    }

    pub(super) fn snapshot(&self) -> Published {
        let state = self.desktops.current_ref();
        Published {
            workspace: state.focused_workspace(),
            window: state
                .snapshot_active_window()
                .map(|w| (w.id, w.title, w.exe)),
            layout: state.active_layout(),
        }
    }

    pub(super) fn current_events(&self) -> Vec<Event> {
        let now = self.snapshot();
        let mut events = Vec::new();
        events.extend(now.workspace.map(|id| Event::Workspace { id }));
        events.push(match now.window {
            Some((id, title, exe)) => Event::ActiveWindow {
                id: Some(id),
                title: Some(title),
                exe: Some(exe),
            },
            None => Event::ActiveWindow {
                id: None,
                title: None,
                exe: None,
            },
        });
        events.extend(now.layout.map(|layout| Event::Layout { layout }));
        events
    }

    /// Notifies IPC subscribers about changes to the workspace, active window and layout.
    pub(super) fn publish(&mut self) {
        let now = self.snapshot();
        if now == self.published {
            return;
        }
        let mut events = Vec::new();
        if now.workspace != self.published.workspace
            && let Some(id) = now.workspace
        {
            events.push(Event::Workspace { id });
        }
        if now.window != self.published.window {
            events.push(match &now.window {
                Some((id, title, exe)) => Event::ActiveWindow {
                    id: Some(*id),
                    title: Some(title.clone()),
                    exe: Some(exe.clone()),
                },
                None => Event::ActiveWindow {
                    id: None,
                    title: None,
                    exe: None,
                },
            });
        }
        if now.layout != self.published.layout
            && let Some(layout) = now.layout
        {
            events.push(Event::Layout { layout });
        }
        self.published = now;
        self.subscribers.retain(|tx| {
            events.iter().all(|event| {
                tx.send(Response::Event {
                    event: event.clone(),
                })
                .is_ok()
            })
        });
    }
}
