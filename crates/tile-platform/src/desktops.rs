//! Native Windows virtual desktops, via `winvd`.
//!
//! Tiling keeps its own workspaces, but every native desktop gets its own set of them.
//! All functions degrade gracefully (`None` / no-op) when the API is unavailable.

use std::{sync::mpsc, thread};

pub use tile_core::DesktopId;
use tile_core::{WindowId, command::DesktopTarget};
use tokio::sync::mpsc::UnboundedSender;
use windows_winvd::Win32::Foundation::HWND;
use winsafe::{self as w, co};
use winvd::{Desktop, DesktopEvent, DesktopEventThread};

/// Joins the multithreaded COM apartment, which the virtual desktop API requires.
/// Keep the guard alive for as long as COM is used on this thread.
pub fn init_com() -> Option<w::guard::CoUninitializeGuard> {
    w::CoInitializeEx(co::COINIT::MULTITHREADED)
        .inspect_err(|e| tracing::warn!("CoInitializeEx failed: {e}"))
        .ok()
}

/// A change to the native virtual desktops, from [`listen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopChange {
    /// The user switched to another desktop.
    Switched(DesktopId),
    /// A window moved between desktops or was (un)pinned.
    Window(WindowId),
    /// Desktops were created, removed or reordered.
    Changed,
}

fn hwnd(id: WindowId) -> HWND {
    HWND(id.0 as *mut std::ffi::c_void)
}

fn id_of(desktop: &Desktop) -> Option<DesktopId> {
    desktop.get_id().ok().map(|guid| guid.to_u128())
}

/// The desktop being shown.
pub fn current_desktop() -> Option<DesktopId> {
    winvd::get_current_desktop().ok().as_ref().and_then(id_of)
}

/// The desktop a window lives on (`None` for pinned windows or on failure).
pub fn desktop_of(id: WindowId) -> Option<DesktopId> {
    if is_pinned(id) {
        return None;
    }
    winvd::get_desktop_by_window(hwnd(id))
        .ok()
        .as_ref()
        .and_then(id_of)
}

/// Whether a window is shown on every desktop.
pub fn is_pinned(id: WindowId) -> bool {
    winvd::is_pinned_window(hwnd(id)).unwrap_or(false)
        || winvd::is_pinned_app(hwnd(id)).unwrap_or(false)
}

/// Toggles "Show this window on all desktops".
pub fn toggle_pin(id: WindowId) {
    let result = if winvd::is_pinned_window(hwnd(id)).unwrap_or(false) {
        winvd::unpin_window(hwnd(id))
    } else {
        winvd::pin_window(hwnd(id))
    };
    if let Err(e) = result {
        tracing::warn!("failed to toggle pin: {e:?}");
    }
}

fn resolve(target: DesktopTarget) -> Option<Desktop> {
    let count = winvd::get_desktop_count().ok()?;
    let current = winvd::get_current_desktop().ok()?.get_index().ok()?;
    let index = match target {
        DesktopTarget::Index(i) => i,
        DesktopTarget::Next => current + 1,
        DesktopTarget::Previous => current.checked_sub(1)?,
    };
    // Going past the last desktop creates a new one.
    if index >= count {
        return if index == count {
            winvd::create_desktop().ok()
        } else {
            None
        };
    }
    Some(winvd::get_desktop(index))
}

/// Switches to the desktop `target` names, creating one past the last.
pub fn switch_desktop(target: DesktopTarget) {
    if let Some(desktop) = resolve(target)
        && let Err(e) = winvd::switch_desktop(desktop)
    {
        tracing::warn!("failed to switch desktop: {e:?}");
    }
}

/// Moves a window to the desktop `target` names, creating one past the last.
pub fn move_to_desktop(id: WindowId, target: DesktopTarget) {
    if let Some(desktop) = resolve(target)
        && let Err(e) = winvd::move_window_to_desktop(desktop, &hwnd(id))
    {
        tracing::warn!("failed to move window to desktop: {e:?}");
    }
}

/// Listens for virtual desktop changes. Keep the returned value alive.
pub fn listen(tx: UnboundedSender<DesktopChange>) -> Option<DesktopEventThread> {
    let (raw_tx, raw_rx) = mpsc::channel::<DesktopEvent>();
    let listener = match winvd::listen_desktop_events(raw_tx) {
        Ok(listener) => listener,
        Err(e) => {
            tracing::warn!("virtual desktop events unavailable: {e:?}");
            return None;
        }
    };
    let bridge = thread::Builder::new()
        .name("desktop events".into())
        .spawn(move || {
            while let Ok(event) = raw_rx.recv() {
                let change = match event {
                    DesktopEvent::DesktopChanged { new, .. } => match id_of(&new) {
                        Some(id) => DesktopChange::Switched(id),
                        None => continue,
                    },
                    DesktopEvent::WindowChanged(h) => DesktopChange::Window(WindowId(h.0 as isize)),
                    DesktopEvent::DesktopCreated(_)
                    | DesktopEvent::DesktopDestroyed { .. }
                    | DesktopEvent::DesktopMoved { .. } => DesktopChange::Changed,
                    DesktopEvent::DesktopNameChanged(..)
                    | DesktopEvent::DesktopWallpaperChanged(..) => continue,
                };
                if tx.send(change).is_err() {
                    break;
                }
            }
        });
    if let Err(e) = bridge {
        tracing::warn!("failed to start desktop event bridge: {e}");
    }
    Some(listener)
}
