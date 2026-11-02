//! The operating system's services that Tile reads or drives: audio, power, network, Wi-Fi and
//! Bluetooth, notifications, the single-instance mutex, the console, starting processes, window
//! messages and safe handles for raw window handles.
//!
//! Like the other low-level crates, it holds `unsafe` and exposes a safe API.

pub mod devices;
pub mod notifications;
mod process;
pub mod wifi;
mod window;

pub use process::{
    CREATE_NO_WINDOW, Instance, detach_console, hide_console, on_console_close, single_instance,
    spawn_detached,
};
pub use window::{
    class_name, empty_recycle_bin, hwnd, pid_of, post_app, post_close, recycle_bin_items,
    turn_off_display, unwatch_sessions, watch_sessions,
};

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
