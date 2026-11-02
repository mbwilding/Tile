//! A window at the bottom of the stack on each monitor, for when no Explorer is there to paint
//! the desktop: a plain colour, or a picture over it. The system's own desktop layer can't be
//! relied on: it keeps showing what Explorer last left there, whatever the wallpaper or desktop
//! colour is set to.

mod fit;
mod image;

use std::{
    io,
    path::{Path, PathBuf},
    ptr,
    sync::{Arc, Mutex, PoisonError},
};

use windows::Win32::{
    Graphics::Imaging::IWICImagingFactory,
    System::Com::{COINIT_MULTITHREADED, CoInitializeEx},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, BitBlt, CreateBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC,
        DeleteObject, EndPaint, HBITMAP, HBRUSH, InvalidateRect, PAINTSTRUCT, SRCCOPY,
        SelectObject,
    },
    UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, GCLP_HBRBACKGROUND, GWLP_USERDATA, GetWindowLongPtrW,
        HWND_BOTTOM, IDC_ARROW, LoadCursorW, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetClassLongPtrW,
        SetWindowLongPtrW, SetWindowPos, WM_APP, WM_ERASEBKGND, WM_PAINT, WNDCLASSW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    },
};

pub use fit::Fill;

use crate::thread::{MessageThread, Pump, WindowClass};

const CLASS: &str = "tile-backdrop";
const WM_SYNC: u32 = WM_APP;

/// A monitor's rectangle: left, top, width and height in pixels.
pub type Area = [i32; 4];

/// What the windows show: a colour, with a picture over it if there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    /// Red, green and blue of the plain colour.
    pub color: [u8; 3],
    /// The picture to show. One that can't be read leaves the plain colour.
    pub image: Option<PathBuf>,
    /// How the picture is fitted to each monitor.
    pub fill: Fill,
}

impl Look {
    /// A plain colour.
    pub fn plain(color: [u8; 3]) -> Self {
        Self {
            color,
            image: None,
            fill: Fill::default(),
        }
    }
}

/// What the thread is asked to show.
struct Wanted {
    areas: Vec<Area>,
    look: Look,
}

/// The windows, on a thread of their own. Dropping it removes them.
pub struct Backdrop {
    wanted: Arc<Mutex<Wanted>>,
    thread: MessageThread,
}

impl Backdrop {
    /// Covers `areas` with `look`, one window each.
    pub fn start(areas: Vec<Area>, look: Look) -> io::Result<Self> {
        let wanted = Arc::new(Mutex::new(Wanted { areas, look }));
        let (thread, ()) = MessageThread::spawn("backdrop", {
            let wanted = wanted.clone();
            move || Ok((Windows::new(wanted), ()))
        })?;
        Ok(Self { wanted, thread })
    }

    /// Moves the windows to cover `areas`, after the monitors changed.
    pub fn set_areas(&self, areas: Vec<Area>) {
        self.change(|w| std::mem::replace(&mut w.areas, areas.clone()) != areas);
    }

    /// Shows `look` instead.
    pub fn set_look(&self, look: Look) {
        self.change(|w| std::mem::replace(&mut w.look, look.clone()) != look);
    }

    /// Applies `edit`, which says whether it changed anything, and has the thread catch up.
    fn change(&self, edit: impl FnOnce(&mut Wanted) -> bool) {
        let changed = edit(&mut self.wanted.lock().unwrap_or_else(PoisonError::into_inner));
        if changed {
            self.thread.post(WM_SYNC);
        }
    }
}

/// The backdrop thread's windows, one per area.
struct Windows {
    wanted: Arc<Mutex<Wanted>>,
    windows: Vec<HWND>,
    /// The class's background: the plain colour.
    brush: HBRUSH,
    brush_color: [u8; 3],
    imaging: Option<IWICImagingFactory>,
    /// The image that couldn't be read, so that is said once.
    failed: Option<PathBuf>,
    class: WindowClass,
}

/// A window's bitmap, kept in its user data: the picture, drawn instead of the class's colour.
fn bitmap_of(hwnd: HWND) -> HBITMAP {
    // SAFETY: reads a window's user data, which only this module sets, to a bitmap or nothing.
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HBITMAP }
}

/// Draws the picture if the window has one, else leaves the colour to the class's brush.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let bitmap = bitmap_of(hwnd);
    if bitmap.is_null() {
        // SAFETY: forwards the arguments it was called with.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    match message {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: a paint cycle on this window, with a memory context made and freed here
            // and the bitmap alive for as long as the window holds it.
            unsafe {
                let dc = BeginPaint(hwnd, &raw mut paint);
                let memory = CreateCompatibleDC(dc);
                let previous = SelectObject(memory, bitmap);
                let r = paint.rcPaint;
                BitBlt(
                    dc,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    memory,
                    r.left,
                    r.top,
                    SRCCOPY,
                );
                SelectObject(memory, previous);
                DeleteDC(memory);
                EndPaint(hwnd, &raw const paint);
            }
            0
        }
        // SAFETY: forwards the arguments it was called with.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn solid(color: [u8; 3]) -> HBRUSH {
    let [r, g, b] = color;
    // SAFETY: takes a plain colour; the brush is deleted when replaced and on drop.
    unsafe { CreateSolidBrush(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16) }
}

impl Windows {
    fn new(wanted: Arc<Mutex<Wanted>>) -> Self {
        // SAFETY: initialises COM for this thread, for the imaging factory; a null instance with
        // a system cursor id loads a shared cursor that needs no freeing.
        let arrow = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            LoadCursorW(ptr::null_mut(), IDC_ARROW)
        };
        let brush_color = wanted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .look
            .color;
        let brush = solid(brush_color);
        let class = WindowClass::register(
            CLASS,
            WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hCursor: arrow,
                hbrBackground: brush,
                ..Default::default()
            },
        );
        let mut windows = Self {
            wanted,
            windows: Vec::new(),
            brush,
            brush_color,
            imaging: None,
            failed: None,
            class,
        };
        windows.sync();
        windows
    }

    /// Makes the windows match what is wanted: one each, sent to the bottom of the stack without
    /// taking focus, showing the look.
    fn sync(&mut self) {
        let (areas, look) = {
            let wanted = self.wanted.lock().unwrap_or_else(PoisonError::into_inner);
            (wanted.areas.clone(), wanted.look.clone())
        };
        self.resize(&areas);
        self.paint(&areas, &look);
    }

    fn resize(&mut self, areas: &[Area]) {
        while self.windows.len() > areas.len() {
            if let Some(hwnd) = self.windows.pop() {
                self.set_bitmap(hwnd, ptr::null_mut());
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

    /// Gives `hwnd` the bitmap (or none, for the plain colour) and frees the one it had.
    fn set_bitmap(&self, hwnd: HWND, bitmap: HBITMAP) {
        let old = bitmap_of(hwnd);
        // SAFETY: a window this thread made, whose user data is only ever a bitmap this thread
        // made, deleted here once it is no longer the window's.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, bitmap as isize);
            if !old.is_null() {
                DeleteObject(old);
            }
            InvalidateRect(hwnd, ptr::null(), 1);
        }
    }

    /// Paints `look` on the windows over `areas`.
    fn paint(&mut self, areas: &[Area], look: &Look) {
        if look.color != self.brush_color
            && let Some(&hwnd) = self.windows.first()
        {
            let brush = solid(look.color);
            // SAFETY: the class's background brush is ours, replaced by a new one and then
            // deleted; the windows paint with whichever is current.
            unsafe {
                SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, brush as isize);
                DeleteObject(self.brush);
            }
            self.brush = brush;
            self.brush_color = look.color;
        }
        let picture = look.image.as_deref().and_then(|path| self.open(path));
        for (&hwnd, &[_, _, w, h]) in self.windows.iter().zip(areas) {
            let [r, g, b] = look.color;
            let size = (w.max(1) as u32, h.max(1) as u32);
            let pixels = picture.as_ref().and_then(|(factory, picture)| {
                picture
                    .render(factory, look.fill, [b, g, r], size)
                    .inspect_err(|e| tracing::warn!("couldn't draw the wallpaper: {e}"))
                    .ok()
            });
            let bitmap = pixels.map_or(ptr::null_mut(), |pixels| {
                // SAFETY: `pixels` is `size.1` rows of `size.0` 32-bit pixels, which is what the
                // call reads to fill the new bitmap.
                unsafe { CreateBitmap(size.0 as i32, size.1 as i32, 1, 32, pixels.as_ptr().cast()) }
            });
            self.set_bitmap(hwnd, bitmap);
        }
    }

    /// Opens the image, saying once if it can't be.
    fn open(&mut self, path: &Path) -> Option<(IWICImagingFactory, image::Picture)> {
        let opened = (|| {
            if self.imaging.is_none() {
                self.imaging = Some(image::factory()?);
            }
            let factory = self.imaging.clone().ok_or("no imaging factory")?;
            image::Picture::open(&factory, path).map(|picture| (factory, picture))
        })();
        match opened {
            Ok(opened) => {
                self.failed = None;
                Some(opened)
            }
            Err(e) => {
                if self.failed.as_deref() != Some(path) {
                    tracing::warn!("can't show the wallpaper {}: {e}", path.display());
                    self.failed = Some(path.to_path_buf());
                }
                None
            }
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
        // SAFETY: no window is left to use the brush.
        unsafe { DeleteObject(self.brush) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::{pid_of, windows_of_class};

    #[test]
    fn covers_areas_and_goes_away() {
        let backdrop =
            Backdrop::start(vec![[0, 0, 10, 10]], Look::plain([0, 0, 0])).expect("starts");
        backdrop.set_areas(vec![[0, 0, 10, 10], [20, 0, 10, 10]]);
        backdrop.set_look(Look::plain([10, 20, 30]));
        backdrop.set_look(Look {
            image: Some(PathBuf::from("no such picture.png")),
            ..Look::plain([10, 20, 30])
        });
        backdrop.set_areas(Vec::new());
        drop(backdrop);
        // Another Tile may be running with windows of the class: only ours count.
        let own = std::process::id();
        for window in windows_of_class(CLASS) {
            assert_ne!(pid_of(window), own, "a backdrop window outlived it");
        }
    }
}
