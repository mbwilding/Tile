//! The shell's second half of showing a Store app: its frame.
//!
//! A Store app's `Windows.UI.Core.CoreWindow` starts hidden by the shell (DWM's shell cloak) and
//! has no title bar. Explorer fixes both: it asks `ApplicationFrameHost` for a frame, puts the app
//! window inside it, and lifts the cloak. With no Explorer, [`start`] does the same for every app
//! window it hears of (from window events, with a sweep each second for any missed), from a
//! thread of its own.
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
/// How many tries, a tick apart, frame one app window before giving up on it.
const MAX_TRIES: u8 = 20;
/// How often empty frames are looked for, and every window is checked in case an event was
/// missed.
const SWEEP: Duration = Duration::from_secs(1);
/// How many sweeps in a row a frame must be empty before it is destroyed.
const EMPTY_SWEEPS: u8 = 3;
/// How many ticks after framing an app is kept from staying frozen.
const RESUME_FOR: u32 = 200;
/// The pause between checks that keep frames and their apps the same size, while there are any.
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

/// Makes every internal call framing makes, once, on a throwaway window and frame, for
/// [`crate::probe`]. A wrong vtable slot either crashes the process or fails a check here.
pub(crate) fn probe() -> Result<(), crate::probe::Error> {
    use crate::probe::Error;
    com::join_apartment();
    let shell = Shell::take().map_err(Error::Inconclusive)?;
    let manager = com::frame_manager().map_err(Error::Failed)?;
    let frame = com::create_frame(&manager).map_err(Error::Failed)?;
    let window =
        com::frame_window(&frame).ok_or_else(|| Error::Failed("the frame has no window".into()))?;
    let class = crate::win::class_name(window);
    if class != "ApplicationFrameWindow" {
        return Err(Error::Failed(format!("a frame's window is a {class}")));
    }
    // What these answer for a window that is no app doesn't matter; that they return does. The
    // frame isn't given a window: the frame host never answers for one that is no app's.
    let _ = com::name(&frame, "Tile.Probe");
    let _ = com::frames(&manager);
    let _events = com::listen(&manager);
    if let Some(statics) = com::window_statics() {
        com::set_bounds(&statics, window, 100, 100);
    }
    shell.uncloak(&[shell.window]);
    com::resume("Tile.Probe_8wekyb3d8bbwe");
    com::destroy_frame(&manager, &frame);
    Ok(())
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
    /// App windows that weren't framed yet, tried again every tick.
    pending: Vec<HWND>,
    /// How many sweeps in a row each frame has held no app.
    empty: HashMap<usize, u8>,
    /// App windows already hidden when the framer started: Windows keeps apps like Settings
    /// running in the background, and showing those would open them unasked. They are framed once
    /// they take focus.
    dormant: HashSet<usize>,
}

/// How many times the shell window is tried for, and how long between tries.
const SHELL_TRIES: u32 = 30;
const SHELL_RETRY: Duration = Duration::from_millis(500);

/// Registers the shell window and takes its key. Windows keeps a process's key a moment after
/// the process ends, so a helper that replaces another is refused at first and is let in soon.
fn take_shell() -> Result<Shell, String> {
    let mut last = String::new();
    for _ in 0..SHELL_TRIES {
        match Shell::take() {
            Ok(shell) => return Ok(shell),
            Err(e) => last = e,
        }
        thread::sleep(SHELL_RETRY);
    }
    Err(last)
}

/// Frames app windows as window events report them, and keeps the frames in step with their
/// apps every tick while there are any. Without the events, every window is scanned each tick.
fn run() -> Result<(), String> {
    com::join_apartment();
    let mut framer = Framer::new(take_shell()?);
    let events = windows::WindowEvents::watch();
    if events.complete() {
        tracing::info!("framing Store app windows");
    } else {
        tracing::warn!("framing Store app windows, scanning for them without window events");
    }
    if let Err(e) = crate::immersive::register() {
        tracing::warn!("couldn't register the shell stand-in: {e}");
    }
    framer.leave_dormant();
    let mut next_tick = Instant::now();
    let mut next_sweep = next_tick + SWEEP;
    while pump_pending() {
        for app in events.take() {
            framer.consider(app);
        }
        let busy = !events.complete() || framer.busy();
        let now = Instant::now();
        if busy && now >= next_tick {
            if events.complete() {
                framer.retry();
            } else {
                framer.scan();
            }
            framer.follow();
            next_tick = now + TICK;
        }
        if now >= next_sweep {
            framer.scan();
            framer.sweep_empty();
            next_sweep = now + SWEEP;
        }
        let until = if framer.busy() || !events.complete() {
            next_tick.min(next_sweep)
        } else {
            next_sweep
        };
        wait(until.saturating_duration_since(Instant::now()));
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
            pending: Vec::new(),
            empty: HashMap::new(),
            dormant: HashSet::new(),
        }
    }

    /// Notes the app windows that are hidden now, to leave them be until something asks for one.
    fn leave_dormant(&mut self) {
        for app in windows::top_level_windows() {
            if windows::is_hidden_app_window(app) {
                self.dormant.insert(app as usize);
            }
        }
    }

    /// Whether anything needs the tick: frames to follow, or app windows to try again.
    fn busy(&self) -> bool {
        !self.frames.is_empty() || !self.pending.is_empty()
    }

    /// Frames every new Store app window the shell is still hiding.
    fn scan(&mut self) {
        self.tried.retain(|&app, _| is_window(app as HWND));
        self.dormant.retain(|&app| is_window(app as HWND));
        for app in windows::top_level_windows() {
            self.consider(app);
        }
    }

    /// Tries again the app windows that weren't framed yet.
    fn retry(&mut self) {
        for app in std::mem::take(&mut self.pending) {
            self.consider(app);
        }
    }

    /// Frames `app` if it is a Store app window the shell is still hiding and hasn't been tried
    /// too often. One that isn't framed this time is tried again next tick.
    fn consider(&mut self, app: HWND) {
        if self.dormant.contains(&(app as usize)) {
            if windows::foreground() != app {
                return;
            }
            self.dormant.remove(&(app as usize));
        }
        let attempts = self.tried.get(&(app as usize)).copied().unwrap_or(0);
        if attempts >= MAX_TRIES || !windows::is_hidden_app_window(app) {
            return;
        }
        self.tried.insert(app as usize, attempts + 1);
        let framed =
            windows::app_id(app).is_some_and(|(id, package)| match self.frame(app, &id, package) {
                Ok(frame) => {
                    tracing::info!("framed {id}");
                    self.frames.push(frame);
                    true
                }
                Err(e) => {
                    tracing::warn!("couldn't frame {id}: {e}");
                    false
                }
            });
        if !framed && !self.pending.contains(&app) {
            self.pending.push(app);
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
