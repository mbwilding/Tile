//! The thread behind each low-level hook: it installs the hook, keeps the handler the hook
//! procedure calls, and pumps the messages the hook is called from.

use std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::mpsc,
    thread::{self, JoinHandle, LocalKey},
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WINDOWS_HOOK_ID, WM_QUIT,
    },
};

/// The slot a hook thread keeps its handler in.
pub(crate) type Slot<H> = LocalKey<RefCell<Option<Box<H>>>>;

/// A kind of low-level hook.
pub(crate) trait Hook {
    /// What the hook procedure hands events to, kept by the hook's thread.
    type Handler: ?Sized + 'static;
    /// `WH_KEYBOARD_LL` or `WH_MOUSE_LL`.
    const KIND: WINDOWS_HOOK_ID;
    /// The hook procedure, which finds the handler with [`with_handler`].
    const PROC: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT;
    /// The thread-local slot the handler lives in.
    fn slot() -> &'static Slot<Self::Handler>;
}

/// Runs `f` on this thread's handler for hooks of kind `K`. `None` if there is none, it's
/// already in use (a re-entrant call), or `f` panicked: a panic must not unwind into Windows.
pub(crate) fn with_handler<K: Hook, R>(f: impl FnOnce(&mut K::Handler) -> R) -> Option<R> {
    K::slot().with(|cell| {
        let mut handler = cell.try_borrow_mut().ok()?;
        let handler = handler.as_mut()?;
        catch_unwind(AssertUnwindSafe(|| f(handler.as_mut()))).ok()
    })
}

/// A thread running a low-level hook. Dropping it removes the hook and waits for the thread.
pub(crate) struct HookThread {
    id: u32,
    thread: Option<JoinHandle<()>>,
}

impl HookThread {
    /// Starts the thread `name`, which makes its handler with `make`, installs a hook of kind
    /// `K` and runs `pump` until the hook is to go. Returns once the hook is installed.
    pub(crate) fn spawn<K: Hook>(
        name: &str,
        make: impl FnOnce() -> Box<K::Handler> + Send + 'static,
        pump: fn(),
    ) -> io::Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || run::<K>(make, pump, &ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(id)) => Ok(Self {
                id,
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other(format!("the {name} thread exited"))),
        }
    }

    /// The thread's id, to post messages to.
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    /// Leaves the hook running for the rest of the process.
    pub(crate) fn detach(mut self) {
        self.thread.take();
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // SAFETY: posting a message with no pointers to a thread id; a thread that has
            // exited just makes the call fail.
            unsafe { PostThreadMessageW(self.id, WM_QUIT, 0, 0) };
            let _ = thread.join();
        }
    }
}

fn run<K: Hook>(
    make: impl FnOnce() -> Box<K::Handler>,
    pump: fn(),
    ready: &mpsc::Sender<io::Result<u32>>,
) {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG to write to. This creates the thread's message queue, so
    // messages posted to the thread from now on aren't lost.
    unsafe { PeekMessageW(&raw mut msg, ptr::null_mut(), 0, 0, PM_NOREMOVE) };
    K::slot().with(|cell| *cell.borrow_mut() = Some(make()));
    // SAFETY: the procedure has the HOOKPROC signature and lives for the whole program; a null
    // module name gets our own exe's handle.
    let hook =
        unsafe { SetWindowsHookExW(K::KIND, Some(K::PROC), GetModuleHandleW(ptr::null()), 0) };
    if hook.is_null() {
        let _ = ready.send(Err(io::Error::last_os_error()));
        K::slot().with(|cell| cell.borrow_mut().take());
        return;
    }
    // SAFETY: no arguments.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    pump();
    // SAFETY: `hook` is the hook installed above, removed once.
    unsafe { UnhookWindowsHookEx(hook) };
    // Dropped here, on its own thread, so it can release keys it holds.
    K::slot().with(|cell| cell.borrow_mut().take());
}

/// Dispatches messages, and with them the hook's calls, until `WM_QUIT`.
pub(crate) fn message_loop() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG to write to. Hook calls are dispatched inside GetMessage,
    // which returns 0 on `WM_QUIT` and -1 on error.
    while unsafe { GetMessageW(&raw mut msg, ptr::null_mut(), 0, 0) } > 0 {
        // SAFETY: `msg` was just filled in by GetMessage.
        unsafe {
            TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}
