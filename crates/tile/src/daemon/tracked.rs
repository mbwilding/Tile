//! What the daemon keeps about each managed window, beside the core state.

use tile_core::Rect;
use tile_platform::{Animation, Mover};
use tokio::time::Instant;

use super::SELF_EVENT_WINDOW;

/// A window being animated. The mover draws the frames on its own thread; the daemon only
/// keeps the animation to know where the window is and when it ends.
pub(super) struct Tween {
    pub(super) animation: Animation,
    pub(super) mover: Mover,
}

/// Show or hide events we caused ourselves and still expect to arrive.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Expected {
    count: u32,
    /// When the last one was caused.
    at: Option<Instant>,
}

impl Expected {
    /// Notes that we caused one more.
    pub(super) fn bump(&mut self) {
        self.count += 1;
        self.at = Some(Instant::now());
    }

    /// Consumes one expected event, if there is a recent one. A stale expectation is dropped.
    pub(super) fn consume(&mut self) -> bool {
        let fresh = self.count > 0 && self.at.is_some_and(|at| at.elapsed() < SELF_EVENT_WINDOW);
        if fresh {
            self.count -= 1;
        } else {
            *self = Self::default();
        }
        fresh
    }
}

/// The daemon's own state for one window. Forgetting the window drops all of it at once.
#[derive(Default)]
pub(super) struct Tracked {
    /// The last rect we placed it at (or are animating it towards).
    pub(super) placed: Option<Rect>,
    /// Its animation, while one runs.
    pub(super) tween: Option<Tween>,
    /// The user is dragging or resizing it with the mouse.
    pub(super) moving: bool,
    /// While moving: its edge has been dragged, so the others have been following it.
    pub(super) resizing: bool,
    /// Shows we caused, whose events are still to come.
    pub(super) pending_show: Expected,
    /// Hides we caused, whose events are still to come.
    pub(super) pending_hide: Expected,
    /// It refused its tile size (an app minimum size), so it is placed with force from now on:
    /// when force was last applied.
    pub(super) forced: Option<Instant>,
    /// A rule's `decorations` override.
    pub(super) decoration_rule: Option<bool>,
    /// Handled as undecorated: `true` if we actually removed a title bar.
    pub(super) undecorated: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_events_are_consumed_once_each() {
        let mut expected = Expected::default();
        assert!(!expected.consume());
        expected.bump();
        expected.bump();
        assert!(expected.consume());
        assert!(expected.consume());
        assert!(!expected.consume());
    }
}
