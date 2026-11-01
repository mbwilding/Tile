use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tile_core::WindowId;
use tile_input::{Decision, MouseEvent, MouseHandler, MouseHook};
use tokio::sync::mpsc;
use winsafe::{self as w, co};

use crate::{hotkeys::WIN_USED, windows::is_shell_window};

/// Reported when a Win+left-button drag begins and ends. The window is moved by the hook
/// itself, so the daemon only needs to know when to keep its hands off and where it was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEvent {
    /// Pressed over a top-level window, which the hook now moves.
    Start {
        /// The window being dragged.
        window: WindowId,
    },
    /// Released at a point on the screen, which is where the window was dropped.
    End {
        /// The pointer's x in physical screen pixels.
        x: i32,
        /// The pointer's y in physical screen pixels.
        y: i32,
    },
}

/// Win + left-drag moves the window under the pointer, via a `WH_MOUSE_LL` hook.
///
/// The press and release are swallowed, so the window never sees the click. The hook thread
/// moves the window on every mouse move, with no hop through the daemon's busy event loop, which
/// is what keeps it close to the pointer.
pub struct WinDrag {
    enabled: Arc<AtomicBool>,
    _hook: MouseHook,
}

/// The window being dragged, and where in it the pointer holds it.
struct Grab {
    hwnd: w::HWND,
    offset: (i32, i32),
    last: Instant,
}

struct Dragger {
    enabled: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<DragEvent>,
    grab: Option<Grab>,
}

/// Mouse moves arrive far faster than a screen refreshes; moving the window for each only fills
/// its message queue.
const MOVE_INTERVAL: Duration = Duration::from_millis(4);

fn win_held() -> bool {
    w::GetAsyncKeyState(co::VK::LWIN) || w::GetAsyncKeyState(co::VK::RWIN)
}

fn window_at(x: i32, y: i32) -> Option<w::HWND> {
    let hwnd = w::HWND::WindowFromPoint(w::POINT::with(x, y))?;
    Some(hwnd.GetAncestor(co::GA::ROOT).unwrap_or(hwnd))
}

fn move_to(hwnd: &w::HWND, x: i32, y: i32) {
    let flags = co::SWP::NOSIZE
        | co::SWP::NOZORDER
        | co::SWP::NOACTIVATE
        | co::SWP::NOOWNERZORDER
        | co::SWP::ASYNCWINDOWPOS;
    let _ = hwnd.SetWindowPos(
        w::HwndPlace::None,
        w::POINT::with(x, y),
        w::SIZE::default(),
        flags,
    );
}

impl Dragger {
    fn begin(&mut self, x: i32, y: i32) -> Decision {
        let Some(hwnd) = window_at(x, y) else {
            return Decision::Pass;
        };
        let raw = hwnd.ptr() as isize;
        if hwnd.IsZoomed() || is_shell_window(raw) {
            return Decision::Pass;
        }
        let Ok(outer) = hwnd.GetWindowRect() else {
            return Decision::Pass;
        };
        self.grab = Some(Grab {
            hwnd,
            offset: (x - outer.left, y - outer.top),
            last: Instant::now(),
        });
        WIN_USED.store(true, Ordering::Relaxed);
        // Stops Win's release opening the Start menu.
        tile_input::tap_silent();
        let _ = self.tx.send(DragEvent::Start {
            window: WindowId(raw),
        });
        Decision::Block
    }
}

impl MouseHandler for Dragger {
    fn mouse(&mut self, event: MouseEvent, x: i32, y: i32) -> Decision {
        match event {
            MouseEvent::LeftDown
                if self.grab.is_none() && self.enabled.load(Ordering::Relaxed) && win_held() =>
            {
                self.begin(x, y)
            }
            MouseEvent::Move => {
                if let Some(grab) = &mut self.grab
                    && grab.last.elapsed() >= MOVE_INTERVAL
                {
                    grab.last = Instant::now();
                    move_to(&grab.hwnd, x - grab.offset.0, y - grab.offset.1);
                }
                Decision::Pass
            }
            MouseEvent::LeftUp => match self.grab.take() {
                Some(grab) => {
                    // The last position, which a throttled move may have skipped.
                    move_to(&grab.hwnd, x - grab.offset.0, y - grab.offset.1);
                    let _ = self.tx.send(DragEvent::End { x, y });
                    Decision::Block
                }
                None => Decision::Pass,
            },
            _ => Decision::Pass,
        }
    }
}

impl WinDrag {
    /// Installs the mouse hook, which reports drags to `tx`. It starts disabled; see
    /// [`WinDrag::set_enabled`].
    pub fn start(tx: mpsc::UnboundedSender<DragEvent>) -> io::Result<Self> {
        let enabled = Arc::new(AtomicBool::new(false));
        let shared = enabled.clone();
        let hook = tile_input::spawn_mouse("mouse hook", move || Dragger {
            enabled: shared,
            tx,
            grab: None,
        })?;
        Ok(Self {
            enabled,
            _hook: hook,
        })
    }

    /// Whether Win + drag is taken over. A drag already in progress still finishes.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}
