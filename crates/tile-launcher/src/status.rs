//! Reading the system for the launcher's system items and carrying out their actions.

use crate::system::{Action, RadioKind, Status, format_clock};
use tile_sys::{
    devices::{self as sys, Flow},
    wifi,
};
use winsafe as w;

/// The local time and date, like `10:42 pm, Saturday 31 October 2026`.
fn clock() -> String {
    let t = w::GetLocalTime();
    format_clock(t.wYear, t.wMonth, t.wDay, t.wDayOfWeek, t.wHour, t.wMinute)
}

/// Reads the audio, battery, network and radios. Some calls wait on Windows, so call this off
/// the UI thread.
pub fn status() -> Status {
    let bluetooth = sys::radio(RadioKind::Bluetooth);
    Status {
        volume: sys::volume(Flow::Output),
        mic: sys::volume(Flow::Input),
        outputs: sys::audio_devices(Flow::Output),
        inputs: sys::audio_devices(Flow::Input),
        battery: sys::battery(),
        network: sys::network(),
        wifi: sys::radio(RadioKind::Wifi),
        wifi_networks: wifi::networks(),
        bluetooth_devices: if bluetooth.is_some() {
            sys::paired_bluetooth()
        } else {
            Vec::new()
        },
        bluetooth,
        clock: Some(clock()),
    }
}

/// Does what the audio, device, radio and Wi-Fi actions say; blocks like [`status`]. The other
/// actions start something and are the caller's to run (see [`command_line`]). Returns whether it
/// worked.
pub fn apply(action: &Action) -> bool {
    match action {
        Action::SetVolume(percent) => sys::set_volume(Flow::Output, *percent),
        Action::Mute(muted) => sys::set_muted(Flow::Output, *muted),
        Action::SetMicVolume(percent) => sys::set_volume(Flow::Input, *percent),
        Action::MuteMic(muted) => sys::set_muted(Flow::Input, *muted),
        Action::SwitchDevice(id) => sys::set_default_device(id),
        Action::SetRadio(radio, on) => sys::set_radio(*radio, *on),
        Action::WifiConnect(profile) => wifi::connect(profile),
        Action::WifiDisconnect => wifi::disconnect(),
        Action::Open(_) | Action::Power(_) => false,
    }
}

/// The command line that carries out an action that starts a program, if it is one.
pub fn command_line(action: &Action) -> Option<String> {
    match action {
        Action::Open(uri) => Some((*uri).to_owned()),
        Action::Power(power) => Some(power.command_line().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads everything the launcher shows; the machine decides what exists, so this only checks
    /// the pieces agree with each other.
    #[test]
    fn reads_the_whole_status() {
        let status = status();
        assert!(status.outputs.iter().filter(|d| d.default).count() <= 1);
        assert!(status.wifi_networks.iter().filter(|n| n.connected).count() <= 1);
        if status.bluetooth.is_none() {
            assert!(status.bluetooth_devices.is_empty());
        }
        println!(
            "outputs {}, inputs {}, wifi {:?} networks {}, bluetooth {:?} devices {:?}",
            status.outputs.len(),
            status.inputs.len(),
            status.wifi,
            status.wifi_networks.len(),
            status.bluetooth,
            status.bluetooth_devices
        );
    }
}
