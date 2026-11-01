//! The notification area, for when Tile is the shell.
//!
//! Apps add their tray icons with `Shell_NotifyIcon`, which finds the window of class
//! `Shell_TrayWnd` and sends it a `WM_COPYDATA` message. Explorer owns that window; with no
//! Explorer, [`Host`] creates it and keeps the icons in a [`Registry`] the launcher lists. A
//! click is passed on to the icon's app the way Explorer would.

mod registry;
mod wire;

use std::{cell::RefCell, io, ptr, slice};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::DataExchange::COPYDATASTRUCT,
    UI::WindowsAndMessaging::{
        ASFW_ANY, AllowSetForegroundWindow, CHANGEFILTERSTRUCT, ChangeWindowMessageFilterEx,
        DefWindowProcW, DestroyWindow, HWND_BROADCAST, KillTimer, MSG, MSGFLT_ALLOW, PostMessageW,
        PostQuitMessage, RegisterWindowMessageW, SetTimer, WM_CLOSE, WM_COPYDATA, WM_DESTROY,
        WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW,
    },
};

pub use self::registry::{Click, IconImage, ItemKey, Registry, TrayItem};
use self::wire::{COPYDATA_APPBAR, COPYDATA_NOTIFY, COPYDATA_RECT, hex};
use crate::{
    thread::{MessageThread, Pump, WindowClass},
    win::{is_window, wide, windows_of_class},
};

/// The window class apps look for.
const TRAY_CLASS: &str = "Shell_TrayWnd";

/// How often icons whose app has gone are dropped.
const PRUNE_MS: u32 = 2000;
const PRUNE_TIMER: usize = 1;

thread_local! {
    static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) };
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            // SAFETY: for WM_COPYDATA, `lparam` points to a COPYDATASTRUCT valid for this call,
            // whose `lpData` holds `cbData` bytes.
            let (kind, data) = unsafe {
                let cds = &*(lparam as *const COPYDATASTRUCT);
                let data = (!cds.lpData.is_null())
                    .then(|| slice::from_raw_parts(cds.lpData.cast::<u8>(), cds.cbData as usize));
                (cds.dwData, data)
            };
            let Some(data) = data else {
                return 0;
            };
            REGISTRY
                .with(|r| {
                    r.borrow().as_ref().map(|registry| match kind {
                        COPYDATA_NOTIFY => LRESULT::from(registry.apply(data)),
                        COPYDATA_RECT => registry.icon_rect(data),
                        COPYDATA_APPBAR => registry.app_bar(data),
                        _ => {
                            // Not a tray message: something asking the taskbar (an app bar, a
                            // menu) that Tile doesn't answer. Logged to see what is asked.
                            tracing::info!(
                                "unanswered taskbar message: dwData {kind}, {} bytes: {}",
                                data.len(),
                                hex(data)
                            );
                            0
                        }
                    })
                })
                .unwrap_or(0)
        }
        WM_TIMER if wparam == PRUNE_TIMER => {
            REGISTRY.with(|r| {
                if let Some(registry) = r.borrow().as_ref() {
                    registry.prune();
                }
            });
            0
        }
        WM_CLOSE => {
            // SAFETY: destroys the window this procedure belongs to.
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe {
                KillTimer(hwnd, PRUNE_TIMER);
                PostQuitMessage(0);
            }
            0
        }
        // SAFETY: the default handling of the same arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Lets any process bring a window to the front until the next input, so a tray icon's app can
/// show its menu once the launcher has closed.
pub fn allow_foreground() {
    // SAFETY: takes a constant.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

/// Whether a taskbar (Explorer's, or another shell's) is running.
pub fn taskbar_exists() -> bool {
    !windows_of_class(TRAY_CLASS).is_empty()
}

/// The notification area's window, on its own thread.
pub struct Host {
    /// The window, which tests send to as an app would.
    #[cfg_attr(not(test), expect(dead_code))]
    hwnd: isize,
    thread: MessageThread,
}

impl Host {
    /// Starts hosting the notification area, filling `registry`. `None` if a taskbar is already
    /// running (Explorer's), or the window couldn't be made.
    pub fn start(registry: Registry) -> Option<Self> {
        if taskbar_exists() {
            return None;
        }
        Self::start_as(registry, TRAY_CLASS).ok()
    }

    /// [`start`](Self::start) with a window class of your choosing, which tests use so they
    /// don't stand in for the real taskbar.
    fn start_as(registry: Registry, class: &'static str) -> io::Result<Self> {
        let (thread, hwnd) =
            MessageThread::spawn("notification area", move || Tray::new(registry, class))?;
        Ok(Self { hwnd, thread })
    }

    /// Whether the window is still there. It goes when another taskbar appears.
    pub fn running(&self) -> bool {
        self.thread.running()
    }

    /// Closes the window and stops the thread.
    pub fn stop(self) {}
}

/// The notification area's window, on its thread.
struct Tray {
    registry: Registry,
    class_name: &'static str,
    window: HWND,
    /// Dropped after the window is destroyed.
    _class: WindowClass,
}

impl Tray {
    fn new(registry: Registry, class_name: &'static str) -> io::Result<(Self, isize)> {
        REGISTRY.with(|r| *r.borrow_mut() = Some(registry.clone()));
        let class = WindowClass::register(
            class_name,
            WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                ..Default::default()
            },
        );
        let window = class.create(WS_EX_TOOLWINDOW)?;
        // An elevated Tile would otherwise never hear from the apps of a lower integrity level.
        // SAFETY: `window` is the window just made; the filter struct is optional.
        unsafe {
            ChangeWindowMessageFilterEx(
                window,
                WM_COPYDATA,
                MSGFLT_ALLOW,
                ptr::null_mut::<CHANGEFILTERSTRUCT>(),
            );
            SetTimer(window, PRUNE_TIMER, PRUNE_MS, None);
        }
        // Apps that were running add their icons again when told a taskbar exists.
        let created = wide("TaskbarCreated");
        // SAFETY: `created` is NUL-terminated; broadcasting a registered message with no data.
        unsafe {
            let message = RegisterWindowMessageW(created.as_ptr());
            PostMessageW(HWND_BROADCAST, message, 0, 0);
        }
        let tray = Self {
            registry,
            class_name,
            window,
            _class: class,
        };
        Ok((tray, window as isize))
    }
}

impl Pump for Tray {
    fn message(&mut self, message: &MSG) -> bool {
        // A real taskbar took over: leave the apps to it.
        if message.message == WM_TIMER
            && windows_of_class(self.class_name)
                .into_iter()
                .any(|w| w != self.window)
        {
            // SAFETY: closes our window, ending the loop.
            unsafe { PostMessageW(self.window, WM_CLOSE, 0, 0) };
        }
        false
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if is_window(self.window) {
            // SAFETY: the window this thread made, destroyed once.
            unsafe { DestroyWindow(self.window) };
        }
        self.registry.clear();
        REGISTRY.with(|r| r.borrow_mut().take());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        registry::ICON_RECT,
        wire::{build::*, *},
        *,
    };
    use crate::win::is_elevated;

    /// Sends `data` the way an app does: a `WM_COPYDATA` to the window.
    fn send(class: &str, data: &[u8]) -> bool {
        send_kind(class, COPYDATA_NOTIFY, data) != 0
    }

    fn send_kind(class: &str, kind: usize, data: &[u8]) -> isize {
        use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
        let hwnd = *windows_of_class(class)
            .first()
            .unwrap_or_else(|| panic!("no window of class {class}"));
        let cds = COPYDATASTRUCT {
            dwData: kind,
            cbData: data.len() as u32,
            lpData: data.as_ptr().cast_mut().cast(),
        };
        // SAFETY: the COPYDATASTRUCT describes `data`, which outlives the synchronous
        // SendMessage.
        unsafe { SendMessageW(hwnd, WM_COPYDATA, 0, (&raw const cds) as isize) }
    }

    #[test]
    fn a_host_keeps_the_icons_apps_send() {
        const CLASS: &str = "TileNotifyTest";
        let registry = Registry::default();
        let host = Host::start_as(registry.clone(), CLASS).expect("the host starts");
        assert!(host.running());

        // A window that exists, to stand in for the app's: the host's own.
        let own = host.hwnd as i32;
        let flags = NIF_MESSAGE | NIF_TIP;
        assert!(send(CLASS, &message(NIM_ADD, own, 1, flags, 0x8001, "One")));
        assert!(send(CLASS, &message(NIM_ADD, own, 2, flags, 0x8002, "Two")));
        let names: Vec<String> = registry.items().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["One", "Two"]);

        assert!(send(CLASS, &message(NIM_MODIFY, own, 1, NIF_TIP, 0, "Uno")));
        assert!(!send(
            CLASS,
            &message(NIM_MODIFY, own, 9, NIF_TIP, 0, "none")
        ));
        // App bar questions: the quiet ones succeed, and the taskbar's position is written into
        // the sender's block, here this process's, unless it has administrator rights.
        let own_pid = std::process::id();
        let mut block = [0u8; 64];
        let at = block.as_mut_ptr() as u64;
        let pos = appbar_message(ABM_GETTASKBARPOS, at, own_pid);
        assert_eq!(send_kind(CLASS, COPYDATA_APPBAR, &pos), 1);
        let edge = u32::from_le_bytes(block[28..32].try_into().unwrap());
        let width = i32::from_le_bytes(block[40..44].try_into().unwrap())
            - i32::from_le_bytes(block[32..36].try_into().unwrap());
        if is_elevated(own_pid) {
            assert_eq!(edge, 0);
        } else {
            assert_eq!((edge, width), (EDGE_BOTTOM, ICON_RECT));
        }
        let state = appbar_message(ABM_GETSTATE, 0, own_pid);
        assert_eq!(send_kind(CLASS, COPYDATA_APPBAR, &state), 0);
        let register = appbar_message(ABM_NEW, 0, own_pid);
        assert_eq!(send_kind(CLASS, COPYDATA_APPBAR, &register), 1);
        // Where the icon is is asked in two halves; the size is the same wherever it is.
        let size = send_kind(CLASS, COPYDATA_RECT, &rect_question(RECT_SIZE, own, 1));
        assert_eq!(size, pack_pair(ICON_RECT, ICON_RECT));
        assert_ne!(
            send_kind(CLASS, COPYDATA_RECT, &rect_question(RECT_POSITION, own, 1)),
            0
        );
        let unknown = send_kind(CLASS, COPYDATA_RECT, &rect_question(RECT_SIZE, own, 9));
        assert_eq!(unknown, 0);
        assert!(send(CLASS, &message(NIM_DELETE, own, 2, 0, 0, "")));
        let names: Vec<String> = registry.items().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["Uno"]);

        host.stop();
        assert!(registry.items().is_empty());
    }
}
