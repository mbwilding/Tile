//! The notification area, for when Tile is the shell.
//!
//! Apps add their tray icons with `Shell_NotifyIcon`, which finds the window of class
//! `Shell_TrayWnd` and sends it a `WM_COPYDATA` message. Explorer owns that window; with no
//! Explorer, [`Host`] creates it and keeps the icons in a [`Registry`] the launcher lists. A
//! click is passed on to the icon's app the way Explorer would.

use std::{
    cell::RefCell,
    io, ptr, slice,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, FALSE, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW, HDC},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::{
        DataExchange::COPYDATASTRUCT,
        Diagnostics::Debug::WriteProcessMemory,
        LibraryLoader::GetModuleHandleW,
        Threading::{
            OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION,
            PROCESS_VM_WRITE, QueryFullProcessImageNameW,
        },
    },
    UI::WindowsAndMessaging::{
        ASFW_ANY, AllowSetForegroundWindow, CHANGEFILTERSTRUCT, ChangeWindowMessageFilterEx,
        CopyIcon, CreateWindowExW, DI_NORMAL, DefWindowProcW, DestroyIcon, DestroyWindow,
        DispatchMessageW, DrawIconEx, FindWindowExW, FindWindowW, GetCursorPos, GetIconInfo,
        GetMessageW, GetWindowThreadProcessId, HICON, HWND_BROADCAST, ICONINFO, IsWindow,
        KillTimer, MSG, MSGFLT_ALLOW, PostMessageW, PostQuitMessage, RegisterClassW,
        RegisterWindowMessageW, SetTimer, TranslateMessage, UnregisterClassW, WM_CLOSE,
        WM_CONTEXTMENU, WM_COPYDATA, WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
        WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
        WS_EX_TOOLWINDOW, WS_POPUP,
    },
};

/// The window class apps look for.
const TRAY_CLASS: &str = "Shell_TrayWnd";
/// The `dwData` of a `WM_COPYDATA` that carries a tray icon message.
const COPYDATA_NOTIFY: usize = 1;
/// The `dwData` of an app bar message (`SHAppBarMessage`): apps asking about the taskbar.
const COPYDATA_APPBAR: usize = 0;
/// The `dwData` of the question `Shell_NotifyIconGetRect` asks: where is this icon?
const COPYDATA_RECT: usize = 3;
/// The question is asked twice: once for the icon's top left corner, once for its size.
const RECT_POSITION: u32 = 1;
const RECT_SIZE: u32 = 2;
/// The size reported for every icon, and how far its corner is from the pointer to centre it.
const ICON_RECT: i32 = 24;
/// The first field of what `Shell_NotifyIcon` sends.
const MAGIC: u32 = 0x3475_3423;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;

const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_GUID: u32 = 0x20;

const NIS_HIDDEN: u32 = 0x01;

/// Sent to a version 4 icon's window for a left click, after the mouse messages.
const NIN_SELECT: u32 = 0x400;

/// How often icons whose app has gone are dropped.
const PRUNE_MS: u32 = 2000;
const PRUNE_TIMER: usize = 1;

/// A tray icon's identity: its app's window and the id the app gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemKey {
    pub hwnd: isize,
    pub id: u32,
}

/// A click on a tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Left,
    Right,
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
        crate::render_hicon(self.0.0, size)
    }

    /// The icon's width in pixels as the app made it: a tray icon is usually 16, so drawing it
    /// bigger blurs it. 0 if it can't be read.
    pub fn size(&self) -> i32 {
        let mut info = ICONINFO::default();
        let mut bitmap = BITMAP::default();
        // SAFETY: `info` and `bitmap` are live structs of the sizes given; the bitmaps
        // GetIconInfo hands over are ours to delete, once each.
        unsafe {
            if GetIconInfo(self.0.0 as HICON, &raw mut info) == 0 {
                return 0;
            }
            let source = if info.hbmColor.is_null() {
                info.hbmMask
            } else {
                info.hbmColor
            };
            let got = GetObjectW(
                source,
                std::mem::size_of::<BITMAP>() as i32,
                (&raw mut bitmap).cast(),
            );
            for bitmap in [info.hbmColor, info.hbmMask] {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
            }
            if got == 0 { 0 } else { bitmap.bmWidth }
        }
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
    pub key: ItemKey,
    /// The tooltip, or the name of the app's program if it has none.
    pub name: String,
    /// The app's program, to take a sharper icon from when the tray icon is small.
    pub path: String,
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

/// A decoded `NOTIFYICONDATA`, as `Shell_NotifyIcon` sends it (handles are 32 bits wide).
#[derive(Debug, Default, PartialEq, Eq)]
struct Nid {
    hwnd: isize,
    id: u32,
    flags: u32,
    callback: u32,
    icon: isize,
    tip: String,
    state: u32,
    state_mask: u32,
    version: u32,
    guid: [u8; 16],
}

/// Offsets into the `COPYDATASTRUCT`'s data: an 8 byte header, then the `NOTIFYICONDATAW`.
mod at {
    pub const MESSAGE: usize = 4;
    pub const NID: usize = 8;
    pub const HWND: usize = NID + 4;
    pub const ID: usize = NID + 8;
    pub const FLAGS: usize = NID + 12;
    pub const CALLBACK: usize = NID + 16;
    pub const ICON: usize = NID + 20;
    pub const TIP: usize = NID + 24;
    pub const TIP_CHARS: usize = 128;
    pub const STATE: usize = TIP + TIP_CHARS * 2;
    pub const STATE_MASK: usize = STATE + 4;
    pub const INFO: usize = STATE_MASK + 4;
    pub const VERSION: usize = INFO + 256 * 2;
    pub const GUID: usize = VERSION + 4 + 64 * 2 + 4;
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// Decodes the data of a tray `WM_COPYDATA`: the message (`NIM_*`) and the icon data. Fields
/// past the end of a short (older) structure stay empty.
fn parse(data: &[u8]) -> Option<(u32, Nid)> {
    if u32_at(data, 0)? != MAGIC {
        return None;
    }
    let message = u32_at(data, at::MESSAGE)?;
    let mut nid = Nid {
        // Handles are sign-extended from 32 bits.
        hwnd: i64::from(u32_at(data, at::HWND)? as i32) as isize,
        id: u32_at(data, at::ID)?,
        flags: u32_at(data, at::FLAGS).unwrap_or(0),
        callback: u32_at(data, at::CALLBACK).unwrap_or(0),
        icon: u32_at(data, at::ICON).map_or(0, |h| i64::from(h as i32) as isize),
        ..Nid::default()
    };
    if let Some(bytes) = data.get(at::TIP..at::TIP + at::TIP_CHARS * 2) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .take_while(|&u| u != 0)
            .collect();
        nid.tip = String::from_utf16_lossy(&units);
    }
    nid.state = u32_at(data, at::STATE).unwrap_or(0);
    nid.state_mask = u32_at(data, at::STATE_MASK).unwrap_or(0);
    nid.version = u32_at(data, at::VERSION).unwrap_or(0);
    if let Some(guid) = data.get(at::GUID..at::GUID + 16) {
        nid.guid.copy_from_slice(guid);
    }
    Some((message, nid))
}

/// The first bytes of `data` as hex, for the log.
fn hex(data: &[u8]) -> String {
    data.iter()
        .take(48)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

const ABM_NEW: u32 = 0;
const ABM_REMOVE: u32 = 1;
const ABM_QUERYPOS: u32 = 2;
const ABM_SETPOS: u32 = 3;
const ABM_GETSTATE: u32 = 4;
const ABM_GETTASKBARPOS: u32 = 5;
const ABM_ACTIVATE: u32 = 6;
const ABM_GETAUTOHIDEBAR: u32 = 7;
const ABM_SETAUTOHIDEBAR: u32 = 8;
const ABM_WINDOWPOSCHANGED: u32 = 9;
const ABM_SETSTATE: u32 = 10;
/// `ABE_BOTTOM`.
const EDGE_BOTTOM: u32 = 3;
/// Where in the sender's block the shell writes the edge, then the rectangle.
const ABM_EDGE_AT: u64 = 28;

/// A decoded app bar message.
#[derive(Debug, PartialEq, Eq)]
struct AppBarRequest {
    /// `ABM_*`.
    message: u32,
    /// An address in the sender's process of the block the answer goes into.
    shared: u64,
    pid: u32,
}

/// Decodes the data of a `dwData` 0 `WM_COPYDATA`: the `APPBARDATA`, then the message, and where
/// in which process the answer is written.
fn parse_appbar(data: &[u8]) -> Option<AppBarRequest> {
    Some(AppBarRequest {
        message: u32_at(data, 40)?,
        shared: u64::from_le_bytes(data.get(48..56)?.try_into().ok()?),
        pid: u32_at(data, 56)?,
    })
}

/// Whether the process `pid` runs with administrator rights. `true` when that can't be told.
fn is_elevated(pid: u32) -> bool {
    // SAFETY: the handles are closed below; the out values are live locals of the sizes given.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if process.is_null() {
            return true;
        }
        let mut token: HANDLE = ptr::null_mut();
        let mut elevated = true;
        if OpenProcessToken(process, TOKEN_QUERY, &raw mut token) != 0 {
            let mut info = TOKEN_ELEVATION { TokenIsElevated: 1 };
            let mut got = 0;
            let size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
            if GetTokenInformation(
                token,
                TokenElevation,
                (&raw mut info).cast(),
                size,
                &raw mut got,
            ) != 0
            {
                elevated = info.TokenIsElevated != 0;
            }
            CloseHandle(token);
        }
        CloseHandle(process);
        elevated
    }
}

/// Writes `bytes` at `address` in the process `pid`. Only into a process without administrator
/// rights: the sender names the process and address itself and can't be told apart from another
/// program, and Tile's own rights must not become a way to write into one that has more than
/// the sender. A program can already write into its peers, so nothing is gained from this.
fn write_to_sender(pid: u32, address: u64, bytes: &[u8]) -> bool {
    if is_elevated(pid) {
        return false;
    }
    // SAFETY: the handle is closed; `bytes` is a live slice of the length given. The address is
    // another process's, which the call checks.
    unsafe {
        let process = OpenProcess(PROCESS_VM_OPERATION | PROCESS_VM_WRITE, FALSE, pid);
        if process.is_null() {
            return false;
        }
        let mut written = 0;
        let ok = WriteProcessMemory(
            process,
            address as usize as *const core::ffi::c_void,
            bytes.as_ptr().cast(),
            bytes.len(),
            &raw mut written,
        );
        CloseHandle(process);
        ok != 0 && written == bytes.len()
    }
}

/// A decoded `Shell_NotifyIconGetRect` question.
#[derive(Debug, PartialEq, Eq)]
struct RectRequest {
    /// [`RECT_POSITION`] or [`RECT_SIZE`].
    what: u32,
    hwnd: isize,
    id: u32,
    guid: Option<[u8; 16]>,
}

/// Decodes the data of a `dwData` 3 `WM_COPYDATA`: the magic and which half is asked for, then
/// the identifier of the icon (`NOTIFYICONIDENTIFIER` with 32 bit handles).
fn parse_rect_request(data: &[u8]) -> Option<RectRequest> {
    if u32_at(data, 0)? != MAGIC {
        return None;
    }
    let guid: [u8; 16] = data.get(24..40)?.try_into().ok()?;
    Some(RectRequest {
        what: u32_at(data, 4)?,
        hwnd: i64::from(u32_at(data, 16)? as i32) as isize,
        id: u32_at(data, 20)?,
        guid: (guid != [0; 16]).then_some(guid),
    })
}

/// Two screen values packed the way the shell answers: the first in the low 16 bits, the second
/// in the next 16, each signed.
fn pack_pair(a: i32, b: i32) -> LRESULT {
    let word = |v: i32| u32::from(v as i16 as u16);
    (word(a) | (word(b) << 16)) as i32 as LRESULT
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn pid_of(hwnd: isize) -> u32 {
    let mut pid = 0;
    // SAFETY: `pid` is a live u32 to write to; a stale window just gives 0.
    unsafe { GetWindowThreadProcessId(hwnd as HWND, &raw mut pid) };
    pid
}

/// The file name of the program `pid` runs, without `.exe`.
fn program_of(pid: u32) -> String {
    let path = process_path(pid);
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
        .to_owned()
}

/// The full path of the program `pid` runs, or an empty string.
fn process_path(pid: u32) -> String {
    // SAFETY: the handle is closed below; the buffer length given is its real length.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if process.is_null() {
            return String::new();
        }
        let mut buffer = [0u16; 520];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &raw mut len);
        CloseHandle(process);
        if ok == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..len as usize])
    }
}

fn is_window(hwnd: isize) -> bool {
    // SAFETY: IsWindow accepts any handle value.
    unsafe { IsWindow(hwnd as HWND) != 0 }
}

/// The tray icons apps have added. Cloning shares the list.
#[derive(Debug, Clone, Default)]
pub struct Registry(Arc<Mutex<Vec<Entry>>>);

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry").field("key", &self.key).finish()
    }
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
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
    pub fn prune(&self) {
        self.lock().retain(|e| is_window(e.key.hwnd));
    }

    /// Forgets every icon.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Whether an icon with this identity is in the list.
    fn has(&self, hwnd: isize, id: u32, guid: Option<[u8; 16]>) -> bool {
        self.lock().iter().any(|e| match (guid, e.guid) {
            (Some(a), Some(b)) => a == b,
            _ => e.key.hwnd == hwnd && e.key.id == id,
        })
    }

    /// Answers what an app asks about the taskbar, so that it isn't told that it failed. There is
    /// no taskbar: where it is, is said to be under the pointer (an app places its popup by it),
    /// and the other questions get the quiet answers of a taskbar that has no app bars and hides
    /// nothing.
    fn app_bar(&self, data: &[u8]) -> LRESULT {
        let Some(request) = parse_appbar(data) else {
            return 0;
        };
        match request.message {
            ABM_GETTASKBARPOS => {
                let mut at = POINT { x: 0, y: 0 };
                // SAFETY: `at` is a live POINT to write to.
                unsafe { GetCursorPos(&raw mut at) };
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
    fn icon_rect(&self, data: &[u8]) -> LRESULT {
        let Some(request) = parse_rect_request(data) else {
            return 0;
        };
        if !self.has(request.hwnd, request.id, request.guid) {
            return 0;
        }
        match request.what {
            RECT_POSITION => {
                let mut at = POINT { x: 0, y: 0 };
                // SAFETY: `at` is a live POINT to write to.
                unsafe { GetCursorPos(&raw mut at) };
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
        unsafe { AllowSetForegroundWindow(pid_of(key.hwnd)) };
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
    fn apply(&self, data: &[u8]) -> bool {
        let Some((message, nid)) = parse(data) else {
            return false;
        };
        let mut entries = self.lock();
        let guid = (nid.flags & NIF_GUID != 0).then_some(nid.guid);
        let found = entries.iter().position(|e| match (guid, e.guid) {
            (Some(a), Some(b)) => a == b,
            _ => e.key.hwnd == nid.hwnd && e.key.id == nid.id,
        });
        match message {
            NIM_ADD | NIM_MODIFY => {
                let index = match (found, message) {
                    (Some(i), _) => i,
                    (None, NIM_ADD) => {
                        entries.push(Entry {
                            key: ItemKey {
                                hwnd: nid.hwnd,
                                id: nid.id,
                            },
                            guid,
                            callback: 0,
                            version: 0,
                            tip: String::new(),
                            program: program_of(pid_of(nid.hwnd)),
                            path: process_path(pid_of(nid.hwnd)),
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

/// Whether another process has a window of `class`, other than `own`.
fn other_window(class: &str, own: isize) -> bool {
    let class = wide(class);
    let mut after: HWND = ptr::null_mut();
    loop {
        // SAFETY: `class` is NUL-terminated and outlives the call.
        after = unsafe { FindWindowExW(ptr::null_mut(), after, class.as_ptr(), ptr::null()) };
        if after.is_null() {
            return false;
        }
        if after as isize != own {
            return true;
        }
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
    let class = wide(TRAY_CLASS);
    // SAFETY: `class` is NUL-terminated and outlives the call.
    unsafe { !FindWindowW(class.as_ptr(), ptr::null()).is_null() }
}

/// The notification area's window, on its own thread.
pub struct Host {
    hwnd: Arc<AtomicIsize>,
    alive: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
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
        let hwnd = Arc::new(AtomicIsize::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("notification area".into())
            .spawn({
                let (hwnd, alive) = (hwnd.clone(), alive.clone());
                move || {
                    run(registry, class, &hwnd, &ready_tx);
                    alive.store(false, Ordering::Release);
                }
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                hwnd,
                alive,
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other("the notification area thread exited")),
        }
    }

    /// Whether the window is still there. It goes when another taskbar appears.
    pub fn running(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Closes the window and stops the thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            // SAFETY: posting a plain message; a window that has gone just makes it fail.
            unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    registry: Registry,
    class: &str,
    hwnd_slot: &AtomicIsize,
    ready: &mpsc::Sender<io::Result<()>>,
) {
    REGISTRY.with(|r| *r.borrow_mut() = Some(registry.clone()));
    let class_name = wide(class);
    // SAFETY: a null module name gives our own exe's handle.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let wc = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialised and `class_name` outlives the call; it is registered
    // again on a restart, so an "already exists" failure is fine and caught below.
    unsafe { RegisterClassW(&raw const wc) };
    // SAFETY: the class was registered above; the strings are NUL-terminated.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        let _ = ready.send(Err(io::Error::last_os_error()));
        return;
    }
    hwnd_slot.store(hwnd as isize, Ordering::Release);
    // An elevated Tile would otherwise never hear from the apps of a lower integrity level.
    // SAFETY: `hwnd` is the window just made; the filter struct is optional.
    unsafe {
        ChangeWindowMessageFilterEx(
            hwnd,
            WM_COPYDATA,
            MSGFLT_ALLOW,
            ptr::null_mut::<CHANGEFILTERSTRUCT>(),
        );
        SetTimer(hwnd, PRUNE_TIMER, PRUNE_MS, None);
    }
    let _ = ready.send(Ok(()));

    // Apps that were running add their icons again when told a taskbar exists.
    let created = wide("TaskbarCreated");
    // SAFETY: `created` is NUL-terminated; broadcasting a registered message with no data.
    unsafe {
        let message = RegisterWindowMessageW(created.as_ptr());
        PostMessageW(HWND_BROADCAST, message, 0, 0);
    }

    let own = hwnd as isize;
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG to write to; the loop ends on WM_QUIT.
    while unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) } > 0 {
        // A real taskbar took over: leave the apps to it.
        if msg.message == WM_TIMER && other_window(class, own) {
            // SAFETY: closes our window, ending the loop.
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        }
        // SAFETY: `msg` was just filled in by GetMessage.
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    registry.clear();
    REGISTRY.with(|r| r.borrow_mut().take());
    // SAFETY: the class belongs to this module and has no windows left.
    unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `Shell_NotifyIcon` sends for one icon.
    fn message(kind: u32, hwnd: i32, id: u32, flags: u32, callback: u32, tip: &str) -> Vec<u8> {
        let mut data = vec![0u8; at::GUID + 16 + 4];
        let mut put =
            |at: usize, value: u32| data[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(0, MAGIC);
        put(at::MESSAGE, kind);
        put(at::HWND, hwnd as u32);
        put(at::ID, id);
        put(at::FLAGS, flags);
        put(at::CALLBACK, callback);
        for (i, unit) in tip.encode_utf16().enumerate() {
            let at = at::TIP + i * 2;
            data[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        data
    }

    #[test]
    fn parses_what_shell_notifyicon_sends() {
        let data = message(NIM_ADD, -5, 7, NIF_MESSAGE | NIF_TIP, 0x8001, "Discord");
        let (kind, nid) = parse(&data).unwrap();
        assert_eq!(kind, NIM_ADD);
        assert_eq!((nid.hwnd, nid.id, nid.callback), (-5, 7, 0x8001));
        assert_eq!(nid.tip, "Discord");
    }

    #[test]
    fn rejects_other_data() {
        assert_eq!(parse(&[0; 4]), None);
        let mut data = message(NIM_ADD, 1, 1, 0, 0, "");
        data[0] ^= 0xFF;
        assert_eq!(parse(&data), None);
    }

    #[test]
    fn short_structures_leave_later_fields_empty() {
        let mut data = message(NIM_MODIFY, 1, 2, NIF_TIP, 0, "x");
        data.truncate(at::TIP);
        let (_, nid) = parse(&data).unwrap();
        assert_eq!((nid.hwnd, nid.id, nid.tip.as_str()), (1, 2, ""));
    }

    /// Sends `data` the way an app does: a `WM_COPYDATA` to the window.
    fn send(class: &str, data: &[u8]) -> bool {
        send_kind(class, COPYDATA_NOTIFY, data) != 0
    }

    fn send_kind(class: &str, kind: usize, data: &[u8]) -> isize {
        use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
        let name = wide(class);
        // SAFETY: the strings are NUL-terminated; the COPYDATASTRUCT describes `data`, which
        // outlives the synchronous SendMessage.
        unsafe {
            let hwnd = FindWindowW(name.as_ptr(), ptr::null());
            assert!(!hwnd.is_null(), "no window of class {class}");
            let cds = COPYDATASTRUCT {
                dwData: kind,
                cbData: data.len() as u32,
                lpData: data.as_ptr().cast_mut().cast(),
            };
            SendMessageW(hwnd, WM_COPYDATA, 0, (&raw const cds) as isize)
        }
    }

    /// What `Shell_NotifyIconGetRect` sends about the icon `id` of the window `hwnd`.
    fn rect_question(what: u32, hwnd: i32, id: u32) -> Vec<u8> {
        let mut data = vec![0u8; 40];
        let mut put =
            |at: usize, value: u32| data[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(0, MAGIC);
        put(4, what);
        put(8, 32);
        put(16, hwnd as u32);
        put(20, id);
        data
    }

    /// What `SHAppBarMessage` sends: an `APPBARDATA`, then the message and where the answer goes.
    fn appbar_message(message: u32, shared: u64, pid: u32) -> Vec<u8> {
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(&40u32.to_le_bytes());
        data[40..44].copy_from_slice(&message.to_le_bytes());
        data[48..56].copy_from_slice(&shared.to_le_bytes());
        data[56..60].copy_from_slice(&pid.to_le_bytes());
        data
    }

    #[test]
    fn parses_the_app_bar_message() {
        let data = appbar_message(ABM_GETTASKBARPOS, 0x1e4_b450_fe80, 41852);
        assert_eq!(
            parse_appbar(&data),
            Some(AppBarRequest {
                message: ABM_GETTASKBARPOS,
                shared: 0x1e4_b450_fe80,
                pid: 41852,
            })
        );
        assert_eq!(parse_appbar(&data[..50]), None);
    }

    #[test]
    fn parses_the_position_question() {
        let data = rect_question(RECT_SIZE, -9, 4);
        assert_eq!(
            parse_rect_request(&data),
            Some(RectRequest {
                what: RECT_SIZE,
                hwnd: -9,
                id: 4,
                guid: None,
            })
        );
        assert_eq!(parse_rect_request(&data[..20]), None);
    }

    #[test]
    fn packs_pairs_the_way_the_shell_answers() {
        assert_eq!(pack_pair(300, 500), 300 | (500 << 16));
        // Signed, for monitors left of or above the primary one.
        assert_eq!(pack_pair(-1, -2) & 0xFFFF, 0xFFFF);
        assert_eq!((pack_pair(-1, -2) >> 16) & 0xFFFF, 0xFFFE);
    }

    #[test]
    fn a_host_keeps_the_icons_apps_send() {
        const CLASS: &str = "TileNotifyTest";
        let registry = Registry::default();
        let host = Host::start_as(registry.clone(), CLASS).expect("the host starts");
        assert!(host.running());

        // A window that exists, to stand in for the app's: the host's own.
        let own = host.hwnd.load(Ordering::Acquire) as i32;
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
