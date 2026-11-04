//! Window borders thicker than Windows' own one pixel: a ring drawn round each tiled window, on
//! a click-through layered window of its own with a smooth edge. It runs on its own thread like
//! the indicator, and never takes focus or clicks.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use tile_config::Color;
use tile_core::Rect;
use winsafe::{self as w, co, gui, prelude::*};

use crate::{
    frame::visible_part,
    gui::{Mailbox, WindowThread, scale},
};

/// Windows 11's window corner radius, in pixels at 100% scaling.
const WINDOW_RADIUS: i32 = 8;

/// A ring to draw round a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ring {
    /// Names the ring: the window it surrounds.
    pub key: isize,
    /// The window's visible frame; the ring is drawn outside it.
    pub frame: Rect,
    /// The ring's colour.
    pub color: Color,
    /// How far the ring reaches beyond the frame, in pixels at 100% scaling.
    pub width: u32,
    /// The part of the screen the ring may show in, as for its window.
    pub clip: Option<Rect>,
}

/// How long the rings wait between frames if DWM can't say when one is drawn.
const FOLLOW: Duration = Duration::from_millis(8);

/// How often a ring checks its window while nothing is moving.
const WATCH: Duration = Duration::from_millis(40);

/// How many frames without a change before the rings stop following every frame.
const QUIET_FRAMES: u32 = 30;

/// How often the ticker checks whether there are rings to follow.
const IDLE: Duration = Duration::from_millis(50);

enum Request {
    Set(Vec<Ring>),
    /// Time to read where the windows are now.
    Follow,
    Quit,
}

/// The border rings, on their own thread.
pub struct Borders(WindowThread<Request>);

impl Borders {
    /// Starts the thread.
    pub fn spawn() -> Self {
        Self(WindowThread::spawn("borders", run))
    }

    /// Makes these the rings that are drawn: new ones appear, moved ones follow, others go.
    pub fn set(&self, rings: Vec<Ring>) {
        self.0.send(Request::Set(rings));
    }

    /// Closes the rings and stops the thread.
    pub fn shutdown(&mut self) {
        self.0.shutdown(Request::Quit);
    }
}

/// How much of the pixel centred at (`x`, `y`) is inside a rounded rectangle of `w` by `h`
/// whose corners have `radius`, from 0 to 1, with a one pixel smooth edge.
fn coverage(x: f32, y: f32, w: f32, h: f32, radius: f32) -> f32 {
    let qx = (x - w / 2.0).abs() - (w / 2.0 - radius);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let distance = outside + qx.max(qy).min(0.0) - radius;
    (0.5 - distance).clamp(0.0, 1.0)
}

/// The pixels, top row first, of a `width` by `height` ring `thickness` thick whose inside has
/// corners of `radius`: premultiplied BGRA, as `UpdateLayeredWindow` wants.
fn render(width: i32, height: i32, thickness: i32, radius: i32, color: Color) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let t = thickness as f32;
    let outer_radius = (radius + thickness) as f32;
    let (inner_w, inner_h) = (width as f32 - 2.0 * t, height as f32 - 2.0 * t);
    let corner = radius + thickness;
    for y in 0..height {
        let edge_row = y < thickness || y >= height - thickness;
        let corner_row = y < corner || y >= height - corner;
        for x in 0..width {
            let edge = edge_row || x < thickness || x >= width - thickness;
            let in_corner = corner_row && (x < corner || x >= width - corner);
            let alpha = if in_corner {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let outer = coverage(px, py, width as f32, height as f32, outer_radius);
                let inner = coverage(px - t, py - t, inner_w, inner_h, radius as f32);
                (outer - inner).max(0.0)
            } else if edge {
                1.0
            } else {
                continue;
            };
            let at = ((y * width + x) * 4) as usize;
            let premultiplied = |c: u8| (f32::from(c) * alpha).round() as u8;
            pixels[at] = premultiplied(color.b);
            pixels[at + 1] = premultiplied(color.g);
            pixels[at + 2] = premultiplied(color.r);
            pixels[at + 3] = (alpha * 255.0).round() as u8;
        }
    }
    pixels
}

/// A ring's window and what it was last drawn as.
struct Overlay {
    hwnd: w::HWND,
    /// Size, colour, thickness and radius of the last drawing.
    drawn: Option<(w::SIZE, Color, i32, i32)>,
    origin: w::POINT,
    /// The ring as last asked for, whose frame follows the window.
    ring: Option<Ring>,
    /// The window's invisible borders (left, top, right, bottom) and the outer size they were
    /// measured at, so its position can be read straight from `GetWindowRect`, which is ahead
    /// of what DWM reports.
    insets: Option<([i32; 4], (i32, i32))>,
    /// How many times the ring has been moved, redrawn or hidden, to tell when nothing changes.
    changes: u32,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let _ = self.hwnd.DestroyWindow();
    }
}

impl Overlay {
    fn new() -> Option<Self> {
        let hwnd = tile_sys::hwnd(tile_sys::create_overlay()?)?;
        Some(Self {
            hwnd,
            drawn: None,
            origin: w::POINT::default(),
            ring: None,
            insets: None,
            changes: 0,
        })
    }

    /// Puts the ring just above its window in the stacking order, so whatever is over the window
    /// is over the ring too.
    fn stack_above(&mut self, target: &w::HWND) {
        // Look at where the ring is, not at what was last done: windows are raised and lowered
        // without telling us, and in monocle that is every focus change.
        if self
            .hwnd
            .GetWindow(co::GW::HWNDNEXT)
            .is_ok_and(|below| &below == target)
        {
            return;
        }
        self.changes += 1;
        let place = match target.GetWindow(co::GW::HWNDPREV) {
            Ok(above) if above == self.hwnd => return,
            Ok(above) => w::HwndPlace::Hwnd(above),
            Err(_) => w::HwndPlace::Place(co::HWND_PLACE::TOP),
        };
        let _ = self.hwnd.SetWindowPos(
            place,
            w::POINT::default(),
            w::SIZE::default(),
            co::SWP::NOACTIVATE | co::SWP::NOMOVE | co::SWP::NOSIZE,
        );
    }

    /// Where the window's visible frame is now: its outer rect, which Windows updates as soon as
    /// the window is moved, less its invisible borders, which are measured again whenever its
    /// size changes (a move to a monitor of another scale does that).
    fn live_frame(&mut self, target: &w::HWND) -> Option<Rect> {
        let outer = target.GetWindowRect().ok()?;
        let size = (outer.right - outer.left, outer.bottom - outer.top);
        let [left, top, right, bottom] = match self.insets {
            Some((insets, measured)) if measured == size => insets,
            _ => {
                let frame = crate::windows::frame_of(target)?;
                let insets = [
                    frame.x - outer.left,
                    frame.y - outer.top,
                    outer.right - frame.right(),
                    outer.bottom - frame.bottom(),
                ];
                self.insets = Some((insets, size));
                insets
            }
        };
        Some(Rect::from_ltrb(
            outer.left + left,
            outer.top + top,
            outer.right - right,
            outer.bottom - bottom,
        ))
    }

    /// Moves the ring to where its window is now, or hides it if the window isn't on screen.
    fn follow(&mut self) -> w::SysResult<()> {
        let Some(mut ring) = self.ring else {
            return Ok(());
        };
        let frame = tile_sys::hwnd(ring.key)
            .filter(|target| target.IsWindowVisible())
            .and_then(|target| self.live_frame(&target))
            .filter(|frame| frame.x > -30_000);
        match frame {
            Some(frame) => {
                ring.frame = frame;
                self.place(&ring)
            }
            None => {
                if self.drawn.take().is_some() {
                    self.hwnd.ShowWindow(co::SW::HIDE);
                    self.changes += 1;
                }
                Ok(())
            }
        }
    }

    /// Cuts the ring at the edge of its clip, once it is where it goes.
    fn cut(&self, ring: &Ring, origin: w::POINT, size: w::SIZE) {
        if let Some(clip) = ring.clip {
            let outer = Rect::new(origin.x, origin.y, size.cx, size.cy);
            tile_sys::set_window_clip(self.hwnd.ptr() as isize, visible_part(outer, clip));
        }
    }

    /// Draws `ring` round its frame, redrawing the pixels only if the look changed.
    fn place(&mut self, ring: &Ring) -> w::SysResult<()> {
        self.ring = Some(*ring);
        let (cx, cy) = ring.frame.center();
        let dpi = tile_shell::monitor_dpi(cx, cy);
        let thickness = scale(ring.width as i32, dpi).max(1);
        let radius = scale(WINDOW_RADIUS, dpi);
        let origin = w::POINT::with(ring.frame.x - thickness, ring.frame.y - thickness);
        let size = w::SIZE::with(ring.frame.w + 2 * thickness, ring.frame.h + 2 * thickness);
        let look = (size, ring.color, thickness, radius);
        let Some(target) = tile_sys::hwnd(ring.key) else {
            self.hwnd.ShowWindow(co::SW::HIDE);
            self.drawn = None;
            return Ok(());
        };
        if self.drawn == Some(look) {
            if origin != self.origin {
                self.hwnd.SetWindowPos(
                    w::HwndPlace::None,
                    origin,
                    size,
                    co::SWP::NOACTIVATE | co::SWP::NOSIZE | co::SWP::NOZORDER,
                )?;
                self.origin = origin;
                self.changes += 1;
            }
            self.cut(ring, origin, size);
            self.stack_above(&target);
            return Ok(());
        }
        let pixels = render(size.cx, size.cy, thickness, radius, ring.color);
        let screen = w::HWND::NULL.GetDC()?;
        let memory = screen.CreateCompatibleDC()?;
        let bitmap = w::HBITMAP::CreateBitmap(size, 1, 32, std::ptr::null_mut())?;
        let mut info = w::BITMAPINFO::default();
        info.bmiHeader.biWidth = size.cx;
        info.bmiHeader.biHeight = -size.cy;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = co::BI::RGB;
        screen.SetDIBits(
            &bitmap,
            0,
            size.cy as u32,
            &pixels,
            &info,
            co::DIB::RGB_COLORS,
        )?;
        let _selected = memory.SelectObject(&*bitmap)?;
        let mut blend = w::BLENDFUNCTION::default();
        blend.SourceConstantAlpha = 255;
        self.hwnd.UpdateLayeredWindow(
            Some(&screen),
            Some(&origin),
            Some(&size),
            Some(&memory),
            Some(&w::POINT::default()),
            w::COLORREF::from_rgb(0, 0, 0),
            &blend,
            co::ULW::ALPHA,
        )?;
        self.cut(ring, origin, size);
        self.hwnd.ShowWindow(co::SW::SHOWNOACTIVATE);
        self.stack_above(&target);
        self.drawn = Some(look);
        self.origin = origin;
        self.changes += 1;
        Ok(())
    }
}

fn run(requests: mpsc::Receiver<Request>, mailbox: Mailbox<Request>) {
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title: "Tile borders",
        class_name: "tile-borders",
        class_bg_brush: gui::Brush::None,
        size: (1, 1),
        style: co::WS::POPUP,
        ex_style: co::WS_EX::TOOLWINDOW | co::WS_EX::NOACTIVATE,
        ..Default::default()
    });
    let overlays: Rc<RefCell<HashMap<isize, Overlay>>> = Rc::default();
    // Rings follow their windows by reading where they are, so a drag looks smooth whatever
    // the daemon is doing.
    let following = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    // Whether something moved lately, so the rings are followed every frame.
    let busy = Arc::new(AtomicBool::new(false));
    let quiet = Rc::new(Cell::new(0u32));
    let ticker = thread::Builder::new().name("borders tick".into()).spawn({
        let (mailbox, following, running, busy) = (
            mailbox.clone(),
            following.clone(),
            running.clone(),
            busy.clone(),
        );
        move || {
            while running.load(Ordering::Relaxed) {
                if !following.load(Ordering::Relaxed) {
                    thread::sleep(IDLE);
                } else if busy.load(Ordering::Relaxed) {
                    // Right after the compositor draws a frame, so a ring moves with its window.
                    if w::DwmFlush().is_err() {
                        thread::sleep(FOLLOW);
                    }
                    mailbox.send(Request::Follow);
                } else {
                    // Nothing moved lately: look only now and then, to catch the next move.
                    thread::sleep(WATCH);
                    mailbox.send(Request::Follow);
                }
            }
        }
    });

    wnd.on().wm_create({
        let (wnd, mailbox) = (wnd.clone(), mailbox.clone());
        move |_| {
            mailbox.attach(wnd.hwnd());
            Ok(0)
        }
    });

    wnd.on().wm(co::WM::APP, {
        let (overlays, following, running, busy, quiet) = (
            overlays.clone(),
            following.clone(),
            running.clone(),
            busy.clone(),
            quiet.clone(),
        );
        move |_| {
            // Only the latest set matters.
            let mut latest = None;
            let mut follow = false;
            while let Ok(request) = requests.try_recv() {
                match request {
                    Request::Set(rings) => latest = Some(rings),
                    Request::Follow => follow = true,
                    Request::Quit => {
                        running.store(false, Ordering::Relaxed);
                        overlays.borrow_mut().clear();
                        w::PostQuitMessage(0);
                        return Ok(0);
                    }
                }
            }
            let before: u32 = overlays.borrow().values().map(|o| o.changes).sum();
            if follow && latest.is_none() {
                for overlay in overlays.borrow_mut().values_mut() {
                    if let Err(e) = overlay.follow() {
                        tracing::debug!("border ring: {e}");
                    }
                }
            }
            if let Some(rings) = latest {
                following.store(!rings.is_empty(), Ordering::Relaxed);
                let mut overlays = overlays.borrow_mut();
                overlays.retain(|key, _| rings.iter().any(|r| r.key == *key));
                for ring in &rings {
                    if !overlays.contains_key(&ring.key)
                        && let Some(overlay) = Overlay::new()
                    {
                        overlays.insert(ring.key, overlay);
                    }
                    if let Some(overlay) = overlays.get_mut(&ring.key)
                        && let Err(e) = overlay.place(ring)
                    {
                        tracing::debug!("border ring: {e}");
                    }
                }
            }
            let after: u32 = overlays.borrow().values().map(|o| o.changes).sum();
            if after == before {
                quiet.set(quiet.get().saturating_add(1));
            } else {
                quiet.set(0);
            }
            busy.store(quiet.get() < QUIET_FRAMES, Ordering::Relaxed);
            Ok(0)
        }
    });

    if let Err(e) = wnd.run_main(Some(co::SW::HIDE)) {
        tracing::error!("borders message loop failed: {e}");
    }
    running.store(false, Ordering::Relaxed);
    if let Ok(ticker) = ticker {
        let _ = ticker.join();
    }
    mailbox.detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(pixels: &[u8], width: i32, x: i32, y: i32) -> u8 {
        pixels[((y * width + x) * 4 + 3) as usize]
    }

    #[test]
    fn the_ring_is_solid_on_its_edges_and_clear_inside() {
        let (w, h) = (60, 40);
        let pixels = render(w, h, 3, 8, Color::rgb(255, 0, 0));
        assert_eq!(alpha_at(&pixels, w, 30, 0), 255);
        assert_eq!(alpha_at(&pixels, w, 30, 2), 255);
        assert_eq!(alpha_at(&pixels, w, 30, 3), 0);
        assert_eq!(alpha_at(&pixels, w, 0, 20), 255);
        assert_eq!(alpha_at(&pixels, w, 30, 20), 0);
    }

    #[test]
    fn corners_are_smooth_and_premultiplied() {
        let (w, h) = (60, 40);
        let pixels = render(w, h, 3, 8, Color::rgb(200, 100, 50));
        assert_eq!(alpha_at(&pixels, w, 0, 0), 0);
        let partial = (0..11)
            .flat_map(|y| (0..11).map(move |x| (x, y)))
            .filter(|&(x, y)| matches!(alpha_at(&pixels, w, x, y), 1..=254))
            .count();
        assert!(partial > 4, "{partial}");
        let at = (20 * w * 4) as usize;
        assert_eq!(&pixels[at..at + 4], &[50, 100, 200, 255]);
    }
}
