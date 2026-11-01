//! Small helpers shared by Tile's own windows (the launcher and the workspace indicator).

use std::sync::atomic::{AtomicIsize, Ordering};

use tile_config::{Color, Corners};
use winsafe::{self as w, co};

/// `c` as a Win32 colour.
pub const fn colorref(c: Color) -> w::COLORREF {
    w::COLORREF::from_rgb(c.r, c.g, c.b)
}

/// The DWM corner style for `corners`.
pub const fn corner_preference(corners: Corners) -> co::DWMWCP {
    match corners {
        Corners::Round => co::DWMWCP::ROUND,
        Corners::Small => co::DWMWCP::ROUNDSMALL,
        Corners::Square => co::DWMWCP::DONOTROUND,
    }
}

/// Has the window in `hwnd` (once made) read its requests.
pub fn wake(hwnd: &AtomicIsize) {
    let hwnd = hwnd.load(Ordering::Acquire);
    if hwnd != 0 {
        tile_sys::post_app(hwnd, 0);
    }
}
