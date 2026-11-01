//! The shell's second half of showing a Store app: its frame.
//!
//! A Store app's `Windows.UI.Core.CoreWindow` starts hidden by the shell (DWM's shell cloak) and
//! has no title bar. Explorer fixes both: it asks `ApplicationFrameHost` for a frame, puts the app
//! window inside it, and lifts the cloak. With no Explorer, [`start`] does the same for every app
//! window it finds, from a thread of its own.
//!
//! Lifting the cloak needs Immersive App Management access, which Windows only gives to the
//! process that owns the shell window, so [`start`] registers a hidden one. Like [`crate::uwp`],
//! this rests on internals read from the debug symbols of Windows 11 (build 26100): the frame
//! manager's and frame's vtable layouts, and the `AcquireIAMKey` and `EnableIAMAccess` calls. If an
//! update changes them the failure is logged and Store app windows stay hidden.

mod com;
mod windows;

use std::{
    collections::{HashMap, HashSet},
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::Foundation::HWND;

use self::{
    com::{IApplicationFrame, IApplicationFrameEventHandler, IApplicationFrameManager},
    windows::Shell,
};
use crate::{
    thread::{pump_pending, wait},
    win::is_window,
};

/// How long an app gets to close after being asked before it is ended.
const CLOSE_GRACE: Duration = Duration::from_secs(3);
/// How many scans try to frame one app window before giving up on it.
const MAX_TRIES: u8 = 20;
/// How often empty frames are looked for.
const SWEEP_EVERY: u32 = 33;
/// How many sweeps in a row a frame must be empty before it is destroyed.
const EMPTY_SWEEPS: u8 = 3;
/// How often windows are checked for a new app to frame.
const SCAN_EVERY: u32 = 1;
/// How many ticks after framing an app is kept from staying frozen.
const RESUME_FOR: u32 = 200;
/// The pause between checks that keep frames and their apps the same size.
const TICK: Duration = Duration::from_millis(30);

/// Starts framing Store app windows on a thread of its own, unless Explorer is the shell.
pub(crate) fn start() {
    let spawned = thread::Builder::new()
        .name("store frames".into())
        .spawn(|| match run() {
            Ok(()) => tracing::info!("the Store app framer stopped"),
            Err(e) => tracing::warn!("Store app windows will stay hidden: {e}"),
        });
    if let Err(e) = spawned {
        tracing::warn!("couldn't start the Store app framer: {e}");
    }
}

/// A Store app window and the frame it sits in.
struct Frame {
    /// When the app was asked to close.
    closing: Option<Instant>,
    /// The frame object, which goes away with its last reference.
    com: IApplicationFrame,
    manager: IApplicationFrameManager,
    /// The app's package, which Windows may freeze while its window is hidden.
    package: String,
    /// Checks left that the app isn't frozen.
    resumes: u32,
    frame: HWND,
    app: HWND,
    size: (i32, i32),
    /// The app's title as last copied to the frame.
    title: Vec<u16>,
}

struct Framer {
    shell: Shell,
    manager: Option<IApplicationFrameManager>,
    /// Hears from every frame; kept alive for the life of the manager.
    events: Option<IApplicationFrameEventHandler>,
    /// The window management service, once found.
    statics: Option<com::IUniversalAppModelInProcStatics>,
    frames: Vec<Frame>,
    /// How many times each app window was tried, so one that keeps failing is given up on.
    tried: HashMap<usize, u8>,
    /// How many sweeps in a row each frame has held no app.
    empty: HashMap<usize, u8>,
}

fn run() -> Result<(), String> {
    com::join_apartment();
    let mut framer = Framer::new(Shell::take()?);
    tracing::info!("framing Store app windows");
    if let Err(e) = crate::immersive::register() {
        tracing::warn!("couldn't register the shell stand-in: {e}");
    }
    let mut tick = 0u32;
    while pump_pending() {
        if tick.is_multiple_of(SCAN_EVERY) {
            framer.scan();
        }
        if tick.is_multiple_of(SWEEP_EVERY) {
            framer.sweep_empty();
        }
        framer.follow();
        tick = tick.wrapping_add(1);
        wait(TICK);
    }
    Ok(())
}

impl Framer {
    fn new(shell: Shell) -> Self {
        Self {
            shell,
            manager: None,
            events: None,
            statics: None,
            frames: Vec::new(),
            tried: HashMap::new(),
            empty: HashMap::new(),
        }
    }

    /// Frames every new Store app window the shell is still hiding.
    fn scan(&mut self) {
        self.tried.retain(|&app, _| is_window(app as HWND));
        for app in windows::top_level_windows() {
            self.consider(app);
        }
    }

    /// Frames `app` if it is a Store app window the shell is still hiding and hasn't been tried
    /// too often.
    fn consider(&mut self, app: HWND) {
        let attempts = self.tried.get(&(app as usize)).copied().unwrap_or(0);
        if attempts >= MAX_TRIES || !windows::is_hidden_app_window(app) {
            return;
        }
        self.tried.insert(app as usize, attempts + 1);
        let Some((id, package)) = windows::app_id(app) else {
            return;
        };
        match self.frame(app, &id, package) {
            Ok(frame) => {
                tracing::info!("framed {id}");
                self.frames.push(frame);
            }
            Err(e) => tracing::warn!("couldn't frame {id}: {e}"),
        }
    }

    /// Makes a frame for `app`, shows the app in it and lifts the cloak on both.
    fn frame(&mut self, app: HWND, id: &str, package: String) -> Result<Frame, String> {
        let first = self.try_frame(app, id, package.clone());
        if first.is_err() && self.manager.take().is_some() {
            // `ApplicationFrameHost` may have restarted since the manager was made.
            return self.try_frame(app, id, package);
        }
        first
    }

    /// The frame manager, made (and listened to) the first time it is needed.
    fn manager(&mut self) -> Result<IApplicationFrameManager, String> {
        if let Some(manager) = &self.manager {
            return Ok(manager.clone());
        }
        let manager = com::frame_manager()?;
        self.manager = Some(manager.clone());
        self.events = com::listen(&manager);
        Ok(manager)
    }

    /// Destroys frames that hold no app and aren't being made: ones a previous helper left
    /// behind, or the frame host's own spares. Left alone they show as empty blue windows.
    fn sweep_empty(&mut self) {
        if self.manager.is_none()
            && !windows::top_level_windows()
                .into_iter()
                .any(windows::is_frame)
        {
            return;
        }
        let Ok(manager) = self.manager() else {
            return;
        };
        let mut seen = HashSet::new();
        for frame in com::frames(&manager) {
            let Some(window) = com::frame_window(&frame) else {
                continue;
            };
            if self.frames.iter().any(|f| f.frame == window) || windows::holds_app(window) {
                continue;
            }
            seen.insert(window as usize);
            let sweeps = self.empty.entry(window as usize).or_insert(0);
            *sweeps += 1;
            if *sweeps >= EMPTY_SWEEPS {
                com::destroy_frame(&manager, &frame);
            }
        }
        self.empty.retain(|window, _| seen.contains(window));
    }

    fn try_frame(&mut self, app: HWND, id: &str, package: String) -> Result<Frame, String> {
        let manager = self.manager()?;
        let frame = com::create_frame(&manager)?;
        let orphan = Orphan {
            manager: &manager,
            frame: &frame,
        };
        let window = com::present(&frame, app, id)?;
        // Everything is in place before the cloak lifts, which is when Tile hears of the window:
        // its title, and a size and place for it that Tile then replaces with a tile.
        let title = windows::copy_title(window, app, &[]);
        windows::place(window);
        self.shell.uncloak(&[window, app]);
        std::mem::forget(orphan);
        Ok(Frame {
            closing: None,
            frame: window,
            app,
            com: frame,
            manager,
            package,
            resumes: RESUME_FOR,
            size: (0, 0),
            title,
        })
    }

    /// Keeps each app the size of its frame, and closes whichever of the two outlives the other.
    fn follow(&mut self) {
        for request in com::take_close_requests() {
            let Some(window) = com::frame_window(&request) else {
                continue;
            };
            if let Some(f) = self.frames.iter_mut().find(|f| f.frame == window) {
                f.closing.get_or_insert_with(Instant::now);
                // Gone at once; the app gets its grace period to exit out of sight.
                windows::hide(f.frame);
                windows::post_close(f.app);
            }
        }
        if self.statics.is_none() {
            self.statics = com::window_statics();
        }
        let statics = self.statics.clone();
        self.frames.retain_mut(|f| {
            let (frame, app) = (is_window(f.frame), is_window(f.app));
            if !frame || !app {
                if frame {
                    com::destroy_frame(&f.manager, &f.com);
                } else if app {
                    windows::post_close(f.app);
                }
                return false;
            }
            if f.resumes > 0 {
                f.resumes -= 1;
                if f.resumes % 10 == 0 {
                    com::resume(&f.package);
                }
            }
            if f.closing.is_some_and(|asked| asked.elapsed() > CLOSE_GRACE) {
                windows::end_process(f.app);
            }
            let title = windows::copy_title(f.frame, f.app, &f.title);
            if !title.is_empty() {
                f.title = title;
            }
            let size = windows::client_size(f.frame);
            if size != f.size && size.0 > 0 && size.1 > 0 {
                f.size = size;
                if let Some(statics) = &statics {
                    com::set_bounds(statics, f.app, size.0, size.1);
                }
            }
            true
        });
    }
}

/// A frame that isn't in use yet, destroyed again if making it fails part way. Forgotten once it
/// is.
struct Orphan<'a> {
    manager: &'a IApplicationFrameManager,
    frame: &'a IApplicationFrame,
}

impl Drop for Orphan<'_> {
    fn drop(&mut self) {
        com::destroy_frame(self.manager, self.frame);
    }
}
