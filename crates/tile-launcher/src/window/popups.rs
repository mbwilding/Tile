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
/// The window class of Windows' own popup menu.
const MENU_CLASS: &str = "#32768";
/// The class prefix of every window Tile itself creates, so none is mistaken for a menu.
const OWN_CLASS_PREFIX: &str = "tile-";

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
    /// Every visible top-level window, of any app, that was showing at the click, so
    /// [`menu_open`] can notice a new one even when it isn't [`MENU_CLASS`].
    pub(super) before: HashSet<isize>,
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

/// Every visible top-level window right now, of any app, to notice a new one later.
fn all_visible() -> HashSet<isize> {
    enumerate(|h| h.IsWindowVisible())
        .iter()
        .map(|h| h.ptr() as isize)
        .collect()
}

/// Whether a window with these attributes is a freshly opened popup menu: [`MENU_CLASS`], or a
/// new, visible, borderless, always-on-top window of some other app, which is how most apps that
/// draw their own menu instead of using Windows' (a Windows 11 style one, WPF, Electron) show
/// one. Split out from [`menu_open`] so it can be tested without a live window.
fn is_new_menu(
    id: isize,
    visible: bool,
    class: &str,
    style: isize,
    ex_style: isize,
    before: &HashSet<isize>,
) -> bool {
    if !visible {
        return false;
    }
    if class == MENU_CLASS {
        return true;
    }
    if before.contains(&id) || class.starts_with(OWN_CLASS_PREFIX) {
        return false;
    }
    let caption = co::WS::CAPTION.raw() as isize;
    let topmost = co::WS_EX::TOPMOST.raw() as isize;
    style & caption != caption && ex_style & topmost == topmost
}

/// Whether a popup menu is showing, native or drawn by the app itself: see [`is_new_menu`].
/// `before` is the windows that were already showing when the click that may have opened it
/// happened (see [`MenuWatch::before`]).
pub(super) fn menu_open(before: &HashSet<isize>) -> bool {
    !enumerate(|h| {
        is_new_menu(
            h.ptr() as isize,
            h.IsWindowVisible(),
            &h.GetClassName().unwrap_or_default(),
            h.GetWindowLongPtr(co::GWLP::STYLE),
            h.GetWindowLongPtr(co::GWLP::EXSTYLE),
            before,
        )
    })
    .is_empty()
}

/// Whether some window, anywhere, is in a popup menu's modal loop right now. Checked before the
/// launcher reclaims focus once [`menu_open`] has given up on seeing one, so a menu it still
/// somehow missed is not closed by stealing its focus.
pub(super) fn menu_active() -> bool {
    w::GetGUIThreadInfo(0).is_ok_and(|info| info.flags.has(co::GUI::INMENUMODE))
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
        before: all_visible(),
        fast: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BORDERLESS: isize = 0;
    const CAPTIONED: isize = co::WS::CAPTION.raw() as isize;
    const TOPMOST: isize = co::WS_EX::TOPMOST.raw() as isize;
    const NOT_TOPMOST: isize = 0;

    #[test]
    fn sees_windows_own_menu_class_even_if_already_showing() {
        let before = HashSet::from([1]);
        assert!(is_new_menu(
            1,
            true,
            "#32768",
            CAPTIONED,
            NOT_TOPMOST,
            &before
        ));
    }

    #[test]
    fn sees_a_new_borderless_topmost_window_of_another_app() {
        let before = HashSet::new();
        assert!(is_new_menu(
            1,
            true,
            "HwndWrapper[App;;1]",
            BORDERLESS,
            TOPMOST,
            &before
        ));
    }

    #[test]
    fn ignores_a_window_that_was_already_showing() {
        let before = HashSet::from([1]);
        assert!(!is_new_menu(
            1,
            true,
            "HwndWrapper[App;;1]",
            BORDERLESS,
            TOPMOST,
            &before
        ));
    }

    #[test]
    fn ignores_a_new_window_with_a_caption() {
        let before = HashSet::new();
        assert!(!is_new_menu(
            1,
            true,
            "App window",
            CAPTIONED,
            TOPMOST,
            &before
        ));
    }

    #[test]
    fn ignores_a_new_window_that_is_not_topmost() {
        let before = HashSet::new();
        assert!(!is_new_menu(
            1,
            true,
            "App window",
            BORDERLESS,
            NOT_TOPMOST,
            &before
        ));
    }

    #[test]
    fn ignores_one_of_tiles_own_windows() {
        let before = HashSet::new();
        assert!(!is_new_menu(
            1,
            true,
            "tile-toast",
            BORDERLESS,
            TOPMOST,
            &before
        ));
    }

    #[test]
    fn ignores_a_window_that_is_not_visible() {
        let before = HashSet::new();
        assert!(!is_new_menu(
            1,
            false,
            "#32768",
            CAPTIONED,
            NOT_TOPMOST,
            &before
        ));
    }
}
