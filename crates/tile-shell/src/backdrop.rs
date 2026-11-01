//! A black window at the bottom of the stack on each monitor, for when no Explorer is there to
//! paint the desktop. The system's own desktop layer can't be relied on: it keeps showing what
//! Explorer last left there, whatever the wallpaper or desktop colour is set to.

use std::{
    io, ptr,
    sync::{Arc, Mutex, PoisonError},
};

use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH},
    UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, HWND_BOTTOM, IDC_ARROW, LoadCursorW, MSG, SWP_NOACTIVATE,
        SWP_SHOWWINDOW, SetWindowPos, WM_APP, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    },
};

use crate::thread::{MessageThread, Pump, WindowClass};

const CLASS: &str = "tile-backdrop";
const WM_SYNC: u32 = WM_APP;

/// A monitor's rectangle: left, top, width and height in pixels.
pub type Area = [i32; 4];

/// The black windows, on a thread of their own. Dropping it removes them.
pub struct Backdrop {
    areas: Arc<Mutex<Vec<Area>>>,
    thread: MessageThread,
}

impl Backdrop {
    /// Covers `areas`, one window each.
    pub fn start(areas: Vec<Area>) -> io::Result<Self> {
        let areas = Arc::new(Mutex::new(areas));
        let (thread, ()) = MessageThread::spawn("backdrop", {
            let areas = areas.clone();
            move || Ok((Windows::new(areas), ()))
        })?;
        Ok(Self { areas, thread })
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
        self.thread.post(WM_SYNC);
    }
}

/// The backdrop thread's windows, one per area.
struct Windows {
    areas: Arc<Mutex<Vec<Area>>>,
    windows: Vec<HWND>,
    class: WindowClass,
}

impl Windows {
    fn new(areas: Arc<Mutex<Vec<Area>>>) -> Self {
        // SAFETY: the black brush is a stock object that is never deleted; a null instance with
        // a system cursor id loads a shared cursor that needs no freeing.
        let (black, arrow) = unsafe {
            (
                GetStockObject(BLACK_BRUSH) as HBRUSH,
                LoadCursorW(ptr::null_mut(), IDC_ARROW),
            )
        };
        let class = WindowClass::register(
            CLASS,
            WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hCursor: arrow,
                hbrBackground: black,
                ..Default::default()
            },
        );
        let mut windows = Self {
            areas,
            windows: Vec::new(),
            class,
        };
        windows.sync();
        windows
    }

    /// Makes the windows match the areas: one each, sent to the bottom of the stack without
    /// taking focus.
    fn sync(&mut self) {
        let areas = self
            .areas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        self.resize(&areas);
    }

    fn resize(&mut self, areas: &[Area]) {
        while self.windows.len() > areas.len() {
            if let Some(hwnd) = self.windows.pop() {
                // SAFETY: a window this thread made, destroyed once.
                unsafe { DestroyWindow(hwnd) };
            }
        }
        while self.windows.len() < areas.len() {
            let Ok(hwnd) = self.class.create(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) else {
                break;
            };
            self.windows.push(hwnd);
        }
        for (&hwnd, &[x, y, w, h]) in self.windows.iter().zip(areas) {
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
}

impl Pump for Windows {
    fn message(&mut self, message: &MSG) -> bool {
        let sync = message.hwnd.is_null() && message.message == WM_SYNC;
        if sync {
            self.sync();
        }
        sync
    }
}

impl Drop for Windows {
    fn drop(&mut self) {
        self.resize(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::{pid_of, windows_of_class};

    #[test]
    fn covers_areas_and_goes_away() {
        let backdrop = Backdrop::start(vec![[0, 0, 10, 10]]).expect("starts");
        backdrop.set_areas(vec![[0, 0, 10, 10], [20, 0, 10, 10]]);
        backdrop.set_areas(Vec::new());
        drop(backdrop);
        // Another Tile may be running with windows of the class: only ours count.
        let own = std::process::id();
        for window in windows_of_class(CLASS) {
            assert_ne!(pid_of(window), own, "a backdrop window outlived it");
        }
    }
}
