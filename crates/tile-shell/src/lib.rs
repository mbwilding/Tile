//! What Explorer normally provides, so Tile can run as the Windows shell: the notification area
//! (`Shell_TrayWnd`) that apps add their tray icons to, a black desktop backdrop, the frames
//! and activation handshake the Settings and Store apps need, and bringing up UAC prompts that
//! wait for a taskbar click.
//!
//! Like the other low-level crates, it holds `unsafe` and exposes a safe API.

pub mod backdrop;
pub mod consent;
pub mod frame;
mod icon;
pub mod immersive;
pub mod notify;
pub mod uwp;

pub use icon::{Icon, monitor_dpi, render_hicon, shell_icon};
