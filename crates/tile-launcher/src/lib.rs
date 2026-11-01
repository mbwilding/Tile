//! The app launcher: a search box over everything Start lists, with a calculator, system items
//! (volume, Wi-Fi, power and the like) and the notification area's icons.
//!
//! The search, the calculator and the system items are plain logic and tested as such; the
//! window and the reading of the system are Win32.

pub mod calc;
pub mod input;
pub mod search;
mod status;
pub mod system;
mod window;

pub use status::{apply as apply_system_action, command_line as system_command_line};
pub use window::{Launcher, LauncherEvent, LauncherSettings};
