//! Small Win32 helpers the modules share: strings, processes, windows and results.

use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{HANDLE, HWND},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{
        FindWindowExW, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindow,
    },
};

/// `text` as a NUL-terminated UTF-16 string.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// A failed COM call as a message saying what was being done.
pub(crate) fn context<T>(result: windows::core::Result<T>, what: &str) -> Result<T, String> {
    result.map_err(|e| format!("{what}: {e}"))
}

/// The id of the process that owns `window`, or 0 if the window has gone.
pub(crate) fn pid_of(window: HWND) -> u32 {
    let mut pid = 0;
    // SAFETY: `pid` is a live out pointer; a stale window just leaves it 0.
    unsafe { GetWindowThreadProcessId(window, &raw mut pid) };
    pid
}

/// Whether `window` is a window that still exists.
pub(crate) fn is_window(window: HWND) -> bool {
    // SAFETY: `IsWindow` accepts any handle value.
    unsafe { IsWindow(window) != 0 }
}

/// Opens the process `pid` for `access`; the handle is closed on drop.
pub(crate) fn open_process(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Option<OwnedHandle> {
    // SAFETY: takes plain values.
    let handle = unsafe { OpenProcess(access, 0, pid) };
    // SAFETY: a handle `OpenProcess` just opened, which nothing else owns.
    (!handle.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// The full path of the program the open `process` runs.
pub(crate) fn image_path(process: &OwnedHandle) -> Option<String> {
    let mut buffer = [0u16; 520];
    let mut length = buffer.len() as u32;
    // SAFETY: `buffer` has room for `length` units and `process` is open.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle(),
            0,
            buffer.as_mut_ptr(),
            &raw mut length,
        )
    };
    (ok != 0).then(|| String::from_utf16_lossy(&buffer[..length as usize]))
}

/// The full path of the program `pid` runs.
pub(crate) fn process_path(pid: u32) -> Option<String> {
    image_path(&open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?)
}

/// The file name at the end of `path`.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// `window`'s class name, empty if it has gone.
pub(crate) fn class_name(window: HWND) -> String {
    let mut class = [0u16; 256];
    // SAFETY: `class` has room for the length given.
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    String::from_utf16_lossy(&class[..length.max(0) as usize])
}

/// `window`'s title, up to 255 units, without a NUL.
pub(crate) fn window_text(window: HWND) -> Vec<u16> {
    let mut buffer = [0u16; 256];
    // SAFETY: `buffer` has room for the length given.
    let length = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    buffer[..length.max(0) as usize].to_vec()
}

/// Every top-level window of `class`, of any process.
pub(crate) fn windows_of_class(class: &str) -> Vec<HWND> {
    let class = wide(class);
    let mut found = Vec::new();
    let mut after: HWND = ptr::null_mut();
    loop {
        // SAFETY: `class` is NUL-terminated and outlives the call.
        after = unsafe { FindWindowExW(ptr::null_mut(), after, class.as_ptr(), ptr::null()) };
        if after.is_null() {
            return found;
        }
        found.push(after);
    }
}

/// Whether the process `pid` runs with administrator rights. `true` when that can't be told.
pub(crate) fn is_elevated(pid: u32) -> bool {
    let Some(process) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return true;
    };
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is open and `token` a live out pointer.
    if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &raw mut token) } == 0 {
        return true;
    }
    // SAFETY: a handle `OpenProcessToken` just opened, which nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut info = TOKEN_ELEVATION { TokenIsElevated: 1 };
    let mut got = 0;
    // SAFETY: `info` is a live TOKEN_ELEVATION of the size given; `token` is open.
    let read = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut info).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &raw mut got,
        )
    };
    read == 0 || info.TokenIsElevated != 0
}
