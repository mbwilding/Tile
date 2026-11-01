//! Sending keys by scancode or virtual key code.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, SendInput,
};

/// `dwExtraInfo` on every key we send, to recognise our own input.
const TAG: usize = 0x4B45_5900;

/// A virtual key code that no key, layout or program uses (`0xE8` is unassigned). Windows opens
/// Start when Win goes up, and the menu bar when Alt does, unless another key was pressed while
/// it was down; a tap of this key counts as one without typing anything.
pub const SILENT_VK: u16 = 0xE8;

/// A key to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A physical key by scancode, which Windows maps through the current layout.
    Scan {
        /// The scancode, without its `E0` prefix.
        scan: u8,
        /// Whether the key has the `E0` prefix.
        extended: bool,
        /// A release rather than a press.
        up: bool,
    },
    /// A virtual key code.
    Vk {
        /// The virtual key code.
        vk: u16,
        /// A release rather than a press.
        up: bool,
    },
}

/// Sends keys, tagged as Tile's own. Returns how many were sent.
pub fn send(inputs: &[Input]) -> usize {
    let inputs: Vec<INPUT> = inputs
        .iter()
        .map(|input| {
            let (vk, scan, flags) = match *input {
                Input::Scan { scan, extended, up } => (
                    0,
                    u16::from(scan),
                    KEYEVENTF_SCANCODE
                        | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 }
                        | if up { KEYEVENTF_KEYUP } else { 0 },
                ),
                Input::Vk { vk, up } => (vk, 0, if up { KEYEVENTF_KEYUP } else { 0 }),
            };
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: scan,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: TAG,
                    },
                },
            }
        })
        .collect();
    if inputs.is_empty() {
        return 0;
    }
    // SAFETY: `inputs` is a live array of `inputs.len()` INPUTs, each of the size we pass.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    sent as usize
}

/// Sends a press and release of [`SILENT_VK`]. Returns whether both were sent.
pub fn tap_silent() -> bool {
    send(&[
        Input::Vk {
            vk: SILENT_VK,
            up: false,
        },
        Input::Vk {
            vk: SILENT_VK,
            up: true,
        },
    ]) == 2
}
