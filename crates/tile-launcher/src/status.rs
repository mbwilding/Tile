//! Reading the system for the launcher's system items and carrying out their actions.

use crate::system::{Action, AudioDevice, RadioKind, Status, format_clock};
use tile_sys::{
    devices::{self as sys, Flow},
    wifi,
};
use winsafe::{self as w, co};

/// The local time and date, like `10:42 pm, Saturday 31 October 2026`.
fn clock() -> String {
    let t = w::GetLocalTime();
    format_clock(t.wYear, t.wMonth, t.wDay, t.wDayOfWeek, t.wHour, t.wMinute)
}

const DEVICES_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio";
/// The device's kind (`Headphones`) and the hardware it is on (`RME Babyface Pro`).
const KIND_VALUE: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
const HARDWARE_VALUE: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},6";
/// `DeviceState` of one that is plugged in and enabled.
const ACTIVE: u32 = 1;

fn text_value(root: &w::HKEY, key: &str, name: &str) -> Option<String> {
    match root.RegGetValue(Some(key), Some(name), co::RRF::RT_REG_SZ) {
        Ok(w::RegistryValue::Sz(text)) => Some(text),
        _ => None,
    }
}

/// The active audio devices of a kind, with the one in use marked. Windows keeps them in the
/// registry; there is nothing to ask for their names that doesn't need COM property stores.
fn devices(flow: Flow) -> Vec<AudioDevice> {
    let (folder, prefix) = match flow {
        Flow::Output => ("Render", "{0.0.0.00000000}"),
        Flow::Input => ("Capture", "{0.0.1.00000000}"),
    };
    let root = w::HKEY::LOCAL_MACHINE;
    let base = format!(r"{DEVICES_KEY}\{folder}");
    let Ok(key) = root.RegOpenKeyEx(Some(&base), co::REG_OPTION::default(), co::KEY::READ) else {
        return Vec::new();
    };
    let Ok(names) = key.RegEnumKeyEx() else {
        return Vec::new();
    };
    let names: Vec<String> = names.filter_map(Result::ok).collect();
    let default = sys::default_device(flow);
    let mut out: Vec<AudioDevice> = names
        .into_iter()
        .filter_map(|guid| {
            let state = key.RegGetValue(Some(&guid), Some("DeviceState"), co::RRF::RT_REG_DWORD);
            if !matches!(state, Ok(w::RegistryValue::Dword(ACTIVE))) {
                return None;
            }
            let properties = format!(r"{base}\{guid}\Properties");
            let kind = text_value(&root, &properties, KIND_VALUE)?;
            let name = match text_value(&root, &properties, HARDWARE_VALUE) {
                Some(hardware) => format!("{kind} ({hardware})"),
                None => kind,
            };
            let id = format!("{prefix}.{guid}");
            Some(AudioDevice {
                default: default.as_deref() == Some(id.as_str()),
                id,
                name,
            })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Reads the audio, battery, network and radios. Some calls wait on Windows, so call this off
/// the UI thread.
pub fn status() -> Status {
    let bluetooth = sys::radio(RadioKind::Bluetooth);
    Status {
        volume: sys::volume(Flow::Output),
        mic: sys::volume(Flow::Input),
        outputs: devices(Flow::Output),
        inputs: devices(Flow::Input),
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

    /// The machine decides what exists, so this checks the shape: at most one device in use per
    /// direction, and every id in the form Windows uses.
    #[test]
    fn lists_audio_devices_with_one_in_use() {
        for (devices, prefix) in [
            (devices(Flow::Output), "{0.0.0."),
            (devices(Flow::Input), "{0.0.1."),
        ] {
            assert!(devices.iter().filter(|d| d.default).count() <= 1);
            assert!(
                devices
                    .iter()
                    .all(|d| d.id.starts_with(prefix) && !d.name.is_empty())
            );
            println!("{devices:?}");
        }
    }
}
