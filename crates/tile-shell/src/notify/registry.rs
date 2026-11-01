//! The tray icons apps have added, and answering what apps ask the taskbar.

use std::{
    os::windows::io::AsRawHandle,
    ptr,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use windows_sys::Win32::{
    Foundation::{HWND, LRESULT, POINT},
    Graphics::Gdi::HDC,
    System::{
        Diagnostics::Debug::WriteProcessMemory,
        Threading::{PROCESS_VM_OPERATION, PROCESS_VM_WRITE},
    },
    UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, CopyIcon, DI_NORMAL, DestroyIcon, DrawIconEx, GetCursorPos,
        HICON, PostMessageW, WM_CONTEXTMENU, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
        WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
    },
};

use super::wire::{
    ABM_ACTIVATE, ABM_EDGE_AT, ABM_GETAUTOHIDEBAR, ABM_GETSTATE, ABM_GETTASKBARPOS, ABM_NEW,
    ABM_QUERYPOS, ABM_REMOVE, ABM_SETAUTOHIDEBAR, ABM_SETPOS, ABM_SETSTATE, ABM_WINDOWPOSCHANGED,
    EDGE_BOTTOM, NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_STATE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETFOCUS, NIM_SETVERSION, NIS_HIDDEN, RECT_POSITION, RECT_SIZE, hex, pack_pair,
    parse, parse_appbar, parse_rect_request,
};
use crate::{
    icon::{IconInfo, render_hicon},
    win::{file_name, is_elevated, is_window, open_process, pid_of, process_path},
};

/// The size reported for every icon, and how far its corner is from the pointer to centre it.
pub(super) const ICON_RECT: i32 = 24;

/// Sent to a version 4 icon's window for a left click, after the mouse messages.
const NIN_SELECT: u32 = 0x400;

/// A tray icon's identity: its app's window and the id the app gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemKey {
    /// The app's window, which hears about clicks on the icon.
    pub hwnd: isize,
    /// The id the app gave the icon, to tell its icons apart.
    pub id: u32,
}

/// A click on a tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// The primary button, which usually opens the app.
    Left,
    /// The secondary button, which usually shows the app's menu.
    Right,
    /// The middle button.
    Middle,
}

/// An icon handle we own, shared with whoever is drawing it, destroyed when the last user lets go.
#[derive(Debug)]
struct OwnedIcon(isize);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: we made this icon with CopyIcon and destroy it once, when the last `Arc` goes.
        unsafe { DestroyIcon(self.0 as HICON) };
    }
}

/// A tray icon's picture, cheap to clone.
#[derive(Debug, Clone)]
pub struct IconImage(Arc<OwnedIcon>);

impl IconImage {
    /// The icon handle's value, which differs for every icon an app sets.
    pub fn handle(&self) -> isize {
        self.0.0
    }

    /// The icon drawn at exactly `size` pixels, scaled smoothly, for drawing 1:1 without
    /// Windows stretching it.
    pub fn render(&self, size: i32) -> Option<crate::Icon> {
        render_hicon(self.0.0, size)
    }

    /// The icon's width in pixels as the app made it: a tray icon is usually 16, so drawing it
    /// bigger blurs it. 0 if it can't be read.
    pub fn size(&self) -> i32 {
        IconInfo::of(self.0.0)
            .and_then(|info| info.width())
            .unwrap_or(0)
    }

    /// Draws the icon at `x, y` on the device context `hdc`, scaled to `size` pixels.
    pub fn draw(&self, hdc: isize, x: i32, y: i32, size: i32) -> bool {
        // SAFETY: Windows validates `hdc`; the icon stays alive through `self`.
        unsafe {
            DrawIconEx(
                hdc as HDC,
                x,
                y,
                self.0.0 as HICON,
                size,
                size,
                0,
                ptr::null_mut(),
                DI_NORMAL,
            ) != 0
        }
    }
}

/// What the launcher lists for a tray icon.
#[derive(Debug, Clone)]
pub struct TrayItem {
    /// Which icon this is, to pass to [`Registry::click`].
    pub key: ItemKey,
    /// The tooltip, or the name of the app's program if it has none.
    pub name: String,
    /// The app's program, to take a sharper icon from when the tray icon is small.
    pub path: String,
    /// The picture, if the app gave one.
    pub icon: Option<IconImage>,
    /// [`IconImage::size`] of `icon`.
    pub icon_size: i32,
}

struct Entry {
    key: ItemKey,
    guid: Option<[u8; 16]>,
    callback: u32,
    version: u32,
    tip: String,
    program: String,
    path: String,
    icon: Option<IconImage>,
    hidden: bool,
}

impl Entry {
    /// Whether this is the icon an app names by `guid`, or else by its window and id.
    fn matches(&self, hwnd: isize, id: u32, guid: Option<[u8; 16]>) -> bool {
        match (guid, self.guid) {
            (Some(a), Some(b)) => a == b,
            _ => self.key.hwnd == hwnd && self.key.id == id,
        }
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry").field("key", &self.key).finish()
    }
}

/// The file name of the program at `path`, without `.exe`.
fn program_name(path: &str) -> String {
    let file = file_name(path);
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
        .to_owned()
}

/// Writes `bytes` at `address` in the process `pid`. Only into a process without administrator
/// rights: the sender names the process and address itself and can't be told apart from another
/// program, and Tile's own rights must not become a way to write into one that has more than
/// the sender. A program can already write into its peers, so nothing is gained from this.
fn write_to_sender(pid: u32, address: u64, bytes: &[u8]) -> bool {
    if is_elevated(pid) {
        return false;
    }
    let Some(process) = open_process(pid, PROCESS_VM_OPERATION | PROCESS_VM_WRITE) else {
        return false;
    };
    let mut written = 0;
    // SAFETY: `bytes` is a live slice of the length given and `process` is open. The address is
    // another process's, which the call checks.
    let ok = unsafe {
        WriteProcessMemory(
            process.as_raw_handle(),
            address as usize as *const core::ffi::c_void,
            bytes.as_ptr().cast(),
            bytes.len(),
            &raw mut written,
        )
    };
    ok != 0 && written == bytes.len()
}

/// The pointer's position on the screen.
fn cursor() -> POINT {
    let mut at = POINT { x: 0, y: 0 };
    // SAFETY: `at` is a live POINT to write to.
    unsafe { GetCursorPos(&raw mut at) };
    at
}

/// The tray icons apps have added. Cloning shares the list.
#[derive(Debug, Clone, Default)]
pub struct Registry(Arc<Mutex<Vec<Entry>>>);

impl Registry {
    fn lock(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The visible icons whose app is still there, oldest first.
    pub fn items(&self) -> Vec<TrayItem> {
        self.prune();
        self.lock()
            .iter()
            .filter(|e| !e.hidden)
            .map(|e| TrayItem {
                key: e.key,
                name: if e.tip.trim().is_empty() {
                    e.program.clone()
                } else {
                    e.tip.trim().to_owned()
                },
                path: e.path.clone(),
                icon_size: e.icon.as_ref().map_or(0, IconImage::size),
                icon: e.icon.clone(),
            })
            .collect()
    }

    /// Forgets icons whose app's window is gone, like Explorer does for apps that crash.
    pub(super) fn prune(&self) {
        self.lock().retain(|e| is_window(e.key.hwnd as HWND));
    }

    /// Forgets every icon.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Answers what an app asks about the taskbar, so that it isn't told that it failed. There is
    /// no taskbar: where it is, is said to be under the pointer (an app places its popup by it),
    /// and the other questions get the quiet answers of a taskbar that has no app bars and hides
    /// nothing.
    pub(super) fn app_bar(&self, data: &[u8]) -> LRESULT {
        let Some(request) = parse_appbar(data) else {
            return 0;
        };
        match request.message {
            ABM_GETTASKBARPOS => {
                let at = cursor();
                let half = ICON_RECT / 2;
                let mut answer = Vec::with_capacity(20);
                answer.extend_from_slice(&EDGE_BOTTOM.to_le_bytes());
                for value in [at.x - half, at.y - half, at.x + half, at.y + half] {
                    answer.extend_from_slice(&value.to_le_bytes());
                }
                // The call has succeeded either way; without the answer the caller sees no
                // taskbar position, as before, but no failure.
                write_to_sender(request.pid, request.shared + ABM_EDGE_AT, &answer);
                1
            }
            ABM_GETSTATE | ABM_GETAUTOHIDEBAR => 0,
            ABM_NEW | ABM_REMOVE | ABM_QUERYPOS | ABM_SETPOS | ABM_ACTIVATE
            | ABM_SETAUTOHIDEBAR | ABM_WINDOWPOSCHANGED | ABM_SETSTATE => 1,
            other => {
                tracing::info!("unknown app bar message {other}: {}", hex(data));
                0
            }
        }
    }

    /// Answers where an icon is. With no taskbar there is no icon to point at, so it says it is
    /// under the pointer, which is where the click that asks for its menu was made. Apps that
    /// ask (many do, to place their menu) otherwise put it in the screen's corner.
    pub(super) fn icon_rect(&self, data: &[u8]) -> LRESULT {
        let Some(request) = parse_rect_request(data) else {
            return 0;
        };
        let known = self
            .lock()
            .iter()
            .any(|e| e.matches(request.hwnd, request.id, request.guid));
        if !known {
            return 0;
        }
        match request.what {
            RECT_POSITION => {
                let at = cursor();
                pack_pair(at.x - ICON_RECT / 2, at.y - ICON_RECT / 2)
            }
            RECT_SIZE => pack_pair(ICON_RECT, ICON_RECT),
            other => {
                tracing::info!("unknown icon rectangle question {other}: {}", hex(data));
                0
            }
        }
    }

    /// Passes a click on to the icon's app. `at` is the screen position the app may show a menu
    /// at. Returns whether the icon was still there.
    pub fn click(&self, key: ItemKey, click: Click, at: (i32, i32)) -> bool {
        let (callback, version, id) = {
            let entries = self.lock();
            match entries.iter().find(|e| e.key == key) {
                Some(e) => (e.callback, e.version, e.key.id),
                None => return false,
            }
        };
        let (down, up) = match click {
            Click::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP),
            Click::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP),
            Click::Middle => (WM_MBUTTONDOWN, WM_MBUTTONUP),
        };
        // The pointer arrives over the icon first, as Explorer reports it.
        let mut events = vec![WM_MOUSEMOVE, down, up];
        if version >= 4 {
            match click {
                Click::Left => events.push(NIN_SELECT),
                Click::Right => events.push(WM_CONTEXTMENU),
                Click::Middle => {}
            }
        }
        // So the app can bring its menu or window to the front.
        // SAFETY: takes a process id by value.
        unsafe { AllowSetForegroundWindow(pid_of(key.hwnd as HWND)) };
        for event in events {
            let (wparam, lparam) = if version >= 4 {
                // Version 4: the position in `wparam`, the event and the id packed in `lparam`.
                let x = (at.0 as u16 as usize) | ((at.1 as u16 as usize) << 16);
                let l = (event as u16 as isize) | ((id as u16 as isize) << 16);
                (x, l)
            } else {
                (id as usize, event as isize)
            };
            // SAFETY: posting plain values; a window that has gone just makes the call fail.
            unsafe { PostMessageW(key.hwnd as HWND, callback, wparam, lparam) };
        }
        true
    }

    /// Applies what `Shell_NotifyIcon` sent. The result goes back to the app.
    pub(super) fn apply(&self, data: &[u8]) -> bool {
        let Some((message, nid)) = parse(data) else {
            return false;
        };
        let mut entries = self.lock();
        let guid = (nid.flags & NIF_GUID != 0).then_some(nid.guid);
        let found = entries
            .iter()
            .position(|e| e.matches(nid.hwnd, nid.id, guid));
        match message {
            NIM_ADD | NIM_MODIFY => {
                let index = match (found, message) {
                    (Some(i), _) => i,
                    (None, NIM_ADD) => {
                        let path = process_path(pid_of(nid.hwnd as HWND)).unwrap_or_default();
                        entries.push(Entry {
                            key: ItemKey {
                                hwnd: nid.hwnd,
                                id: nid.id,
                            },
                            guid,
                            callback: 0,
                            version: 0,
                            tip: String::new(),
                            program: program_name(&path),
                            path,
                            icon: None,
                            hidden: false,
                        });
                        entries.len() - 1
                    }
                    _ => return false,
                };
                let entry = &mut entries[index];
                if nid.flags & NIF_MESSAGE != 0 {
                    entry.callback = nid.callback;
                }
                if nid.flags & NIF_TIP != 0 {
                    entry.tip = nid.tip;
                }
                if nid.flags & NIF_ICON != 0 {
                    // SAFETY: CopyIcon takes any icon handle and fails on a bad one; icons are
                    // shared between processes, which is how the app's reaches us.
                    let copy = unsafe { CopyIcon(nid.icon as HICON) };
                    entry.icon =
                        (!copy.is_null()).then(|| IconImage(Arc::new(OwnedIcon(copy as isize))));
                }
                if nid.flags & NIF_STATE != 0 && nid.state_mask & NIS_HIDDEN != 0 {
                    entry.hidden = nid.state & NIS_HIDDEN != 0;
                }
                true
            }
            NIM_DELETE => match found {
                Some(i) => {
                    entries.remove(i);
                    true
                }
                None => false,
            },
            NIM_SETVERSION => match found {
                Some(i) => {
                    entries[i].version = nid.version;
                    true
                }
                None => false,
            },
            NIM_SETFOCUS => found.is_some(),
            _ => false,
        }
    }
}
