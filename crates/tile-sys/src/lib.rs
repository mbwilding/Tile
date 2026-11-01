//! The operating system's services that Tile reads or drives: audio, power, network, Wi-Fi and
//! Bluetooth, the single-instance mutex, the console, and window messages.
//!
//! Like the other low-level crates, it holds `unsafe` and exposes a safe API.

pub mod devices;
mod process;
pub mod wifi;
mod window;

pub use process::{Instance, detach_console, on_console_close, single_instance};
pub use window::{post_app, post_close, unwatch_sessions, watch_sessions};
