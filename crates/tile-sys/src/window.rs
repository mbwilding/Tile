//! Window messages and session notifications for windows of this process.

use std::ffi::c_void;

use windows_sys::Win32::{
    System::RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
    },
    UI::WindowsAndMessaging::{PostMessageW, SC_CLOSE, WM_APP, WM_SYSCOMMAND},
};

/// Asks for `WM_WTSSESSION_CHANGE` (lock, unlock, …) to be sent to the window with handle
/// `hwnd`.
pub fn watch_sessions(hwnd: isize) -> bool {
    // SAFETY: the handle is only an identifier, never dereferenced by us; Windows validates it
    // and fails for anything but a window of this process.
    unsafe { WTSRegisterSessionNotification(hwnd as *mut c_void, NOTIFY_FOR_THIS_SESSION) != 0 }
}

/// Undoes [`watch_sessions`], before the window is destroyed.
pub fn unwatch_sessions(hwnd: isize) {
    // SAFETY: as for `watch_sessions`.
    unsafe { WTSUnRegisterSessionNotification(hwnd as *mut c_void) };
}

/// Asks the top-level window `hwnd` to close, exactly as Alt+F4 does (`SC_CLOSE`).
pub fn post_close(hwnd: isize) -> bool {
    // SAFETY: posting a message with no pointers; Windows validates the handle and fails for
    // one that isn't a window.
    unsafe { PostMessageW(hwnd as *mut c_void, WM_SYSCOMMAND, SC_CLOSE as usize, 0) != 0 }
}

/// Posts `WM_APP + offset` to the window `hwnd`, to wake its thread from another one.
pub fn post_app(hwnd: isize, offset: u32) -> bool {
    // SAFETY: posting a message with no pointers; Windows validates the handle and fails for
    // one that isn't a window.
    unsafe { PostMessageW(hwnd as *mut c_void, WM_APP + offset, 0, 0) != 0 }
}
