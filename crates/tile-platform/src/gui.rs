//! Helpers shared by Tile's own windows (the launcher and the workspace indicator): each runs
//! on a thread of its own, painted double buffered with fonts and sizes scaled for its monitor.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use tile_config::{Color, Corners};
use winsafe::{self as w, co, guard::DeleteObjectGuard};

/// `c` as a Win32 colour.
pub const fn colorref(c: Color) -> w::COLORREF {
    w::COLORREF::from_rgb(c.r, c.g, c.b)
}

/// `a` moved `amount` (0 to 255) of the way towards `b`.
pub fn mix(a: Color, b: Color, amount: u16) -> Color {
    let ch = |x: u8, y: u8| ((u16::from(x) * (255 - amount) + u16::from(y) * amount) / 255) as u8;
    Color::rgb(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b))
}

/// The DWM corner style for `corners`.
pub const fn corner_preference(corners: Corners) -> co::DWMWCP {
    match corners {
        Corners::Round => co::DWMWCP::ROUND,
        Corners::Small => co::DWMWCP::ROUNDSMALL,
        Corners::Square => co::DWMWCP::DONOTROUND,
    }
}

/// `px` at 96 DPI scaled to `dpi`.
pub const fn scale(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

/// Joins this thread to a single-threaded COM apartment, which the shell's objects want, until
/// the guard is dropped.
pub fn init_com() -> w::HrResult<w::guard::CoUninitializeGuard> {
    w::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE)
}

/// A ClearType font `px` pixels tall. An empty `face` lets Windows choose.
pub fn make_font(face: &str, px: i32, bold: bool) -> w::SysResult<DeleteObjectGuard<w::HFONT>> {
    let mut lf = w::LOGFONT::new_face(-px, face);
    lf.lfQuality = co::QUALITY::CLEARTYPE;
    if bold {
        lf.lfWeight = co::FW::BOLD;
    }
    w::HFONT::CreateFontIndirect(&lf)
}

/// Gives a popup window its corners and border colour.
pub fn style_popup(hwnd: &w::HWND, corners: Corners, border: Color) {
    let _ = hwnd.DwmSetWindowAttribute(w::DwmAttr::WindowCornerPreference(corner_preference(
        corners,
    )));
    let _ = hwnd.DwmSetWindowAttribute(w::DwmAttr::BorderColor(colorref(border)));
}

/// Sets how opaque a layered window is, from 0 to 255.
pub fn set_alpha(hwnd: &w::HWND, alpha: u8) -> w::SysResult<()> {
    hwnd.SetLayeredWindowAttributes(w::COLORREF::from_rgb(0, 0, 0), alpha, co::LWA::ALPHA)
}

/// As [`set_alpha`], but also makes `key` a transparent colour key: any pixel painted exactly
/// that colour shows whatever is behind the window instead. Lets a window hold several separately
/// shaped pieces (the notification popup's stacked cards) with real gaps between them.
pub fn set_transparent(hwnd: &w::HWND, key: Color, alpha: u8) -> w::SysResult<()> {
    hwnd.SetLayeredWindowAttributes(colorref(key), alpha, co::LWA::COLORKEY | co::LWA::ALPHA)
}

static ANTIALIASING: AtomicBool = AtomicBool::new(true);

/// Switches smooth edges on rounded shapes drawn by [`fill_round`] and [`fill_round_bordered`].
pub fn set_antialiasing(on: bool) {
    ANTIALIASING.store(on, Ordering::Relaxed);
}

/// How many times larger than the screen the corners are drawn before shrinking them.
const SUPERSAMPLE: i32 = 4;

/// Redraws the `r` pixel square at `at`, which is one corner of a rounded rectangle of
/// `color`, at [`SUPERSAMPLE`] times the size over what is already there and shrinks it back, so
/// the curve's edge blends with its surroundings. `right` and `bottom` say which corner it is.
fn smooth_corner(
    dc: &w::HDC,
    at: w::POINT,
    r: i32,
    right: bool,
    bottom: bool,
    color: Color,
) -> w::SysResult<()> {
    let big = r * SUPERSAMPLE;
    let scratch = dc.CreateCompatibleDC()?;
    let bitmap = dc.CreateCompatibleBitmap(big, big)?;
    let _bitmap = scratch.SelectObject(&*bitmap)?;
    let (small, large) = (w::SIZE::with(r, r), w::SIZE::with(big, big));
    scratch.StretchBlt(w::POINT::default(), large, dc, at, small, co::ROP::SRCCOPY)?;
    let brush = w::HBRUSH::CreateSolidBrush(colorref(color))?;
    let _brush = scratch.SelectObject(&*brush)?;
    let pen = w::HPEN::CreatePen(co::PS::NULL, 1, colorref(color))?;
    let _pen = scratch.SelectObject(&*pen)?;
    // The whole rectangle, placed so this corner lands in the scratch square; the rest is clipped.
    let left = if right { -big } else { 0 };
    let top = if bottom { -big } else { 0 };
    scratch.RoundRect(
        w::RECT {
            left,
            top,
            right: left + 2 * big + 1,
            bottom: top + 2 * big + 1,
        },
        w::SIZE::with(2 * big, 2 * big),
    )?;
    let _mode = dc.SetStretchBltMode(co::STRETCH_MODE::HALFTONE)?;
    dc.StretchBlt(
        at,
        small,
        &scratch,
        w::POINT::default(),
        large,
        co::ROP::SRCCOPY,
    )
}

/// Fills `bounds` with a rounded rectangle whose corners have `radius`, with smooth corners.
fn fill_smooth(dc: &w::HDC, bounds: w::RECT, radius: i32, color: Color) -> w::SysResult<()> {
    let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
    if width <= 0 || height <= 0 {
        return Ok(());
    }
    let r = radius.clamp(0, width.min(height) / 2);
    let brush = w::HBRUSH::CreateSolidBrush(colorref(color))?;
    let fill = |left: i32, top: i32, right: i32, bottom: i32| {
        if right > left && bottom > top {
            dc.FillRect(
                w::RECT {
                    left,
                    top,
                    right,
                    bottom,
                },
                &brush,
            )
        } else {
            Ok(())
        }
    };
    // Everything but the four corner squares.
    fill(bounds.left, bounds.top + r, bounds.right, bounds.bottom - r)?;
    fill(
        bounds.left + r,
        bounds.top,
        bounds.right - r,
        bounds.top + r,
    )?;
    fill(
        bounds.left + r,
        bounds.bottom - r,
        bounds.right - r,
        bounds.bottom,
    )?;
    if r == 0 {
        return Ok(());
    }
    for (right, bottom) in [(false, false), (true, false), (false, true), (true, true)] {
        let at = w::POINT::with(
            if right { bounds.right - r } else { bounds.left },
            if bottom {
                bounds.bottom - r
            } else {
                bounds.top
            },
        );
        smooth_corner(dc, at, r, right, bottom, color)?;
    }
    Ok(())
}

/// Fills `bounds` with a `color` rounded rectangle, `radius` pixels on each corner. Shared by
/// the launcher's cards and the notification popup, so both look like the same kind of window.
pub fn fill_round(dc: &w::HDC, bounds: w::RECT, radius: i32, color: Color) -> w::SysResult<()> {
    fill_round_bordered(dc, bounds, radius, color, color, 0)
}

/// As [`fill_round`], but stroked with a `width` pixel border in `border`. `width` of 0 draws no
/// border, same as [`fill_round`].
pub fn fill_round_bordered(
    dc: &w::HDC,
    bounds: w::RECT,
    radius: i32,
    fill: Color,
    border: Color,
    width: i32,
) -> w::SysResult<()> {
    if ANTIALIASING.load(Ordering::Relaxed) {
        // RoundRect's `radius` is the corner ellipse's diameter.
        let corner = radius / 2;
        if width > 0 {
            fill_smooth(dc, bounds, corner, border)?;
            let inset = w::RECT {
                left: bounds.left + width,
                top: bounds.top + width,
                right: bounds.right - width,
                bottom: bounds.bottom - width,
            };
            return fill_smooth(dc, inset, (corner - width).max(0), fill);
        }
        return fill_smooth(dc, bounds, corner, fill);
    }
    let brush = w::HBRUSH::CreateSolidBrush(colorref(fill))?;
    let _brush = dc.SelectObject(&*brush)?;
    let style = if width > 0 {
        co::PS::SOLID
    } else {
        co::PS::NULL
    };
    let pen = w::HPEN::CreatePen(style, width.max(1), colorref(border))?;
    let _pen = dc.SelectObject(&*pen)?;
    // RoundRect leaves out the bottom and right edges with a null pen; a real pen needs no such
    // adjustment, but including it anyway keeps both cases pixel-aligned with the bounds given.
    let bounds = w::RECT {
        right: bounds.right + 1,
        bottom: bounds.bottom + 1,
        ..bounds
    };
    dc.RoundRect(bounds, w::SIZE::with(radius, radius))
}

/// Draws `s` in `font` and `color` within `bounds`, truncated to one line with an ellipsis.
pub fn text(
    dc: &w::HDC,
    font: &w::HFONT,
    color: Color,
    s: &str,
    mut bounds: w::RECT,
    flags: co::DT,
) -> w::SysResult<()> {
    let _font = dc.SelectObject(font)?;
    dc.SetTextColor(colorref(color))?;
    dc.DrawText(
        s,
        &mut bounds,
        flags | co::DT::SINGLELINE | co::DT::NOPREFIX | co::DT::END_ELLIPSIS,
    )?;
    Ok(())
}

/// As [`text`], but wraps onto several lines instead of staying on one; text past the bottom of
/// `bounds` is cut with an ellipsis.
pub fn text_wrapped(
    dc: &w::HDC,
    font: &w::HFONT,
    color: Color,
    s: &str,
    mut bounds: w::RECT,
    flags: co::DT,
) -> w::SysResult<()> {
    let _font = dc.SelectObject(font)?;
    dc.SetTextColor(colorref(color))?;
    dc.DrawText(
        s,
        &mut bounds,
        flags | co::DT::WORDBREAK | co::DT::NOPREFIX | co::DT::END_ELLIPSIS,
    )?;
    Ok(())
}

/// Handles `WM_PAINT` by having `paint` draw the whole client area (the rectangle it is given)
/// off screen, then copying it to the window at once, so nothing flickers.
pub fn paint_buffered(
    hwnd: &w::HWND,
    paint: impl FnOnce(&w::HDC, w::RECT) -> w::SysResult<()>,
) -> w::SysResult<()> {
    let hdc = hwnd.BeginPaint()?;
    let client = hwnd.GetClientRect()?;
    let size = w::SIZE::with(client.right - client.left, client.bottom - client.top);
    let mem = hdc.CreateCompatibleDC()?;
    let bitmap = hdc.CreateCompatibleBitmap(size.cx, size.cy)?;
    let _bitmap = mem.SelectObject(&*bitmap)?;
    paint(&mem, client)?;
    hdc.BitBlt(
        w::POINT::default(),
        size,
        &mem,
        w::POINT::default(),
        co::ROP::SRCCOPY,
    )
}

/// Sends requests to a window's thread and wakes its window to read them. Clones send from
/// any thread.
pub struct Mailbox<R> {
    tx: mpsc::Sender<R>,
    hwnd: Arc<AtomicIsize>,
}

impl<R> Clone for Mailbox<R> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            hwnd: self.hwnd.clone(),
        }
    }
}

impl<R> Mailbox<R> {
    /// Queues `request` and wakes the window, once it is made, with `WM_APP`.
    pub fn send(&self, request: R) {
        let _ = self.tx.send(request);
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            tile_sys::post_app(hwnd, 0);
        }
    }

    /// Notes that the window `hwnd` is made, and wakes it for what was sent before.
    pub fn attach(&self, hwnd: &w::HWND) {
        let raw = hwnd.ptr() as isize;
        self.hwnd.store(raw, Ordering::Release);
        tile_sys::post_app(raw, 0);
    }

    /// Notes that the window is gone.
    pub fn detach(&self) {
        self.hwnd.store(0, Ordering::Release);
    }
}

/// A window running on a thread of its own, which takes requests of type `R`.
pub struct WindowThread<R> {
    mailbox: Mailbox<R>,
    thread: Option<JoinHandle<()>>,
}

impl<R: Send + 'static> WindowThread<R> {
    /// Starts `run` on a thread called `name`. It gets the requests to read on `WM_APP`, and a
    /// mailbox to [`Mailbox::attach`] its window to and to send itself more requests with.
    pub fn spawn(
        name: &str,
        run: impl FnOnce(mpsc::Receiver<R>, Mailbox<R>) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mailbox = Mailbox {
            tx,
            hwnd: Arc::new(AtomicIsize::new(0)),
        };
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn({
                let mailbox = mailbox.clone();
                move || run(rx, mailbox)
            })
            .inspect_err(|e| tracing::error!("failed to start the {name} thread: {e}"))
            .ok();
        Self { mailbox, thread }
    }

    /// Queues `request` for the window and wakes it.
    pub fn send(&self, request: R) {
        self.mailbox.send(request);
    }

    /// Sends `quit`, which should end the window's message loop, and waits for the thread.
    pub fn shutdown(&mut self, quit: R) {
        self.send(quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_colours_and_scales_pixels() {
        let (black, white) = (Color::rgb(0, 0, 0), Color::rgb(255, 255, 255));
        assert_eq!(mix(black, white, 0), black);
        assert_eq!(mix(black, white, 255), white);
        assert_eq!(mix(black, white, 51), Color::rgb(51, 51, 51));
        assert_eq!(scale(12, 96), 12);
        assert_eq!(scale(12, 144), 18);
        assert_eq!(scale(36, 120), 45);
    }
}
