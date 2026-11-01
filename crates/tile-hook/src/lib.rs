//! The Win32 calls winsafe doesn't wrap safely: `WH_KEYBOARD_LL` hooks that can block key-ups
//! and report scancodes, `SendInput` by scancode, session notifications, swapping the system
//! cursors (to hide the pointer), the single-instance mutex, the console close handler and
//! app icons from the shell. This is the only crate in Tile with `unsafe`, and it exposes a
//! safe API.

pub mod backdrop;
pub mod frame;
mod icon;
pub mod immersive;
pub mod notify;
pub mod system;
pub mod uwp;
pub mod wifi;

pub use icon::{Icon, monitor_dpi, render_hicon, shell_icon};

use std::{
    cell::RefCell,
    ffi::c_void,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::{OnceLock, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, FALSE, GetLastError, HANDLE, LPARAM, LRESULT, TRUE,
        WPARAM,
    },
    System::{
        Console::{
            CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, FreeConsole,
            GetConsoleProcessList, SetConsoleCtrlHandler,
        },
        LibraryLoader::GetModuleHandleW,
        RemoteDesktop::{
            NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
            WTSUnRegisterSessionNotification,
        },
        Threading::{CreateMutexW, GetCurrentThreadId, INFINITE},
    },
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE, SendInput,
        },
        WindowsAndMessaging::{
            CallNextHookEx, CreateCursor, DestroyCursor, DispatchMessageW, GetMessageW, HC_ACTION,
            KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT,
            MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, OCR_APPSTARTING, OCR_CROSS, OCR_HAND,
            OCR_HELP, OCR_IBEAM, OCR_NO, OCR_NORMAL, OCR_SIZEALL, OCR_SIZENESW, OCR_SIZENS,
            OCR_SIZENWSE, OCR_SIZEWE, OCR_UP, OCR_WAIT, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
            PostMessageW, PostThreadMessageW, QS_ALLINPUT, SC_CLOSE, SPI_SETCURSORS,
            SYSTEM_CURSOR_ID, SetSystemCursor, SetWindowsHookExW, SystemParametersInfoW,
            TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN,
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_SYSCOMMAND, WM_SYSKEYDOWN,
        },
    },
};
use windows_sys::core::BOOL;

/// `dwExtraInfo` on every key we send, to recognise our own input.
pub const TAG: usize = 0x4B45_5900;

/// Posted to the hook thread by [`Waker::wake`].
const WM_WAKE: u32 = WM_APP + 1;

/// A key event as the hook sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKey {
    /// The scancode. Values above `0xFF` are synthetic (e.g. the Ctrl Windows adds to AltGr).
    pub scan: u32,
    pub extended: bool,
    /// The virtual key code Windows mapped it to.
    pub vk: u16,
    /// Down (including auto-repeat) or up.
    pub down: bool,
    /// Sent by a program rather than typed.
    pub injected: bool,
    pub extra_info: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Let the event through to the next hook and the system.
    Pass,
    /// Swallow it.
    Block,
}

/// Runs on the hook thread, which owns it.
pub trait Handler {
    /// Decides one key event. Must be quick: Windows skips a hook that takes too long.
    fn key(&mut self, key: RawKey) -> Decision;
    /// When [`tick`](Self::tick) should next be called.
    fn deadline(&self) -> Option<Instant>;
    fn tick(&mut self);
    /// Called after [`Waker::wake`]. Returning `false` removes the hook and ends the thread.
    fn wake(&mut self) -> bool;
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn Handler>>> = const { RefCell::new(None) };
}

/// Runs `f` on this thread's handler. `None` if there is none, it's already in use (a
/// re-entrant call), or `f` panicked.
fn with_handler<R>(f: impl FnOnce(&mut dyn Handler) -> R) -> Option<R> {
    HANDLER.with(|cell| {
        let mut handler = cell.try_borrow_mut().ok()?;
        let handler = handler.as_mut()?;
        catch_unwind(AssertUnwindSafe(|| f(handler.as_mut()))).ok()
    })
}

/// Wakes the hook thread to call [`Handler::wake`].
#[derive(Debug, Clone, Copy)]
pub struct Waker(u32);

impl Waker {
    pub fn wake(&self) -> bool {
        // SAFETY: posting a message with no pointers to a thread id; a thread that has exited
        // just makes the call fail.
        unsafe { PostThreadMessageW(self.0, WM_WAKE, 0, 0) != 0 }
    }
}

/// Starts a thread `name` that installs a keyboard hook and feeds it to the handler `make`
/// creates there. Returns once the hook is installed: Windows calls the newest hook first, so
/// hooks started later see keys before those started earlier.
pub fn spawn<H: Handler + 'static>(
    name: &str,
    make: impl FnOnce() -> H + Send + 'static,
) -> io::Result<(Waker, JoinHandle<()>)> {
    let (tx, rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name(name.into())
        .spawn(move || run(make, &tx))?;
    match rx.recv() {
        Ok(Ok(id)) => Ok((Waker(id), thread)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(io::Error::other("the keyboard hook thread exited")),
    }
}

fn run<H: Handler + 'static>(make: impl FnOnce() -> H, ready: &mpsc::Sender<io::Result<u32>>) {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG to write to. This creates the thread's message queue, so
    // the waker's messages aren't lost.
    unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_NOREMOVE) };
    HANDLER.with(|cell| *cell.borrow_mut() = Some(Box::new(make())));
    // SAFETY: `hook_proc` has the HOOKPROC signature and lives for the whole program; a null
    // module name gets our own exe's handle.
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(hook_proc),
            GetModuleHandleW(ptr::null()),
            0,
        )
    };
    if hook.is_null() {
        let _ = ready.send(Err(io::Error::last_os_error()));
        return;
    }
    // SAFETY: no arguments.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    'pump: loop {
        let deadline = with_handler(|h| h.deadline()).flatten();
        let timeout = deadline.map_or(INFINITE, |at| {
            let wait = at.saturating_duration_since(Instant::now());
            // Round up, so we don't wake just before the deadline and spin.
            u32::try_from(wait.as_micros().div_ceil(1000)).unwrap_or(INFINITE - 1)
        });
        // SAFETY: waits on no handles (the null array has length 0), only on this thread's
        // message queue.
        unsafe {
            MsgWaitForMultipleObjectsEx(0, ptr::null(), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        // SAFETY: `msg` is a valid MSG to write to. Hook calls are dispatched inside PeekMessage.
        while unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_QUIT {
                break 'pump;
            }
            if msg.hwnd.is_null() && msg.message == WM_WAKE {
                if with_handler(|h| h.wake()) == Some(false) {
                    break 'pump;
                }
                continue;
            }
            // SAFETY: `msg` was just filled in by PeekMessage.
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if deadline.is_some_and(|at| Instant::now() >= at) {
            with_handler(|h| h.tick());
        }
    }

    // SAFETY: `hook` is the hook installed above, removed once.
    unsafe { UnhookWindowsHookEx(hook) };
    // Dropped here, on its own thread, so it can release keys it holds.
    HANDLER.with(|cell| cell.borrow_mut().take());
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for `WH_KEYBOARD_LL` with `HC_ACTION`, `lparam` points to a KBDLLHOOKSTRUCT
        // that is valid for the duration of this call.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let key = RawKey {
            scan: info.scanCode,
            extended: info.flags & LLKHF_EXTENDED != 0,
            vk: info.vkCode as u16,
            down: matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN),
            injected: info.flags & LLKHF_INJECTED != 0,
            extra_info: info.dwExtraInfo,
        };
        if with_handler(|h| h.key(key)) == Some(Decision::Block) {
            return 1;
        }
    }
    // SAFETY: hands the unchanged arguments on to the next hook.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// A key to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A physical key by scancode, which Windows maps through the current layout.
    Scan { scan: u8, extended: bool, up: bool },
    /// A virtual key code.
    Vk { vk: u16, up: bool },
}

/// Sends keys, tagged with [`TAG`]. Returns how many were sent.
pub fn send(inputs: &[Input]) -> usize {
    let inputs: Vec<INPUT> = inputs
        .iter()
        .map(|input| {
            let (vk, scan, flags) = match *input {
                Input::Scan { scan, extended, up } => (
                    0,
                    u16::from(scan),
                    KEYEVENTF_SCANCODE
                        | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 }
                        | if up { KEYEVENTF_KEYUP } else { 0 },
                ),
                Input::Vk { vk, up } => (vk, 0, if up { KEYEVENTF_KEYUP } else { 0 }),
            };
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: scan,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: TAG,
                    },
                },
            }
        })
        .collect();
    if inputs.is_empty() {
        return 0;
    }
    // SAFETY: `inputs` is a live array of `inputs.len()` INPUTs, each of the size we pass.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    sent as usize
}

/// The system cursors [`hide_cursor`] blanks out: every shape apps get from `LoadCursor`.
const CURSORS: [SYSTEM_CURSOR_ID; 14] = [
    OCR_NORMAL,
    OCR_IBEAM,
    OCR_WAIT,
    OCR_CROSS,
    OCR_UP,
    OCR_SIZENWSE,
    OCR_SIZENESW,
    OCR_SIZEWE,
    OCR_SIZENS,
    OCR_SIZEALL,
    OCR_NO,
    OCR_HAND,
    OCR_APPSTARTING,
    OCR_HELP,
];

/// Hides the pointer everywhere by swapping every system cursor for a transparent one, until
/// [`restore_cursors`]. Apps drawing their own cursor images still show them. Returns `false`
/// if some cursor couldn't be swapped.
pub fn hide_cursor() -> bool {
    // A 32x32 monochrome cursor: 4 bytes a row. AND bits set and XOR bits clear leave the
    // screen untouched.
    let and = [0xFF_u8; 32 * 4];
    let xor = [0_u8; 32 * 4];
    let mut all = true;
    for id in CURSORS {
        // SAFETY: both planes are live 32x32 1bpp bitmaps, the size given; a null instance is
        // allowed for a cursor not tied to a module.
        let blank = unsafe {
            CreateCursor(
                ptr::null_mut(),
                0,
                0,
                32,
                32,
                and.as_ptr().cast(),
                xor.as_ptr().cast(),
            )
        };
        if blank.is_null() {
            all = false;
            continue;
        }
        // SAFETY: `blank` is a cursor we own; on success the system takes it over (and
        // destroys it later), on failure it is still ours and destroyed here.
        unsafe {
            if SetSystemCursor(blank, id) == 0 {
                DestroyCursor(blank);
                all = false;
            }
        }
    }
    all
}

/// Reloads the user's cursor scheme, undoing [`hide_cursor`].
pub fn restore_cursors() -> bool {
    // SAFETY: `SPI_SETCURSORS` reads no parameter (null) and, with no flags, writes nothing.
    unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, ptr::null_mut(), 0) != 0 }
}

/// Holds the named mutex [`single_instance`] took; dropping it lets another instance start.
pub struct Instance(HANDLE);

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Takes the session-wide mutex `name`, or `None` if another process holds it. An elevated
/// holder counts too: opening its mutex is denied rather than shared.
pub fn single_instance(name: &str) -> Option<Instance> {
    let name: Vec<u16> = format!(r"Local\{name}").encode_utf16().chain([0]).collect();
    // SAFETY: `name` is NUL-terminated and outlives the call; no security attributes.
    let handle = unsafe { CreateMutexW(ptr::null(), FALSE, name.as_ptr()) };
    // SAFETY: reads the calling thread's last error, set by `CreateMutexW` just above.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if handle.is_null() {
        return None;
    }
    let instance = Instance(handle);
    (!existed).then_some(instance)
}

/// Lets go of the console if this process is the only one on it: started by Explorer, a task or
/// a Run entry, it would otherwise keep an empty console window open. Started from a terminal
/// it shares the console with the shell, so it stays attached and its output stays visible.
pub fn detach_console() {
    let mut owners = 0u32;
    // SAFETY: `owners` is a valid buffer for the one process id we ask for; the call only
    // reports how many processes share the console.
    let count = unsafe { GetConsoleProcessList(&raw mut owners, 1) };
    if count == 1 {
        // SAFETY: no arguments; only detaches this process from its console.
        unsafe { FreeConsole() };
    }
}

static ON_CONSOLE_CLOSE: OnceLock<fn()> = OnceLock::new();

/// Runs `f` when our console closes or the user logs off or shuts down, before Windows ends the
/// process. Ctrl+C is left to the caller. Only the first call registers.
pub fn on_console_close(f: fn()) {
    unsafe extern "system" fn handler(event: u32) -> BOOL {
        if matches!(
            event,
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
        ) && let Some(f) = ON_CONSOLE_CLOSE.get()
        {
            f();
        }
        // Not handled: the next handler (or the default, ending the process) runs.
        FALSE
    }
    if ON_CONSOLE_CLOSE.set(f).is_ok() {
        // SAFETY: `handler` is a valid `extern "system"` handler that lives for the whole
        // process and only reads a `OnceLock`.
        unsafe { SetConsoleCtrlHandler(Some(handler), TRUE) };
    }
}

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

/// What the mouse hook reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    LeftDown,
    LeftUp,
    Move,
}

/// Runs on the mouse hook thread, which owns it.
pub trait MouseHandler {
    /// Decides one mouse event at screen position `(x, y)`. Must be quick.
    fn mouse(&mut self, event: MouseEvent, x: i32, y: i32) -> Decision;
}

thread_local! {
    static MOUSE: RefCell<Option<Box<dyn MouseHandler>>> = const { RefCell::new(None) };
}

/// A running `WH_MOUSE_LL` hook; dropping it removes the hook and ends its thread.
pub struct MouseHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        // SAFETY: posting a message with no pointers to a thread id; a thread that has exited
        // just makes the call fail.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts a thread `name` with a mouse hook feeding the handler `make` creates there.
pub fn spawn_mouse<H: MouseHandler + 'static>(
    name: &str,
    make: impl FnOnce() -> H + Send + 'static,
) -> io::Result<MouseHook> {
    let (tx, rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name(name.into())
        .spawn(move || run_mouse(make, &tx))?;
    match rx.recv() {
        Ok(Ok(thread_id)) => Ok(MouseHook {
            thread_id,
            thread: Some(thread),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(io::Error::other("the mouse hook thread exited")),
    }
}

fn run_mouse<H: MouseHandler + 'static>(
    make: impl FnOnce() -> H,
    ready: &mpsc::Sender<io::Result<u32>>,
) {
    MOUSE.with(|cell| *cell.borrow_mut() = Some(Box::new(make())));
    // SAFETY: `mouse_proc` has the HOOKPROC signature and lives for the whole program; a null
    // module name gets our own exe's handle.
    let hook = unsafe {
        SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_proc),
            GetModuleHandleW(ptr::null()),
            0,
        )
    };
    if hook.is_null() {
        let _ = ready.send(Err(io::Error::last_os_error()));
        return;
    }
    // SAFETY: no arguments.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG to write to. Hook calls are dispatched inside GetMessage,
    // which returns 0 on `WM_QUIT` and -1 on error.
    while unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) } > 0 {
        // SAFETY: `msg` was just filled in by GetMessage.
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // SAFETY: `hook` is the hook installed above, removed once.
    unsafe { UnhookWindowsHookEx(hook) };
    MOUSE.with(|cell| cell.borrow_mut().take());
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for `WH_MOUSE_LL` with `HC_ACTION`, `lparam` points to an MSLLHOOKSTRUCT
        // that is valid for the duration of this call.
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        let event = match wparam as u32 {
            WM_LBUTTONDOWN => Some(MouseEvent::LeftDown),
            WM_LBUTTONUP => Some(MouseEvent::LeftUp),
            WM_MOUSEMOVE => Some(MouseEvent::Move),
            _ => None,
        };
        if let Some(event) = event.filter(|_| info.flags & LLMHF_INJECTED == 0) {
            let decision = MOUSE.with(|cell| {
                let mut handler = cell.try_borrow_mut().ok()?;
                let handler = handler.as_mut()?;
                catch_unwind(AssertUnwindSafe(|| {
                    handler.mouse(event, info.pt.x, info.pt.y)
                }))
                .ok()
            });
            if decision == Some(Decision::Block) {
                return 1;
            }
        }
    }
    // SAFETY: hands the unchanged arguments on to the next hook.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_instance_is_exclusive() {
        let name = format!("tile-test-{}", std::process::id());
        let first = single_instance(&name);
        assert!(first.is_some());
        assert!(single_instance(&name).is_none());
        drop(first);
        assert!(single_instance(&name).is_some());
    }
}
