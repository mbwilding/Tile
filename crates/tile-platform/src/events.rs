use std::{io, ops::Range};

use tile_core::WindowId;
use tokio::sync::mpsc;
use wineventhook::{
    AccessibleObjectId, EventFilter, ObjectWindowEvent, SystemWindowEvent, WindowEvent,
    WindowEventHook, WindowEventType, raw_event,
};

/// A window event relevant to tiling, about the top-level window it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinEvent {
    /// The window was shown.
    Show(WindowId),
    /// The window was hidden.
    Hide(WindowId),
    /// The window was destroyed.
    Destroy(WindowId),
    /// The window came to the foreground.
    Foreground(WindowId),
    /// The window is being minimised.
    MinimizeStart(WindowId),
    /// The window was restored from minimised.
    MinimizeEnd(WindowId),
    /// The user started moving or resizing the window with the mouse.
    MoveSizeStart(WindowId),
    /// The user finished moving or resizing the window.
    MoveSizeEnd(WindowId),
    /// The window was cloaked (by the shell, for another desktop, or by its app).
    Cloaked(WindowId),
    /// The window was uncloaked.
    Uncloaked(WindowId),
    /// The window moved or changed size.
    LocationChange(WindowId),
    /// The window's title changed.
    NameChange(WindowId),
}

impl WinEvent {
    fn from_event(event: &WindowEvent) -> Option<Self> {
        let raw = WindowId(event.window_handle()?.as_ptr() as isize);
        Some(match event.event_type() {
            WindowEventType::System(e) => match e {
                SystemWindowEvent::Foreground => Self::Foreground(raw),
                SystemWindowEvent::MinimizeStart => Self::MinimizeStart(raw),
                SystemWindowEvent::MinimizeEnd => Self::MinimizeEnd(raw),
                SystemWindowEvent::MoveSizeStart => Self::MoveSizeStart(raw),
                SystemWindowEvent::MoveSizeEnd => Self::MoveSizeEnd(raw),
                _ => return None,
            },
            WindowEventType::Object(e) => match e {
                ObjectWindowEvent::Show => Self::Show(raw),
                ObjectWindowEvent::Hide => Self::Hide(raw),
                ObjectWindowEvent::Destroy => Self::Destroy(raw),
                ObjectWindowEvent::Cloaked => Self::Cloaked(raw),
                ObjectWindowEvent::Uncloaked => Self::Uncloaked(raw),
                ObjectWindowEvent::LocationChange => Self::LocationChange(raw),
                ObjectWindowEvent::NameChange => Self::NameChange(raw),
                _ => return None,
            },
            _ => return None,
        })
    }
}

/// Only events about whole windows, not their children (buttons, carets, ...).
fn is_window_event(event: &WindowEvent) -> bool {
    event.object_type() == AccessibleObjectId::Window && event.child_id().is_none()
}

/// Window event hooks. Events stop when this is dropped or [`WinEventHooks::unhook`]ed.
pub struct WinEventHooks {
    hooks: Vec<WindowEventHook>,
}

impl WinEventHooks {
    /// Installs out-of-context hooks for the event ranges the window manager cares about.
    pub async fn install() -> io::Result<(Self, mpsc::UnboundedReceiver<WinEvent>)> {
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<WindowEvent>();
        let ranges: [Range<i32>; 3] = [
            raw_event::SYSTEM_FOREGROUND..raw_event::SYSTEM_MINIMIZEEND + 1,
            raw_event::OBJECT_CREATE..raw_event::OBJECT_NAMECHANGE + 1,
            raw_event::OBJECT_CLOAKED..raw_event::OBJECT_UNCLOAKED + 1,
        ];
        let mut hooks = Vec::new();
        for range in ranges {
            let filter = EventFilter::default()
                .events(range)
                .skip_own_process(true)
                .predicate(is_window_event);
            hooks.push(WindowEventHook::hook(filter, raw_tx.clone()).await?);
        }
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(event) = raw_rx.recv().await {
                if let Some(e) = WinEvent::from_event(&event)
                    && tx.send(e).is_err()
                {
                    break;
                }
            }
        });
        Ok((Self { hooks }, rx))
    }

    /// Removes the hooks, after which no more events arrive.
    pub async fn unhook(self) {
        for hook in self.hooks {
            if let Err(e) = hook.unhook().await {
                tracing::warn!("failed to unhook window events: {e}");
            }
        }
    }
}
