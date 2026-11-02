//! Window messages and session notifications for windows of this process, and safe handles and
//! facts for the raw window handles window events report.

use std::ffi::c_void;

use windows_sys::Win32::{
    System::RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
    },
    UI::Shell::{SHEmptyRecycleBinW, SHQUERYRBINFO, SHQueryRecycleBinW},
    UI::WindowsAndMessaging::{
        CreateWindowExW, GetClassNameW, GetWindowThreadProcessId, HWND_BROADCAST, IsWindow,
        PostMessageW, SC_CLOSE, SC_MONITORPOWER, WM_APP, WM_SYSCOMMAND, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    },
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

/// Turns every monitor off until the next input, as the power button's display-off does.
pub fn turn_off_display() -> bool {
    // SAFETY: posting a message with no pointers to every top-level window; the `2` asks for the
    // monitors to power off.
    unsafe { PostMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER as usize, 2) != 0 }
}

/// Empties the Recycle Bin of every drive. Windows asks to confirm and shows its own progress.
pub fn empty_recycle_bin() -> bool {
    // SAFETY: no owner window and no root path (every drive); the flags are all zero, so
    // Windows keeps its confirmation, progress and sound.
    unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), 0) >= 0 }
}

/// Makes a hidden popup for the caller to draw on with `UpdateLayeredWindow`: layered,
/// never activated and click-through. The caller destroys it, on this thread.
pub fn create_overlay() -> Option<isize> {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    // SAFETY: `STATIC` is a system class, so there is no window procedure of ours to keep alive;
    // the string is NUL-terminated and outlives the call, and the rest is null (no parent, menu,
    // module or creation data).
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    (!hwnd.is_null()).then_some(hwnd as isize)
}

/// How many items the Recycle Bin holds across every drive, if Windows says.
pub fn recycle_bin_items() -> Option<u64> {
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        i64Size: 0,
        i64NumItems: 0,
    };
    // SAFETY: no root path (every drive) and `info` is a live struct with `cbSize` set, as the
    // call requires.
    let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
    (hr >= 0).then_some(info.i64NumItems.max(0) as u64)
}

/// Posts `WM_APP + offset` to the window `hwnd`, to wake its thread from another one.
pub fn post_app(hwnd: isize, offset: u32) -> bool {
    // SAFETY: posting a message with no pointers; Windows validates the handle and fails for
    // one that isn't a window.
    unsafe { PostMessageW(hwnd as *mut c_void, WM_APP + offset, 0, 0) != 0 }
}

/// A winsafe handle for the raw window handle `raw`, if it is a window that exists, without
/// enumerating every window to find it.
///
/// A handle can outlive its window, and Windows may in time give its value to a new window, as
/// it can with any handle `EnumWindows` returns; every call on it is checked by Windows.
pub fn hwnd(raw: isize) -> Option<winsafe::HWND> {
    // SAFETY: `IsWindow` accepts any handle value.
    let exists = unsafe { IsWindow(raw as *mut c_void) } != 0;
    // SAFETY: the handle names a window that exists; winsafe only passes it to Windows, which
    // validates it on every call.
    exists.then(|| unsafe { winsafe::HWND::from_ptr(raw as *mut c_void) })
}

/// The class name of the window `raw`, or `None` if it isn't a window.
pub fn class_name(raw: isize) -> Option<String> {
    let mut class = [0u16; 256];
    // SAFETY: `class` has room for the length given; Windows validates the handle.
    let length =
        unsafe { GetClassNameW(raw as *mut c_void, class.as_mut_ptr(), class.len() as i32) };
    (length > 0).then(|| String::from_utf16_lossy(&class[..length as usize]))
}

/// The id of the process that owns the window `raw`, or `None` if it isn't a window.
pub fn pid_of(raw: isize) -> Option<u32> {
    let mut pid = 0;
    // SAFETY: `pid` is a live out pointer; Windows validates the handle.
    unsafe { GetWindowThreadProcessId(raw as *mut c_void, &raw mut pid) };
    (pid != 0).then_some(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_a_real_window_from_a_stale_handle() {
        // SAFETY: no arguments.
        let desktop = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow() };
        let raw = desktop as isize;
        assert!(hwnd(raw).is_some());
        assert_eq!(class_name(raw).as_deref(), Some("#32769"));
        assert!(pid_of(raw).is_some());
        assert!(hwnd(0).is_none());
        assert_eq!(class_name(0), None);
        assert_eq!(pid_of(0), None);
    }
}
