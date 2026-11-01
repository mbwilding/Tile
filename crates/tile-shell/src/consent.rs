//! UAC's consent prompt when no taskbar is there to show it.
//!
//! When something asks for admin rights while its window isn't in front (and the window in front
//! isn't one of Explorer's own: its desktop, taskbar and Run box carry the
//! `AllowConsentToStealFocus` property), `consent.exe` doesn't switch to the secure desktop
//! straight away. It makes a minimised "User Account Control" window that flashes on the taskbar
//! and waits, for as long as it takes, for that button to be clicked. With no taskbar the
//! button is never seen, so the prompt never appears and the program asking waits for good.
//!
//! [`Prompts`] stands in for that click: it watches for the waiting window and restores it, which
//! is what tells `consent.exe` to carry on and ask on the secure desktop as usual. The window
//! belongs to a SYSTEM process, which doesn't accept messages from Tile, so it is restored the
//! way the taskbar does it, with `SwitchToThisWindow`, which has Windows post the restore itself.
//!
//! This rests on how `consent.exe` and `SwitchToThisWindow` behave in Windows 11 (build 26100),
//! read from their code: the window's class name, that a restore of it is what releases the
//! prompt, and the switch option that restores a minimised window.

use std::{
    collections::HashMap,
    io, ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        FindWindowExW, IsIconic, IsWindow, PostMessageW, SC_RESTORE, SwitchToThisWindow,
        WM_SYSCOMMAND,
    },
};

/// The class of the minimised window `consent.exe` waits behind.
pub const WAITING_CLASS: &str = "$$$Secure UAP Dummy Window Class For Interim Dialog";
/// `SwitchToThisWindow`'s option to restore a minimised window as it comes to the front; the
/// documented `TRUE` (an Alt+Tab switch) leaves it minimised.
const RESTORE: i32 = 2;
/// How often to look for a waiting prompt.
const POLL: Duration = Duration::from_millis(250);

/// Restores waiting consent prompts on a thread of its own. Dropping it stops the thread.
pub struct Prompts {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Prompts {
    /// Starts watching for consent prompts that wait for a taskbar click.
    pub fn start() -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name("consent prompts".into())
            .spawn({
                let stop = stop.clone();
                move || run(&stop)
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Prompts {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(stop: &AtomicBool) {
    let mut tries = Tries::default();
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let waiting = waiting_windows();
        tries.keep(&waiting.iter().map(|&w| w as usize).collect::<Vec<_>>());
        for window in waiting {
            if tries.due(window as usize, now) {
                tracing::info!("showing a UAC prompt that was waiting for a taskbar click");
                restore(window);
            }
        }
        thread::sleep(POLL);
    }
}

/// The minimised windows consent prompts are waiting behind.
fn waiting_windows() -> Vec<HWND> {
    let class: Vec<u16> = WAITING_CLASS.encode_utf16().chain([0]).collect();
    let mut found = Vec::new();
    let mut after = ptr::null_mut();
    loop {
        // SAFETY: `class` is NUL-terminated and outlives the call; `after` is null or a handle the
        // search just returned.
        after = unsafe { FindWindowExW(ptr::null_mut(), after, class.as_ptr(), ptr::null()) };
        if after.is_null() {
            return found;
        }
        // SAFETY: `IsIconic` accepts any handle value.
        if unsafe { IsIconic(after) } != 0 {
            found.push(after);
        }
    }
}

/// Restores `window` the way a click on its taskbar button does.
fn restore(window: HWND) {
    // SAFETY: both calls accept any handle value; a window that has gone just makes them fail.
    unsafe {
        if IsWindow(window) == 0 {
            return;
        }
        SwitchToThisWindow(window, RESTORE);
        // Windows refuses this across the gap in rights to the prompt; it only matters should a
        // later build change what the switch above does.
        PostMessageW(window, WM_SYSCOMMAND, SC_RESTORE as usize, 0);
    }
}

/// How often to try a waiting window again: once at once, then while it is still there.
const RETRY: Duration = Duration::from_secs(2);
/// Tries per window before giving up on it, so a prompt that won't come up isn't pushed forever.
const MAX_TRIES: u8 = 5;

/// When each waiting window was last tried, and how many times.
#[derive(Default)]
struct Tries(HashMap<usize, (Instant, u8)>);

impl Tries {
    /// Forgets the windows that are no longer waiting.
    fn keep(&mut self, waiting: &[usize]) {
        self.0.retain(|window, _| waiting.contains(window));
    }

    /// Whether to try `window` now, noting the try if so.
    fn due(&mut self, window: usize, now: Instant) -> bool {
        match self.0.get_mut(&window) {
            None => {
                self.0.insert(window, (now, 1));
                true
            }
            Some((last, count)) if *count < MAX_TRIES && now.duration_since(*last) >= RETRY => {
                *last = now;
                *count += 1;
                true
            }
            Some(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_window_is_tried_at_once() {
        let mut tries = Tries::default();
        assert!(tries.due(1, Instant::now()));
    }

    #[test]
    fn a_window_is_tried_again_only_after_a_pause() {
        let mut tries = Tries::default();
        let start = Instant::now();
        assert!(tries.due(1, start));
        assert!(!tries.due(1, start + POLL));
        assert!(tries.due(1, start + RETRY));
    }

    #[test]
    fn a_window_that_stays_is_given_up_on() {
        let mut tries = Tries::default();
        let start = Instant::now();
        let tried = (0..20).filter(|&i| tries.due(1, start + RETRY * i)).count();
        assert_eq!(tried, usize::from(MAX_TRIES));
    }

    #[test]
    fn windows_that_went_are_forgotten() {
        let mut tries = Tries::default();
        let start = Instant::now();
        tries.due(1, start);
        tries.due(2, start);
        tries.keep(&[2]);
        assert!(tries.due(1, start + POLL));
        assert!(!tries.due(2, start + POLL));
    }
}
