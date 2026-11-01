//! The low-level keyboard hook: a thread that decides every key event before the system sees it.

use std::{cell::RefCell, io, ptr, time::Instant};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::INFINITE,
    UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
        LLKHF_INJECTED, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
        PeekMessageW, PostThreadMessageW, QS_ALLINPUT, TranslateMessage, WH_KEYBOARD_LL,
        WINDOWS_HOOK_ID, WM_APP, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
    },
};

use crate::hook::{Hook, HookThread, Slot, with_handler};

/// Posted to the hook thread by [`Waker::wake`].
const WM_WAKE: u32 = WM_APP + 1;

/// A key event as the hook sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKey {
    /// The scancode. Values above `0xFF` are synthetic (e.g. the Ctrl Windows adds to AltGr).
    pub scan: u32,
    /// Whether the scancode has the `E0` prefix, as the arrows and right-hand modifiers do.
    pub extended: bool,
    /// The virtual key code Windows mapped it to.
    pub vk: u16,
    /// Down (including auto-repeat) or up.
    pub down: bool,
    /// Sent by a program rather than typed.
    pub injected: bool,
}

/// What a hook does with an event.
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

    /// When [`tick`](Self::tick) should next be called. Never, unless overridden.
    fn deadline(&self) -> Option<Instant> {
        None
    }

    /// Called once the [`deadline`](Self::deadline) has passed.
    fn tick(&mut self) {}

    /// Called after [`Waker::wake`]. Returning `false` removes the hook and ends the thread.
    fn wake(&mut self) -> bool {
        true
    }
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn Handler>>> = const { RefCell::new(None) };
}

/// The keyboard kind of hook.
struct Keyboard;

impl Hook for Keyboard {
    type Handler = dyn Handler;
    const KIND: WINDOWS_HOOK_ID = WH_KEYBOARD_LL;
    const PROC: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT = hook_proc;

    fn slot() -> &'static Slot<dyn Handler> {
        &HANDLER
    }
}

/// Wakes the hook thread to call [`Handler::wake`].
#[derive(Debug, Clone, Copy)]
pub struct Waker(u32);

impl Waker {
    /// Asks the hook thread to call [`Handler::wake`]. `false` if the thread has ended.
    pub fn wake(&self) -> bool {
        // SAFETY: posting a message with no pointers to a thread id; a thread that has exited
        // just makes the call fail.
        unsafe { PostThreadMessageW(self.0, WM_WAKE, 0, 0) != 0 }
    }
}

/// A running `WH_KEYBOARD_LL` hook; dropping it removes the hook and ends its thread.
pub struct KeyboardHook {
    thread: HookThread,
}

impl KeyboardHook {
    /// What wakes the hook thread, to hand its handler work from another thread.
    pub fn waker(&self) -> Waker {
        Waker(self.thread.id())
    }

    /// Leaves the hook running for the rest of the process.
    pub fn detach(self) {
        self.thread.detach();
    }
}

/// Starts a thread `name` that installs a keyboard hook and feeds it to the handler `make`
/// creates there. Returns once the hook is installed: Windows calls the newest hook first, so
/// hooks started later see keys before those started earlier.
pub fn spawn<H: Handler + 'static>(
    name: &str,
    make: impl FnOnce() -> H + Send + 'static,
) -> io::Result<KeyboardHook> {
    let thread =
        HookThread::spawn::<Keyboard>(name, move || Box::new(make()) as Box<dyn Handler>, pump)?;
    Ok(KeyboardHook { thread })
}

/// Dispatches messages until `WM_QUIT` or a wake the handler declines, calling its tick on time.
fn pump() {
    let mut msg = MSG::default();
    loop {
        let deadline = with_handler::<Keyboard, _>(|h| h.deadline()).flatten();
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
        while unsafe { PeekMessageW(&raw mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_QUIT {
                return;
            }
            if msg.hwnd.is_null() && msg.message == WM_WAKE {
                if with_handler::<Keyboard, _>(|h| h.wake()) == Some(false) {
                    return;
                }
                continue;
            }
            // SAFETY: `msg` was just filled in by PeekMessage.
            unsafe {
                TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        }
        if deadline.is_some_and(|at| Instant::now() >= at) {
            with_handler::<Keyboard, _>(|h| h.tick());
        }
    }
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
        };
        if with_handler::<Keyboard, _>(|h| h.key(key)) == Some(Decision::Block) {
            return 1;
        }
    }
    // SAFETY: hands the unchanged arguments on to the next hook.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}
