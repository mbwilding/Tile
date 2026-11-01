//! Keyboard and mouse input at the lowest level: `WH_KEYBOARD_LL` and `WH_MOUSE_LL` hooks that
//! can block events and report scancodes, `SendInput` by scancode, and hiding the pointer.
//!
//! Like the other low-level crates, it holds `unsafe` and exposes a safe API.

mod cursor;
mod hook;
mod keyboard;
mod mouse;
mod send;

pub use cursor::{hide_cursor, restore_cursors};
pub use keyboard::{Decision, Handler, KeyboardHook, RawKey, Waker, spawn};
pub use mouse::{MouseEvent, MouseHandler, MouseHook, spawn_mouse};
pub use send::{Input, SILENT_TAP, SILENT_VK, send, tap_silent};
