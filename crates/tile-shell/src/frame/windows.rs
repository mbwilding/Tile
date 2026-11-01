//! The window side of framing: the shell window and the cloak it lets this process lift, and
//! telling Store app windows and their frames apart.

use std::{os::windows::io::AsRawHandle, ptr};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    Graphics::Dwm::{DwmGetWindowAttribute, DwmSetWindowAttribute},
    Storage::Packaging::Appx::{GetApplicationUserModelId, GetPackageFullName},
    System::{
        LibraryLoader::{GetProcAddress, LoadLibraryW},
        Threading::{PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess},
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, EnumChildWindows, EnumWindows, GetClientRect, GetParent, GetSystemMetrics,
        PostMessageW, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
        SetWindowPos, SetWindowTextW, ShowWindow, WM_CLOSE,
    },
};

use crate::win::{class_name, file_name, image_path, open_process, pid_of, wide, window_text};

const DWMWA_CLOAK: u32 = 13;
const DWMWA_CLOAKED: u32 = 14;
const DWM_CLOAKED_SHELL: u32 = 2;

/// A Store app's window.
const APP_CLASS: &str = "Windows.UI.Core.CoreWindow";
/// A Store app's frame.
const FRAME_CLASS: &str = "ApplicationFrameWindow";

/// Shell hosts whose hidden windows must be left alone.
const SHELL_HOSTS: [&str; 8] = [
    "applicationframehost.exe",
    "lockapp.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "textinputhost.exe",
    "widgets.exe",
    "widgetservice.exe",
];

#[link(name = "user32")]
unsafe extern "system" {
    /// Registers `window` as the shell's, if the desktop has none yet.
    fn SetShellWindow(window: HWND) -> i32;
}

/// `NtUserAcquireIAMKey(key*)`.
type Acquire = unsafe extern "system" fn(*mut usize) -> i32;
/// `NtUserEnableIAMAccess(key*, enable)`.
type Enable = unsafe extern "system" fn(*const usize, i32) -> i32;

/// The shell window this process registered, and the Immersive App Management key that came
/// with it, which lets it lift the shell's cloak.
pub(super) struct Shell {
    key: usize,
    enable: Enable,
}

impl Shell {
    /// Registers a hidden window as the shell window and takes the key. Call from the thread
    /// that pumps that window's messages.
    pub(super) fn take() -> Result<Self, String> {
        let class = wide("STATIC");
        // SAFETY: the class is a built-in one; the arguments make a 1x1 window with no parent.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                1,
                1,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if window.is_null() {
            return Err("couldn't make the shell window".into());
        }
        // SAFETY: `window` was just created on this thread.
        if unsafe { SetShellWindow(window) } == 0 {
            return Err("another process is the shell".into());
        }
        let library = wide("win32u.dll");
        // SAFETY: loading a system library by name. The exports are looked up rather than
        // linked, so an update that drops them stops only this, not the whole program.
        let module = unsafe { LoadLibraryW(library.as_ptr()) };
        // SAFETY: both exports have these signatures in Windows 11 build 26100.
        let (acquire, enable) = unsafe {
            let acquire = GetProcAddress(module, c"NtUserAcquireIAMKey".as_ptr().cast())
                .ok_or("win32u has no NtUserAcquireIAMKey")?;
            let enable = GetProcAddress(module, c"NtUserEnableIAMAccess".as_ptr().cast())
                .ok_or("win32u has no NtUserEnableIAMAccess")?;
            (
                std::mem::transmute::<unsafe extern "system" fn() -> isize, Acquire>(acquire),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, Enable>(enable),
            )
        };
        let mut key = 0usize;
        // SAFETY: `key` is a live out pointer.
        if unsafe { acquire(&raw mut key) } == 0 {
            return Err("Windows refused the Immersive App Management key".into());
        }
        Ok(Self { key, enable })
    }

    /// Lifts the shell's cloak from `windows`.
    pub(super) fn uncloak(&self, windows: &[HWND]) {
        // SAFETY: `self.key` is the key `take` was given, passed by pointer as the call expects.
        unsafe { (self.enable)(&raw const self.key, 1) };
        for &window in windows {
            let clear = 0i32;
            // SAFETY: `clear` outlives the call and is the 4 bytes DWMWA_CLOAK takes.
            unsafe {
                DwmSetWindowAttribute(window, DWMWA_CLOAK as _, (&raw const clear).cast(), 4)
            };
        }
        // SAFETY: as above.
        unsafe { (self.enable)(&raw const self.key, 0) };
    }
}

/// Every top-level window.
pub(super) fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> i32 {
        // SAFETY: `list` is the `Vec` passed below, alive for the whole enumeration.
        unsafe { (*(list as *mut Vec<HWND>)).push(window) };
        1
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: `collect` only touches the `Vec` behind the pointer.
    unsafe { EnumWindows(Some(collect), (&raw mut windows) as LPARAM) };
    windows
}

/// Whether `window` is a Store app frame.
pub(super) fn is_frame(window: HWND) -> bool {
    class_name(window) == FRAME_CLASS
}

/// Whether `frame` has a Store app's window inside it.
pub(super) fn holds_app(frame: HWND) -> bool {
    unsafe extern "system" fn look(window: HWND, found: LPARAM) -> i32 {
        if class_name(window) != APP_CLASS {
            return 1;
        }
        // SAFETY: `found` is the flag passed below, alive for the whole enumeration.
        unsafe { *(found as *mut bool) = true };
        0
    }
    let mut found = false;
    // SAFETY: `look` only touches the flag behind the pointer.
    unsafe { EnumChildWindows(frame, Some(look), (&raw mut found) as LPARAM) };
    found
}

/// Whether `window` is a Store app's window that the shell is still hiding, with nothing around
/// it yet.
pub(super) fn is_hidden_app_window(window: HWND) -> bool {
    if class_name(window) != APP_CLASS {
        return false;
    }
    let mut cloak = 0u32;
    // SAFETY: `cloak` is the 4 bytes DWMWA_CLOAKED fills in.
    let read =
        unsafe { DwmGetWindowAttribute(window, DWMWA_CLOAKED as _, (&raw mut cloak).cast(), 4) };
    // SAFETY: `GetParent` accepts any handle value.
    read >= 0 && cloak & DWM_CLOAKED_SHELL != 0 && unsafe { GetParent(window) }.is_null()
}

/// The app user model id and package full name of the Store app that owns `window`, unless it's
/// a shell host.
pub(super) fn app_id(window: HWND) -> Option<(String, String)> {
    let process = open_process(pid_of(window), PROCESS_QUERY_LIMITED_INFORMATION)?;
    let exe = file_name(&image_path(&process)?).to_lowercase();
    if SHELL_HOSTS.contains(&exe.as_str()) {
        return None;
    }
    let mut id = [0u16; 130];
    let mut id_length = id.len() as u32;
    let mut package = [0u16; 280];
    let mut package_length = package.len() as u32;
    // SAFETY: the buffers have room for the lengths given and `process` is open.
    let packaged = unsafe {
        GetApplicationUserModelId(process.as_raw_handle(), &raw mut id_length, id.as_mut_ptr()) == 0
            && GetPackageFullName(
                process.as_raw_handle(),
                &raw mut package_length,
                package.as_mut_ptr(),
            ) == 0
    };
    packaged.then(|| {
        (
            String::from_utf16_lossy(&id[..id_length.saturating_sub(1) as usize]),
            String::from_utf16_lossy(&package[..package_length.saturating_sub(1) as usize]),
        )
    })
}

/// Gives `frame` the title of the app window `app`, which is what Windows (and Tile) use to tell
/// windows apart and to decide which to manage, unless the app has none yet or `current`
/// already is it. Returns the app's title.
pub(super) fn copy_title(frame: HWND, app: HWND, current: &[u16]) -> Vec<u16> {
    let title = window_text(app);
    if !title.is_empty() && title != current {
        let mut terminated = title.clone();
        terminated.push(0);
        // SAFETY: `terminated` is NUL-terminated and outlives the call.
        unsafe { SetWindowTextW(frame, terminated.as_ptr()) };
    }
    title
}

/// Centres a new frame on the primary screen at a size that suits it, 1200x800 or three
/// quarters of a smaller screen, and shows it.
pub(super) fn place(frame: HWND) {
    let (screen_width, screen_height) = (screen(SM_CXSCREEN), screen(SM_CYSCREEN));
    let (width, height) = (
        1200.min(screen_width * 3 / 4),
        800.min(screen_height * 3 / 4),
    );
    // SAFETY: `frame` is a window this process made.
    unsafe {
        SetWindowPos(
            frame,
            ptr::null_mut(),
            (screen_width - width) / 2,
            (screen_height - height) / 2,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        ShowWindow(frame, SW_SHOW);
    }
}

/// The size of `window`'s client area.
pub(super) fn client_size(window: HWND) -> (i32, i32) {
    let mut client = RECT::default();
    // SAFETY: `client` is a live out pointer; a stale window leaves it empty.
    unsafe { GetClientRect(window, &raw mut client) };
    (client.right, client.bottom)
}

/// Hides `window`.
pub(super) fn hide(window: HWND) {
    // SAFETY: a plain hide; a stale window just makes it fail.
    unsafe { ShowWindow(window, SW_HIDE) };
}

/// Asks `window` to close.
pub(super) fn post_close(window: HWND) {
    // SAFETY: a plain message post; a stale window just makes it fail.
    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
}

/// Ends the process that owns `window`, for an app that didn't close when asked.
pub(super) fn end_process(window: HWND) {
    if let Some(process) = open_process(pid_of(window), PROCESS_TERMINATE) {
        // SAFETY: `process` is open with the right to end it.
        unsafe { TerminateProcess(process.as_raw_handle(), 0) };
    }
}

fn screen(metric: i32) -> i32 {
    // SAFETY: `GetSystemMetrics` takes no pointers.
    unsafe { GetSystemMetrics(metric) }
}
