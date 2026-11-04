//! Placing a window by its visible frame, shared by [`Windows::place`](crate::Windows::place)
//! and the movers' animation frames.

use tile_core::Rect;
use winsafe::{self as w, co, prelude::Handle as _};

/// Space between a window's outer rect and its visible frame (shadows / resize borders).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Insets {
    /// The left border, in pixels.
    pub left: i32,
    /// The top border, in pixels.
    pub top: i32,
    /// The right border, in pixels.
    pub right: i32,
    /// The bottom border, in pixels.
    pub bottom: i32,
}

/// How [`set_frame`] places a window.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Placement {
    /// Skip `WM_WINDOWPOSCHANGING`, which is where Windows applies an app's minimum size, so
    /// windows can be shrunk below it.
    pub force: bool,
    /// The last placement of a move (not an animation frame): the frame is recomputed and the
    /// whole window repainted.
    pub settled: bool,
    /// Wait until the app has processed the move and painted it, as the movers do for each
    /// frame; otherwise the move is posted to the window's thread.
    pub sync: bool,
    /// The part of the screen the window may show in. The rest is cut off, so a window part way
    /// off its monitor doesn't appear on the next one. A clip the whole window fits in removes
    /// the cut. `None` leaves the window as it is.
    pub clip: Option<Rect>,
}

/// What the cut was last made as: the visible part of the window, or `None` for no cut. A
/// mover keeps it across the frames of a move so an unchanged cut isn't made again.
pub(crate) type Cut = Option<Option<[i32; 4]>>;

/// Moves `hwnd` so its visible frame covers `visible`, given its invisible `insets`. `cut` is
/// what the last call made of the window's cut (`&mut None` to always make it).
pub(crate) fn set_frame(
    hwnd: &w::HWND,
    visible: Rect,
    insets: Insets,
    how: Placement,
    cut: &mut Cut,
) -> w::SysResult<()> {
    let pos = w::POINT::with(visible.x - insets.left, visible.y - insets.top);
    let size = w::SIZE::with(
        visible.w + insets.left + insets.right,
        visible.h + insets.top + insets.bottom,
    );
    let mut flags = co::SWP::NOZORDER | co::SWP::NOACTIVATE | co::SWP::NOOWNERZORDER;
    // A cut is relative to the window's position, so the move must land before it is made.
    if !how.sync && how.clip.is_none() {
        flags |= co::SWP::ASYNCWINDOWPOS;
    }
    if how.force {
        flags |= co::SWP::NOSENDCHANGING;
    }
    let resizing = hwnd
        .GetWindowRect()
        .is_ok_and(|r| (r.right - r.left, r.bottom - r.top) != (size.cx, size.cy));
    if resizing || how.settled {
        // Discard the old pixels so nothing stale shows in newly uncovered space.
        flags |= co::SWP::NOCOPYBITS;
    }
    if how.settled {
        flags |= co::SWP::FRAMECHANGED;
    }
    let (cx, cy) = visible.center();
    let destination =
        w::HMONITOR::MonitorFromPoint(w::POINT::with(cx, cy), co::MONITOR::DEFAULTTONEAREST);
    let crossing = hwnd.MonitorFromWindow(co::MONITOR::DEFAULTTONEAREST) != destination;
    hwnd.SetWindowPos(w::HwndPlace::None, pos, size, flags)?;
    if crossing {
        // Crossing monitors with different DPI makes apps rescale themselves; apply twice.
        hwnd.SetWindowPos(w::HwndPlace::None, pos, size, flags)?;
    }
    if let Some(clip) = how.clip {
        let outer = Rect::new(pos.x, pos.y, size.cx, size.cy);
        let part = visible_part(outer, clip);
        if *cut != Some(part) || how.settled {
            tile_sys::set_window_clip(hwnd.ptr() as isize, part);
            *cut = Some(part);
        }
    }
    // A move that keeps the size needs no repaint: the compositor just moves the window, and
    // forcing one every frame is what makes a window stutter at a high refresh rate.
    if how.settled || (how.sync && resizing) {
        let everything = w::RECT {
            left: 0,
            top: 0,
            right: i32::from(i16::MAX),
            bottom: i32::from(i16::MAX),
        };
        let mut redraw = co::RDW::INVALIDATE | co::RDW::ERASE | co::RDW::ALLCHILDREN;
        if how.settled {
            redraw |= co::RDW::FRAME;
        }
        if how.sync {
            // Returns once the app and its children have handled WM_PAINT.
            redraw |= co::RDW::UPDATENOW;
        }
        let _ = hwnd.RedrawWindow(everything, &w::HRGN::NULL, redraw);
    }
    Ok(())
}

/// The part of `outer` inside `clip`, in `outer`'s own coordinates as left, top, right, bottom.
/// `None` when all of it is inside; an empty rectangle when none of it is.
pub(crate) fn visible_part(outer: Rect, clip: Rect) -> Option<[i32; 4]> {
    let left = outer.x.max(clip.x);
    let top = outer.y.max(clip.y);
    let right = outer.right().min(clip.right());
    let bottom = outer.bottom().min(clip.bottom());
    if (left, top, right, bottom) == (outer.x, outer.y, outer.right(), outer.bottom()) {
        return None;
    }
    if right <= left || bottom <= top {
        return Some([0; 4]);
    }
    Some([
        left - outer.x,
        top - outer.y,
        right - outer.x,
        bottom - outer.y,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect {
        x: 0,
        y: 0,
        w: 1000,
        h: 800,
    };

    #[test]
    fn a_window_inside_the_clip_is_not_cut() {
        assert_eq!(visible_part(Rect::new(100, 0, 400, 800), MONITOR), None);
    }

    #[test]
    fn a_window_over_the_edge_keeps_only_what_is_inside() {
        let outer = Rect::new(800, 0, 400, 800);
        assert_eq!(visible_part(outer, MONITOR), Some([0, 0, 200, 800]));
        let outer = Rect::new(-150, 0, 400, 800);
        assert_eq!(visible_part(outer, MONITOR), Some([150, 0, 400, 800]));
    }

    #[test]
    fn a_window_wholly_outside_shows_nothing() {
        assert_eq!(
            visible_part(Rect::new(1000, 0, 400, 800), MONITOR),
            Some([0; 4])
        );
    }
}
