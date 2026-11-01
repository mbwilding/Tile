//! The low-level keyboard hook: a thread that decides every key event before the system sees it.

use std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Instant,
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{GetCurrentThreadId, INFINITE},
    },
    UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
        LLKHF_INJECTED, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_NOREMOVE,
        PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_QUIT,
        WM_SYSKEYDOWN,
    },
};

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
