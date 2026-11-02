//! What Explorer normally provides, so Tile can run as the Windows shell: the notification area
//! (`Shell_TrayWnd`) that apps add their tray icons to, a black desktop backdrop, and the frames
//! and activation handshake the Settings and Store apps need.
//!
//! Like the other low-level crates, it holds `unsafe` and exposes a safe API.

pub mod backdrop;
mod frame;
mod icon;
mod immersive;
pub mod notify;
pub mod probe;
mod thread;
pub mod uwp;
mod win;

pub use icon::{Icon, monitor_dpi, shell_icon};
