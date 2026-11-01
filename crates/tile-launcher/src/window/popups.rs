//! What a click on a tray icon opens: its menu, which the launcher waits behind, and a popup
//! the app may put in a monitor's corner, which is moved to the pointer.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use tile_platform::{enumerate, monitors};
use winsafe::{self as w, co, gui, prelude::*};

/// Polls for the menu and the popup while one is being watched.
pub(super) const MENU_TIMER: usize = 2;
/// How long a menu may take to appear before it is taken to have never come.
pub(super) const MENU_GRACE: Duration = Duration::from_millis(1500);
/// How long after a click a popup in a corner is moved, and how often it is looked for then.
pub(super) const FIX_WINDOW: Duration = Duration::from_secs(2);
const POPUP_POLL_MS: u32 = 50;
/// How often the menu is looked for once the popups are no longer.
pub(super) const MENU_POLL_MS: u32 = 250;
/// The window class of a popup menu.
const MENU_CLASS: &str = "#32768";

/// Waiting on what a click on a tray icon opens: for its menu to close, and for a popup the app
/// puts in a monitor's corner (because it can't find the taskbar) to be moved to the pointer.
pub(super) struct MenuWatch {
    pub(super) since: Instant,
    /// The menu has been seen on screen.
    pub(super) seen: bool,
    /// The launcher stays open behind the menu (a right click); otherwise it has closed.
    pub(super) keep_open: bool,
    /// The app's process and where the pointer was, to place its popup by.
    pub(super) pid: Option<u32>,
    pub(super) at: (i32, i32),
    /// The app's windows that were showing at the click, which are not its popup.
    pub(super) baseline: HashSet<isize>,
    /// Checking every [`POPUP_POLL_MS`] still, rather than every [`MENU_POLL_MS`].
    pub(super) fast: bool,
}

/// The windows of the process `pid` that are showing.
fn visible_windows(pid: u32) -> HashSet<isize> {
    enumerate(|h| h.GetWindowThreadProcessId().1 == pid && h.IsWindowVisible())
        .iter()
        .map(|h| h.ptr() as isize)
        .collect()
}

/// Whether a popup menu is showing.
pub(super) fn menu_open() -> bool {
    !enumerate(|h| h.IsWindowVisible() && h.GetClassName().is_ok_and(|c| c == MENU_CLASS))
        .is_empty()
}

/// Some apps place the popup of their tray icon by finding the taskbar, which is not there, and
/// open it in the corner of a monitor. This moves such a popup, a small borderless window of the
/// app that wasn't showing before the click and sits exactly at a monitor's top left corner, to
/// the pointer: above it, or below if there isn't room, kept on that monitor.
pub(super) fn move_corner_popups(pid: u32, baseline: &HashSet<isize>, at: (i32, i32)) {
    let caption = co::WS::CAPTION.raw() as isize;
    let candidates = enumerate(|h| {
        h.GetWindowThreadProcessId().1 == pid
            && h.IsWindowVisible()
            && !baseline.contains(&(h.ptr() as isize))
            && h.GetWindowLongPtr(co::GWLP::STYLE) & caption != caption
    });
    if candidates.is_empty() {
        return;
    }
    let monitors = monitors();
    for hwnd in candidates {
        let Ok(r) = hwnd.GetWindowRect() else {
            continue;
        };
        let (width, height) = (r.right - r.left, r.bottom - r.top);
        if !(40..=800).contains(&width) || !(20..=900).contains(&height) {
            continue;
        }
        let Some(corner) = monitors
            .iter()
            .find(|m| (r.left - m.rect.x).abs() <= 2 && (r.top - m.rect.y).abs() <= 2)
        else {
            continue;
        };
        let area = monitors
            .iter()
            .find(|m| m.rect.contains_point(at.0, at.1))
            .unwrap_or(corner)
            .work_area;
        let x = at.0.clamp(area.x, (area.right() - width).max(area.x));
        let above = at.1 - height;
        let y = if above >= area.y {
            above
        } else {
            (at.1 + 8).min((area.bottom() - height).max(area.y))
        };
        let moved = hwnd.SetWindowPos(
            w::HwndPlace::None,
            w::POINT::with(x, y),
            w::SIZE::default(),
            co::SWP::NOSIZE | co::SWP::NOZORDER | co::SWP::NOACTIVATE,
        );
        if let Err(e) = moved {
            tracing::debug!("couldn't move a tray popup: {e}");
        }
    }
}

/// Starts watching what a click on the tray icon of the window `owner` opens. With `keep_open`
/// the launcher stays behind the menu; otherwise it is closing.
pub(super) fn watch_menu(
    wnd: &gui::WindowMain,
    owner: isize,
    at: (i32, i32),
    keep_open: bool,
) -> MenuWatch {
    let pid = tile_sys::pid_of(owner);
    let _ = wnd.hwnd().SetTimer(MENU_TIMER, POPUP_POLL_MS, None);
    MenuWatch {
        since: Instant::now(),
        seen: false,
        keep_open,
        pid,
        at,
        baseline: pid.map(visible_windows).unwrap_or_default(),
        fast: true,
    }
}
