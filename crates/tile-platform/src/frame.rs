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
}

/// Moves `hwnd` so its visible frame covers `visible`, given its invisible `insets`.
pub(crate) fn set_frame(
    hwnd: &w::HWND,
    visible: Rect,
    insets: Insets,
    how: Placement,
) -> w::SysResult<()> {
    let pos = w::POINT::with(visible.x - insets.left, visible.y - insets.top);
    let size = w::SIZE::with(
        visible.w + insets.left + insets.right,
        visible.h + insets.top + insets.bottom,
    );
    let mut flags = co::SWP::NOZORDER | co::SWP::NOACTIVATE | co::SWP::NOOWNERZORDER;
    if !how.sync {
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
    if how.sync || how.settled {
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
