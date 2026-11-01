//! The low-level mouse hook.

use std::{cell::RefCell, io, ptr};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, LLMHF_INJECTED, MSLLHOOKSTRUCT, WH_MOUSE_LL, WINDOWS_HOOK_ID,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    },
};

use crate::{
    hook::{Hook, HookThread, Slot, message_loop, with_handler},
    keyboard::Decision,
};

/// What the mouse hook reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    /// The left button went down.
    LeftDown,
    /// The left button went up.
    LeftUp,
    /// The pointer moved.
    Move,
}

/// Runs on the mouse hook thread, which owns it.
pub trait MouseHandler {
    /// Decides one mouse event at screen position `(x, y)`. Must be quick.
    fn mouse(&mut self, event: MouseEvent, x: i32, y: i32) -> Decision;
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn MouseHandler>>> = const { RefCell::new(None) };
}

/// The mouse kind of hook.
struct Mouse;

impl Hook for Mouse {
    type Handler = dyn MouseHandler;
    const KIND: WINDOWS_HOOK_ID = WH_MOUSE_LL;
    const PROC: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT = mouse_proc;

    fn slot() -> &'static Slot<dyn MouseHandler> {
        &HANDLER
    }
}

/// A running `WH_MOUSE_LL` hook; dropping it removes the hook and ends its thread.
pub struct MouseHook {
    _thread: HookThread,
}

/// Starts a thread `name` with a mouse hook feeding the handler `make` creates there.
pub fn spawn_mouse<H: MouseHandler + 'static>(
    name: &str,
    make: impl FnOnce() -> H + Send + 'static,
) -> io::Result<MouseHook> {
    let thread = HookThread::spawn::<Mouse>(
        name,
        move || Box::new(make()) as Box<dyn MouseHandler>,
        message_loop,
    )?;
    Ok(MouseHook { _thread: thread })
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
        if let Some(event) = event.filter(|_| info.flags & LLMHF_INJECTED == 0)
            && with_handler::<Mouse, _>(|h| h.mouse(event, info.pt.x, info.pt.y))
                == Some(Decision::Block)
        {
            return 1;
        }
    }
    // SAFETY: hands the unchanged arguments on to the next hook.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}
