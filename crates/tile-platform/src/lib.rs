//! Windows integration for Tile, built on safe wrappers:
//! `winsafe` for Win32/DWM, `wineventhook` for window events, `tile-input` for the
//! low-level keyboard hooks, `uiautomation` for closing windows and `tray-icon` for the tray.

mod backdrop;
mod click_to_do;
pub mod desktops;
mod drag;
mod events;
mod gui;
mod hotkeys;
mod indicator;
mod lock;
mod monitors;
mod mover;
mod sign_in;
mod startup;
mod token;
mod tray;
mod windows;

pub use backdrop::Backdrop;
pub use click_to_do::{click_to_do_available, click_to_do_disabled, disable_click_to_do};
pub use drag::{DragEvent, WinDrag};
pub use events::{WinEvent, WinEventHooks};
pub use gui::{colorref, corner_preference, wake};
pub use hotkeys::{HotkeyEvent, Hotkeys};
pub use indicator::{Indicator, IndicatorSettings};
pub use lock::{GRANT_SCRIPT, lock_disabled, lock_workstation, set_lock_disabled};
pub use monitors::{monitors, rect};
pub use mover::{Animation, Insets, Mover};
pub use sign_in::{
    conhost, headless_command, installed_exe, is_shell as is_sign_in_shell,
    set_shell as set_sign_in_shell,
};
pub use startup::{commands as startup_commands, first_this_session as first_startup_this_session};
pub use token::{is_elevated, user_sid};
pub use tray::{Tray, TrayEvent};
pub use windows::{
    Discovered, PlaceError, Windows, close_foreground, cursor_pos, enumerate, focus_desktop,
    focus_exe, focus_hwnd, foreground, is_shell_window, mouse_button_down, restore_windows,
    run_as_admin, set_cursor_pos, wait_for_exit,
};
