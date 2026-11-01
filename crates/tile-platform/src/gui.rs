//! Helpers shared by Tile's own windows (the launcher and the workspace indicator): each runs
//! on a thread of its own, painted double buffered with fonts and sizes scaled for its monitor.

use std::{
    sync::{
        Arc,
        atomic::{AtomicIsize, Ordering},
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
