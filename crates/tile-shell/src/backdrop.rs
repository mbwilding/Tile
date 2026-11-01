//! A black window at the bottom of the stack on each monitor, for when no Explorer is there to
//! paint the desktop. The system's own desktop layer can't be relied on: it keeps showing what
//! Explorer last left there, whatever the wallpaper or desktop colour is set to.

use std::{
    io, ptr,
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_BOTTOM,
        IDC_ARROW, LoadCursorW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassW,
        SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, TranslateMessage, UnregisterClassW, WM_APP,
        WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    },
};

const CLASS: &str = "tile-backdrop";
const WM_SYNC: u32 = WM_APP;

/// A monitor's rectangle: left, top, width and height in pixels.
pub type Area = [i32; 4];

/// The black windows, on a thread of their own. Dropping it removes them.
pub struct Backdrop {
    areas: Arc<Mutex<Vec<Area>>>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Backdrop {
    /// Covers `areas`, one window each.
    pub fn start(areas: Vec<Area>) -> io::Result<Self> {
        let areas = Arc::new(Mutex::new(areas));
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new().name("backdrop".into()).spawn({
            let areas = areas.clone();
            move || run(&areas, &ready_tx)
        })?;
        match ready_rx.recv() {
            Ok(thread_id) => Ok(Self {
                areas,
                thread_id,
                thread: Some(thread),
            }),
            Err(_) => Err(io::Error::other("the backdrop thread exited")),
        }
    }

    /// Moves the windows to cover `areas`, after the monitors changed.
    pub fn set_areas(&self, areas: Vec<Area>) {
        {
            let mut current = self.areas.lock().unwrap_or_else(PoisonError::into_inner);
            if *current == areas {
                return;
            }
            *current = areas;
        }
        // SAFETY: posting a message with no pointers to a thread id; if the thread has gone the
        // call just fails.
        unsafe { PostThreadMessageW(self.thread_id, WM_SYNC, 0, 0) };
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        // SAFETY: as in `set_areas`.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Makes the windows match `areas`: one each, sent to the bottom of the stack without taking
/// focus.
fn sync(windows: &mut Vec<HWND>, areas: &[Area], class: &[u16]) {
    while windows.len() > areas.len() {
        if let Some(hwnd) = windows.pop() {
            // SAFETY: a window this thread made, destroyed once.
            unsafe { DestroyWindow(hwnd) };
        }
    }
    while windows.len() < areas.len() {
        // SAFETY: the class was registered by `run`; the strings are NUL-terminated.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            break;
        }
        windows.push(hwnd);
    }
    for (&hwnd, &[x, y, w, h]) in windows.iter().zip(areas) {
        // SAFETY: a window this thread made.
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
    }
}

fn run(areas: &Mutex<Vec<Area>>, ready: &mpsc::Sender<u32>) {
    let class = wide(CLASS);
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG to write to; this creates the thread's message queue, so the
    // messages posted to it aren't lost.
    unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_NOREMOVE) };
    // SAFETY: a null module name gives our own exe's handle.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    // SAFETY: the black brush is a stock object that is never deleted.
    let black = unsafe { GetStockObject(BLACK_BRUSH) } as HBRUSH;
    // SAFETY: a null instance with a system cursor id loads a shared cursor that needs no freeing.
    let arrow = unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) };
    let wc = WNDCLASSW {
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: instance,
        hCursor: arrow,
        hbrBackground: black,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialised.
    unsafe { RegisterClassW(&raw const wc) };
    // SAFETY: no arguments.
    let _ = ready.send(unsafe { GetCurrentThreadId() });

    let mut windows = Vec::new();
    let current =
        |areas: &Mutex<Vec<Area>>| areas.lock().unwrap_or_else(PoisonError::into_inner).clone();
    sync(&mut windows, &current(areas), &class);
    // SAFETY: `msg` is a valid MSG to write to; the loop ends on WM_QUIT.
    while unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) } > 0 {
        if msg.hwnd.is_null() && msg.message == WM_SYNC {
            sync(&mut windows, &current(areas), &class);
            continue;
        }
        // SAFETY: `msg` was just filled in by GetMessage.
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    sync(&mut windows, &[], &class);
    // SAFETY: the class belongs to this module and has no windows left.
    unsafe { UnregisterClassW(class.as_ptr(), instance) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId};

    #[test]
    fn covers_areas_and_goes_away() {
        let backdrop = Backdrop::start(vec![[0, 0, 10, 10]]).expect("starts");
        backdrop.set_areas(vec![[0, 0, 10, 10], [20, 0, 10, 10]]);
        backdrop.set_areas(Vec::new());
        drop(backdrop);
        // Another Tile may be running with windows of the class: only ours count.
        let class = wide(CLASS);
        let own = std::process::id();
        let mut after: HWND = ptr::null_mut();
        loop {
            // SAFETY: the strings are NUL-terminated; `pid` is a live u32 to write to.
            let (next, pid) = unsafe {
                let next = FindWindowExW(ptr::null_mut(), after, class.as_ptr(), ptr::null());
                let mut pid = 0;
                GetWindowThreadProcessId(next, &raw mut pid);
                (next, pid)
            };
            if next.is_null() {
                break;
            }
            assert_ne!(pid, own, "a backdrop window outlived it");
            after = next;
        }
    }
}
