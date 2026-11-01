use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use tile_core::{
    Rect, WindowId,
    animation::{Bezier, lerp_rect},
};
use winsafe as w;

use crate::frame::{Insets, Placement, set_frame};

/// One window's move from `from` to `to` (visible frames) over `duration`.
#[derive(Debug, Clone, Copy)]
pub struct Animation {
    pub from: Rect,
    pub to: Rect,
    pub start: Instant,
    pub duration: Duration,
    pub curve: Bezier,
    pub insets: Insets,
    /// Skip `WM_WINDOWPOSCHANGING` so app minimum sizes can't refuse it.
    pub force: bool,
}

impl Animation {
    /// Where the window should be at `now`, and whether the animation has finished.
    pub fn at(&self, now: Instant) -> (Rect, bool) {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let progress = elapsed / self.duration.as_secs_f32().max(f32::EPSILON);
        if progress >= 1.0 {
            (self.to, true)
        } else {
            (
                lerp_rect(self.from, self.to, self.curve.ease(progress)),
                false,
            )
        }
    }

    pub fn end(&self) -> Instant {
        self.start + self.duration
    }
}

#[derive(Default)]
struct Shared {
    next: Mutex<Option<Animation>>,
    wake: Condvar,
    closed: AtomicBool,
}

/// Animates one window on its own thread, driven by the app's own redraws.
///
/// Every frame is applied synchronously and then painted with `RDW_UPDATENOW`, which only
/// returns once the app (and its child windows) finished `WM_PAINT`; then it waits for the
/// compositor's next frame (`DwmFlush`). So each app animates exactly as fast as it can
/// redraw, never faster than the display refreshes: no timers, rates or heuristics. A hung
/// app only blocks its own mover.
pub struct Mover {
    shared: Arc<Shared>,
}

impl Mover {
    /// Starts a mover for the top-level window `id`, or `None` if it has gone.
    pub fn spawn(id: WindowId) -> Option<Self> {
        let hwnd = tile_sys::hwnd(id.0)?;
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        thread::Builder::new()
            .name("window mover".into())
            .spawn(move || run(&hwnd, &worker))
            .ok()?;
        Some(Self { shared })
    }

    /// Starts (or retargets) the animation; the mover picks it up at its next frame.
    pub fn animate(&self, animation: Animation) {
        if let Ok(mut next) = self.shared.next.lock() {
            *next = Some(animation);
            self.shared.wake.notify_one();
        }
    }
}

impl Drop for Mover {
    /// The worker finishes the animation in progress (its final frame) before exiting.
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.wake.notify_one();
    }
}

fn run(hwnd: &w::HWND, shared: &Shared) {
    loop {
        let mut animation = {
            let Ok(mut next) = shared.next.lock() else {
                return;
            };
            while next.is_none() && !shared.closed.load(Ordering::Acquire) {
                next = match shared.wake.wait(next) {
                    Ok(next) => next,
                    Err(_) => return,
                };
            }
            match next.take() {
                Some(animation) => animation,
                None => return,
            }
        };
        loop {
            if let Some(newer) = shared.next.lock().ok().and_then(|mut next| next.take()) {
                animation = newer;
            }
            let (rect, done) = animation.at(Instant::now());
            let started = Instant::now();
            draw(hwnd, rect, &animation, done);
            tracing::trace!(
                "mover {:#x}: frame drawn in {:.1} ms",
                hwnd.ptr() as isize,
                started.elapsed().as_secs_f64() * 1000.0
            );
            if done || !hwnd.IsWindow() {
                break;
            }
            // Wait for the compositor's next frame (vsync) before drawing the next one.
            let _ = w::DwmFlush();
        }
    }
}

/// Applies one frame and waits until the app has painted it. `settled` marks the last one.
fn draw(hwnd: &w::HWND, visible: Rect, animation: &Animation, settled: bool) {
    let how = Placement {
        force: animation.force,
        settled,
        sync: true,
    };
    if let Err(e) = set_frame(hwnd, visible, animation.insets, how) {
        tracing::debug!("mover: SetWindowPos failed: {e}");
    }
}
