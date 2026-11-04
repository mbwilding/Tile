//! Applying the core state's effects to real windows, borders and published state.

use std::{collections::HashSet, time::Duration};
use tile_core::{Effect, Rect, WindowId};
use tile_ipc::{Event, Response};
use tile_platform::{Animation, Mover, PlaceError, Ring, focus_desktop, foreground};
use tokio::time::Instant;

use super::{ActiveWindow, Daemon, HEAL_DELAY, Published, tracked::Tween};
use crate::{
    launch,
    paths::{self, Restore},
};

/// How a window gets to its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Motion {
    /// At once.
    Jump,
    /// Sliding from where it is, if animations are on.
    Animate,
    /// Sliding in from the edge of its monitor, as a scrolled window coming into view does.
    SlideIn,
}

/// A clip no window is bigger than, which lifts the cut a window was given.
const UNCLIPPED: Rect = Rect::new(-100_000, -100_000, 200_000, 200_000);

/// `rect` moved sideways to just past the right (or left) edge of `clip`.
fn off_edge(rect: Rect, clip: Rect, right: bool) -> Rect {
    let x = if right { clip.right() } else { clip.x - rect.w };
    Rect::new(x, rect.y, rect.w, rect.h)
}

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
                Effect::Place { id, rect, clip } => {
                    let motion = if shown.contains(&id) {
                        Motion::SlideIn
                    } else {
                        Motion::Animate
                    };
                    if let Err(id) = self.place(id, rect, clip, motion) {
                        failed.push(id);
                    }
                }
                Effect::Show(id) => {
                    self.track(id).pending_show.bump();
                    // Under the backdrop before it appears, and shown in the same step: showing
                    // raises a window to the top.
                    self.stack_scrolling();
                    match self.stack_anchor.filter(|_| self.stacked.contains(&id)) {
                        Some(anchor) => self.windows.show_under(id, anchor),
                        None => self.windows.show(id),
                    }
                }
                Effect::Hide(id) => {
                    let tracked = self.track(id);
                    tracked.pending_hide.bump();
                    tracked.tween = None;
                    self.windows.hide(id);
                }
                Effect::Park(id) => {
                    if !self.slide_out(id) {
                        let tracked = self.track(id);
                        tracked.pending_hide.bump();
                        tracked.tween = None;
                        self.windows.hide(id);
                    }
                }
                Effect::Focus(id) => {
                    if !self.windows.focus(id) {
                        tracing::debug!("Windows refused to focus {id:?}");
                        focus_refused = true;
                    }
                    // Focus raises the window above the backdrops.
                    self.stack_scrolling();
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
        self.stack_scrolling();
        self.sync_rings();
        self.persist_restore();
        self.publish();
        // Keep our notion of focus (and the highlight) truthful when Windows said no.
        if focus_refused && !self.resyncing_focus {
            self.resyncing_focus = true;
            // A window we are hiding may still be in front, and isn't where focus went.
            let leaving = |d: &Self, id| {
                d.tracked
                    .get(&id)
                    .is_some_and(|t| t.pending_hide.is_pending())
            };
            if let Some(id) = foreground().filter(|id| !leaving(self, *id)) {
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
        clip: Option<Rect>,
        motion: Motion,
    ) -> Result<(), WindowId> {
        if self.is_moving(id) {
            return Ok(());
        }
        let tracked = self.track(id);
        // A cut that is no longer wanted is lifted by clipping to the window itself.
        let clip_now = clip.or_else(|| tracked.clipped.then_some(UNCLIPPED));
        tracked.clipped = clip.is_some();
        let previous = tracked.placed.replace(rect);
        let previous_clip = std::mem::replace(&mut tracked.clip, clip_now);
        if previous == Some(rect) && previous_clip == clip_now {
            return Ok(());
        }
        let mut previous = previous;
        let animating = self.loaded.config.animations.enabled
            && motion != Motion::Jump
            && !self.desktops.is_hidden(id)
            && self.desktops.is_current(id);
        if animating
            && motion == Motion::SlideIn
            && self.track(id).tween.is_none()
            && let Some(clip) = clip
        {
            // Start just past the monitor's edge, where the cut hides it.
            let start = off_edge(rect, clip, rect.center().0 >= clip.center().0);
            self.place_now(id, start, false)?;
            previous = Some(start);
        }
        if animating
            && let Some(previous) = previous
            && self.animate(id, previous, rect, clip_now, false)
        {
            return Ok(());
        }
        self.track(id).tween = None;
        self.place_now(id, rect, true)
    }

    /// Slides `id` from `from` to `to` on its mover, continuing from wherever a running
    /// animation is. `hide` hides the window on arrival. `false` if it can't be animated.
    fn animate(
        &mut self,
        id: WindowId,
        from: Rect,
        to: Rect,
        clip: Option<Rect>,
        hide: bool,
    ) -> bool {
        // So the mover knows what to stay under.
        self.stack_scrolling();
        let now = std::time::Instant::now();
        let existing = self
            .track(id)
            .tween
            .take()
            .map(|t| (t.animation.at(now).0, t.mover));
        let Some((from, mover)) = existing.or_else(|| Mover::spawn(id).map(|m| (from, m))) else {
            return false;
        };
        let Some(insets) = self.windows.insets_of(id) else {
            return false;
        };
        let config = &self.loaded.config.animations;
        let animation = Animation {
            from,
            to,
            start: now,
            duration: Duration::from_millis(config.duration_ms.max(1)),
            curve: config.curve(),
            insets,
            force: self.is_forced(id),
            clip,
            hide_when_done: hide,
            under: self.stack_anchor,
        };
        mover.animate(animation);
        self.track(id).tween = Some(Tween { animation, mover });
        true
    }

    /// Slides a window that scrolled out of view off the edge of its monitor, and hides it
    /// there. `false` if it can't, and should be hidden at once.
    fn slide_out(&mut self, id: WindowId) -> bool {
        if !self.loaded.config.animations.enabled
            || !self.desktops.is_current(id)
            || !self.windows.is_visible(id)
        {
            return false;
        }
        let Some(tracked) = self.tracked.get(&id) else {
            return false;
        };
        let (Some(placed), Some(clip), true) = (tracked.placed, tracked.clip, tracked.clipped)
        else {
            return false;
        };
        let target = off_edge(placed, clip, placed.center().0 >= clip.center().0);
        if !self.animate(id, placed, target, Some(clip), true) {
            return false;
        }
        let tracked = self.track(id);
        tracked.placed = Some(target);
        tracked.pending_hide.bump();
        true
    }

    /// `settled`: the final placement of a move (not an animation frame), which repaints.
    pub(super) fn place_now(
        &mut self,
        id: WindowId,
        rect: Rect,
        settled: bool,
    ) -> Result<(), WindowId> {
        let clip = self.track(id).clip;
        match self
            .windows
            .place(id, rect, self.is_forced(id), settled, clip)
        {
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
        self.retry_smartscreen_override();
        if !self.tiling {
            return;
        }
        self.sync_rings();
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
            self.paint_border(old, Some(self.loaded.config.border_inactive()));
        }
        if let Some(new) = focused {
            self.paint_border(new, Some(self.loaded.config.border_active()));
        }
        self.bordered = focused;
        self.sync_rings();
        self.heal_at = Some(Instant::now() + HEAL_DELAY);
    }

    /// Draws the border rings round the tiled windows on show, or clears them when the border is
    /// Windows' own one pixel one. The first pixel is Windows' own, so a ring is one less wide,
    /// except on a window cut to its monitor, whose own border is taken away.
    pub(super) fn sync_rings(&mut self) {
        let config = &self.loaded.config;
        let border_width = config.general.border_width;
        if !config.general.borders || border_width == 0 || !self.tiling {
            self.rings.set(Vec::new());
            return;
        }
        let focused = self.desktops.current_ref().focused();
        let rings = self
            .desktops
            .managed()
            .into_iter()
            .filter(|id| self.desktops.is_current(*id) && !self.desktops.is_hidden(*id))
            .filter_map(|id| {
                let frame = self.windows.frame_rect(id)?;
                let cut = self.tracked.get(&id).is_some_and(|t| t.clipped);
                let width = if cut {
                    border_width
                } else {
                    border_width.saturating_sub(1)
                };
                if width == 0 {
                    return None;
                }
                let color = if Some(id) == focused {
                    config.border_active()
                } else {
                    config.border_inactive()
                };
                Some(Ring {
                    key: id.0,
                    frame,
                    color,
                    width,
                    clip: self.ring_clip(id, frame),
                })
            })
            .collect();
        self.rings.set(rings);
    }

    /// With Tile's backdrop on the desktop, hides what a scrolling window overflows onto the
    /// other monitors behind their backdrops: its monitor's backdrop goes to the very bottom
    /// and the window directly beneath the lowest of the others, so it is over its own
    /// backdrop and under theirs. That works while one monitor scrolls: with two, each
    /// would have to be under the other's backdrop, and the cut to the monitor is all there is.
    pub(super) fn stack_scrolling(&mut self) {
        let active = self.scrolling_to_stack();
        let stacked: HashSet<WindowId> = active.iter().map(|(id, _)| *id).collect();
        if stacked != self.stacked {
            tracing::debug!("stacking under the other backdrops: {stacked:?}");
        }
        // Windows that stopped scrolling would otherwise stay under a backdrop.
        for id in self.stacked.difference(&stacked) {
            self.windows.raise(*id);
        }
        self.stack_anchor = None;
        if let (Some(backdrop), Some(&(_, own))) = (&self.backdrop, active.first()) {
            let handles = backdrop.handles();
            tile_platform::send_to_bottom(handles[own]);
            let others: Vec<isize> = handles
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != own)
                .map(|(_, h)| *h)
                .collect();
            self.stack_anchor = tile_platform::lowest_of(&others);
            if let Some(anchor) = self.stack_anchor {
                for (id, _) in &active {
                    self.windows.stack_below(*id, anchor);
                }
            }
        }
        self.stacked = stacked;
    }

    /// The windows `stack_scrolling` puts under the other monitors' backdrops, with their
    /// monitor: none unless the backdrop is up and exactly one monitor scrolls.
    fn scrolling_to_stack(&self) -> Vec<(WindowId, usize)> {
        let Some(backdrop) = &self.backdrop else {
            return Vec::new();
        };
        if backdrop.handles().len() != self.monitors.len() || self.monitors.len() < 2 {
            return Vec::new();
        }
        let scrolling: Vec<(WindowId, usize)> = self
            .desktops
            .managed()
            .into_iter()
            // Hidden ones too: a column scrolled out of view must still be under the backdrop
            // when it comes back.
            .filter(|id| self.desktops.is_current(*id))
            .filter_map(|id| {
                let tracked = self.tracked.get(&id).filter(|t| t.clipped)?;
                let clip = tracked.clip?;
                let monitor = self.monitors.iter().position(|m| m.work_area == clip)?;
                Some((id, monitor))
            })
            .collect();
        match scrolling.first() {
            Some(&(_, own)) if scrolling.iter().all(|(_, m)| *m == own) => scrolling,
            _ => Vec::new(),
        }
    }

    /// The part of the screen a window's ring may show in: its monitor, so the ring of a
    /// fullscreen window doesn't spill onto the next one. Floating windows may straddle
    /// monitors and are left alone.
    fn ring_clip(&self, id: WindowId, frame: Rect) -> Option<Rect> {
        let tracked = self.tracked.get(&id);
        if tracked.is_some_and(|t| t.clipped) {
            return tracked.and_then(|t| t.clip);
        }
        if self.desktops.is_floating(id) {
            return None;
        }
        let (x, y) = frame.center();
        self.monitors
            .iter()
            .find(|m| m.rect.contains_point(x, y))
            .map(|m| m.rect)
    }

    /// Re-applies border colors that apps overwrote (Chromium/Electron repaint their frame
    /// on activation and theme changes).
    pub(super) fn heal_borders(&mut self) {
        self.sync_rings();
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
                self.paint_border(id, want);
            }
        }
    }

    pub(super) fn repaint_borders(&mut self) {
        let general = &self.loaded.config.general;
        let color = general
            .borders
            .then_some(self.loaded.config.border_inactive());
        for id in self.desktops.managed() {
            self.paint_border(id, color);
        }
        self.bordered = None;
    }

    /// Sets a window's own border, or takes it away for a window that is cut to its monitor:
    /// Tile's ring is drawn instead, which can be cut too.
    pub(super) fn paint_border(&self, id: WindowId, color: Option<tile_config::Color>) {
        if color.is_some() && self.tracked.get(&id).is_some_and(|t| t.clipped) {
            self.windows.hide_border(id);
        } else {
            self.windows.set_border(id, color);
        }
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
            clipped: self
                .tracked
                .iter()
                .filter(|(_, t)| t.clipped)
                .map(|(id, _)| id.0)
                .collect(),
            lock_disabled: self.win_l.taken,
            cursor_hidden: self.cursor_hidden.is_some(),
            smartscreen_disabled: self.smartscreen.taken,
        };
        restore.hidden.sort_unstable();
        restore.undecorated.sort_unstable();
        restore.clipped.sort_unstable();
        if restore != self.saved_restore {
            paths::save_restore(&restore);
            self.saved_restore = restore;
        }
    }

    pub(super) fn snapshot(&self) -> Published {
        let state = self.desktops.current_ref();
        Published {
            workspace: state.focused_workspace(),
            window: state.snapshot_active_window().map(|w| ActiveWindow {
                id: w.id,
                title: w.title,
                exe: w.exe,
            }),
            layout: state.active_layout(),
        }
    }

    pub(super) fn current_events(&self) -> Vec<Event> {
        let now = self.snapshot();
        let mut events = Vec::new();
        events.extend(now.workspace.map(|id| Event::Workspace { id }));
        events.push(ActiveWindow::event(now.window.as_ref()));
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
            events.push(ActiveWindow::event(now.window.as_ref()));
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
