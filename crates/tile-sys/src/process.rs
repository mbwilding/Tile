//! The single-instance mutex and the console: closing, and letting go of it.

use std::{ptr, sync::OnceLock};

use windows_sys::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, FALSE, GetLastError, HANDLE, TRUE},
        System::{
            Console::{
                CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, FreeConsole,
                GetConsoleProcessList, SetConsoleCtrlHandler,
            },
            Threading::CreateMutexW,
        },
    },
    core::BOOL,
};

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
