//! Threads that own windows: a registered window class, a thread pumping its messages, and the
//! pieces of a message loop.

use std::{
    io, ptr,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, MSG, MsgWaitForMultipleObjects,
        PM_NOREMOVE, PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, RegisterClassW,
        TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WM_QUIT, WNDCLASSW, WS_POPUP,
    },
};

use crate::win::wide;

/// A window class of this module's, unregistered again on drop. Drop it on the thread that
/// destroyed its windows, after they are gone.
pub(crate) struct WindowClass {
    name: Vec<u16>,
    instance: HINSTANCE,
}

impl WindowClass {
    /// Registers `class` (its name and module filled in here) as `name`. A thread that ended
    /// without unregistering leaves the class behind, and the failure to register it again is
    /// harmless: that class is used instead.
    pub(crate) fn register(name: &str, class: WNDCLASSW) -> Self {
        let name = wide(name);
        // SAFETY: a null module name gives our own exe's handle.
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        let class = WNDCLASSW {
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..class
        };
        // SAFETY: `class` is fully initialised and `name` outlives the call.
        unsafe { RegisterClassW(&raw const class) };
        Self { name, instance }
    }

    /// Makes a hidden, empty popup window of this class, titled with the class name.
    pub(crate) fn create(&self, ex_style: WINDOW_EX_STYLE) -> io::Result<HWND> {
        // SAFETY: the class is registered and its name, used as the title too, is
        // NUL-terminated.
        let window = unsafe {
            CreateWindowExW(
                ex_style,
                self.name.as_ptr(),
                self.name.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                self.instance,
                ptr::null(),
            )
        };
        if window.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(window)
        }
    }
}

impl Drop for WindowClass {
    fn drop(&mut self) {
        // SAFETY: the class is ours; with windows of it left the call just fails.
        unsafe { UnregisterClassW(self.name.as_ptr(), self.instance) };
    }
}

/// What a [`MessageThread`] runs. It is made on the thread, sees the messages its loop takes, and
/// is dropped there when the loop ends, which is where it destroys its windows.
pub(crate) trait Pump {
    /// Looks at `message` before it is dispatched. Returns `true` for one handled here, which
    /// isn't dispatched.
    fn message(&mut self, _message: &MSG) -> bool {
        false
    }
}

/// A thread running a message loop for a [`Pump`]. Dropping it ends the loop and waits for the
/// thread.
pub(crate) struct MessageThread {
    id: u32,
    thread: Option<JoinHandle<()>>,
}

impl MessageThread {
    /// Starts the thread `name`, which runs `start` and then pumps messages for what it made
    /// until `WM_QUIT`. Returns once `start` has run, with the value it handed back.
    pub(crate) fn spawn<P, R>(
        name: &str,
        start: impl FnOnce() -> io::Result<(P, R)> + Send + 'static,
    ) -> io::Result<(Self, R)>
    where
        P: Pump,
        R: Send + 'static,
    {
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new().name(name.into()).spawn(move || {
            make_queue();
            let mut pump = match start() {
                Ok((pump, value)) => {
                    // SAFETY: no arguments.
                    let _ = ready_tx.send(Ok((unsafe { GetCurrentThreadId() }, value)));
                    pump
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let mut message = MSG::default();
            // SAFETY: `message` is a valid MSG to write to; the loop ends on WM_QUIT.
            while unsafe { GetMessageW(&raw mut message, ptr::null_mut(), 0, 0) } > 0 {
                if !pump.message(&message) {
                    dispatch(&message);
                }
            }
        })?;
        match ready_rx.recv() {
            Ok(Ok((id, value))) => Ok((
                Self {
                    id,
                    thread: Some(thread),
                },
                value,
            )),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other(format!("the {name} thread exited"))),
        }
    }

    /// Posts `message`, with no window, to the thread. `false` if it has ended.
    pub(crate) fn post(&self, message: u32) -> bool {
        // SAFETY: posting a message with no pointers to a thread id; if the thread has gone the
        // call just fails.
        unsafe { PostThreadMessageW(self.id, message, 0, 0) != 0 }
    }

    /// Whether the loop is still running.
    pub(crate) fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for MessageThread {
    fn drop(&mut self) {
        self.post(WM_QUIT);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Makes the calling thread's message queue, so messages posted to it before it first waits
/// aren't lost.
pub(crate) fn make_queue() {
    let mut message = MSG::default();
    // SAFETY: `message` is a valid MSG to write to; nothing is removed.
    unsafe { PeekMessageW(&raw mut message, ptr::null_mut(), 0, 0, PM_NOREMOVE) };
}

/// Translates and dispatches one message.
fn dispatch(message: &MSG) {
    // SAFETY: `message` was filled in by GetMessage or PeekMessage.
    unsafe {
        TranslateMessage(message);
        DispatchMessageW(message);
    }
}

/// Dispatches the thread's pending messages (and with them its hooks' and window events'
/// callbacks). Returns `false` once `WM_QUIT` is taken.
pub(crate) fn pump_pending() -> bool {
    let mut message = MSG::default();
    // SAFETY: `message` is a valid MSG to write to.
    while unsafe { PeekMessageW(&raw mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        if message.message == WM_QUIT {
            return false;
        }
        dispatch(&message);
    }
    true
}

/// Sleeps for up to `time`, waking early for a message, input or a callback.
pub(crate) fn wait(time: Duration) {
    let millis = u32::try_from(time.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: waits on no handles, only for this thread's queue.
    unsafe { MsgWaitForMultipleObjects(0, ptr::null(), 0, millis, QS_ALLINPUT) };
}
