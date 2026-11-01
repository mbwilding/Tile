//! The low-level mouse hook.

use std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, LLMHF_INJECTED, MSG,
        MSLLHOOKSTRUCT, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT,
    },
};

use crate::keyboard::Decision;

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
