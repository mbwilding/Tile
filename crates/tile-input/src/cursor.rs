//! Hiding the pointer by blanking the system cursors.

use std::ptr;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateCursor, DestroyCursor, OCR_APPSTARTING, OCR_CROSS, OCR_HAND, OCR_HELP, OCR_IBEAM, OCR_NO,
    OCR_NORMAL, OCR_SIZEALL, OCR_SIZENESW, OCR_SIZENS, OCR_SIZENWSE, OCR_SIZEWE, OCR_UP, OCR_WAIT,
    SPI_SETCURSORS, SYSTEM_CURSOR_ID, SetSystemCursor, SystemParametersInfoW,
};

/// The system cursors [`hide_cursor`] blanks out: every shape apps get from `LoadCursor`.
const CURSORS: [SYSTEM_CURSOR_ID; 14] = [
    OCR_NORMAL,
    OCR_IBEAM,
    OCR_WAIT,
    OCR_CROSS,
    OCR_UP,
    OCR_SIZENWSE,
    OCR_SIZENESW,
    OCR_SIZEWE,
    OCR_SIZENS,
    OCR_SIZEALL,
    OCR_NO,
    OCR_HAND,
    OCR_APPSTARTING,
    OCR_HELP,
];

/// Hides the pointer everywhere by swapping every system cursor for a transparent one, until
/// [`restore_cursors`]. Apps drawing their own cursor images still show them. Returns `false`
/// if some cursor couldn't be swapped.
pub fn hide_cursor() -> bool {
    // A 32x32 monochrome cursor: 4 bytes a row. AND bits set and XOR bits clear leave the
    // screen untouched.
    let and = [0xFF_u8; 32 * 4];
    let xor = [0_u8; 32 * 4];
    let mut all = true;
    for id in CURSORS {
        // SAFETY: both planes are live 32x32 1bpp bitmaps, the size given; a null instance is
        // allowed for a cursor not tied to a module.
        let blank = unsafe {
            CreateCursor(
                ptr::null_mut(),
                0,
                0,
                32,
                32,
                and.as_ptr().cast(),
                xor.as_ptr().cast(),
            )
        };
        if blank.is_null() {
            all = false;
            continue;
        }
        // SAFETY: `blank` is a cursor we own; on success the system takes it over (and
        // destroys it later), on failure it is still ours and destroyed here.
        unsafe {
            if SetSystemCursor(blank, id) == 0 {
                DestroyCursor(blank);
                all = false;
            }
        }
    }
    all
}

/// Reloads the user's cursor scheme, undoing [`hide_cursor`].
pub fn restore_cursors() -> bool {
    // SAFETY: `SPI_SETCURSORS` reads no parameter (null) and, with no flags, writes nothing.
    unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, ptr::null_mut(), 0) != 0 }
}
