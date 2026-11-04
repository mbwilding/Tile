//! Windows integration for Tile, built on safe wrappers:
//! `winsafe` for Win32/DWM, `wineventhook` for window events, `tile-input` for the
//! low-level keyboard hooks, `uiautomation` for closing windows and `tray-icon` for the tray.

mod backdrop;
mod borders;
mod click_to_do;
pub mod desktops;
mod drag;
mod events;
mod focus_sink;
mod frame;
mod gui;
mod hotkeys;
mod indicator;
mod install;
mod lock;
mod monitors;
mod mover;
pub mod registry;
mod sign_in;
mod smartscreen;
mod startup;
mod tile_icon;
mod toast;
mod token;
mod tray;
mod windows;

pub use backdrop::Backdrop;
pub use borders::{Borders, Ring};
pub use click_to_do::{click_to_do_available, click_to_do_disabled, disable_click_to_do};
pub use drag::{DragEvent, WinDrag};
pub use events::{WinEvent, WinEventHooks};
pub use focus_sink::start_focus_sink;
pub use gui::{
    Mailbox, WindowThread, colorref, corner_preference, fill_round, fill_round_bordered, init_com,
    make_font, mix, paint_buffered, scale, set_alpha, set_antialiasing, set_transparent,
    style_popup, text, text_wrapped,
};
pub use hotkeys::{HotkeyEvent, Hotkeys};
pub use indicator::{Indicator, IndicatorSettings};
pub use install::{EXE, install_dir, installed_exe};
pub use lock::{lock_disabled, lock_workstation, request_lock_permission, set_lock_disabled};
pub use monitors::{monitor_paths, monitors, rect};
pub use mover::{Animation, Mover};
pub use sign_in::{
    conhost, headless_command, is_shell as is_sign_in_shell, set_shell as set_sign_in_shell,
};
pub use smartscreen::{
    request_smartscreen_permission, set_smartscreen_disabled, smartscreen_overridden,
};
pub use startup::{commands as startup_commands, first_this_session as first_startup_this_session};
pub use toast::{Toast, ToastSettings};
pub use token::{is_elevated, user_sid};
pub use tray::{Tray, TrayEvent};
pub use windows::{
    App, Discovered, PlaceError, Windows, close_foreground, cursor_pos, enumerate, focus_desktop,
    focus_exe, focus_hwnd, foreground, is_shell_window, list_apps, lowest_of, mouse_button_down,
    restore_windows, run_as_admin, send_to_bottom, set_cursor_pos, wait_for_exit,
};
