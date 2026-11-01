//! Sending keys by scancode or virtual key code.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, SendInput,
};

use crate::keyboard::TAG;

/// A key to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A physical key by scancode, which Windows maps through the current layout.
    Scan { scan: u8, extended: bool, up: bool },
    /// A virtual key code.
    Vk { vk: u16, up: bool },
}

/// Sends keys, tagged with [`TAG`]. Returns how many were sent.
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
