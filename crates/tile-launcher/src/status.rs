//! Reading the system for the launcher's system items and carrying out their actions.

use crate::system::{
    Action, AudioDevice, Battery, BluetoothDevice, Network, Radio, Status, Volume, WifiNetwork,
    format_clock,
};
use tile_hook::{
    system::{self as sys, Flow},
    wifi,
};
use winsafe::{self as w, co};

fn radio_kind(radio: Radio) -> sys::RadioKind {
    match radio {
        Radio::Wifi => sys::RadioKind::Wifi,
        Radio::Bluetooth => sys::RadioKind::Bluetooth,
    }
}

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

fn volume_of(v: sys::Volume) -> Volume {
    Volume {
        percent: v.percent,
        muted: v.muted,
    }
}

/// Reads the audio, battery, network and radios. Some calls wait on Windows, so call this off
/// the UI thread.
pub fn status() -> Status {
    let bluetooth = sys::radio(sys::RadioKind::Bluetooth);
    Status {
        volume: sys::volume(Flow::Output).map(volume_of),
        mic: sys::volume(Flow::Input).map(volume_of),
        outputs: devices(Flow::Output),
        inputs: devices(Flow::Input),
        battery: sys::battery().map(|b| Battery {
            percent: b.percent,
            charging: b.charging,
            plugged_in: b.plugged_in,
            minutes_left: b.minutes_left,
        }),
        network: sys::network().map(|n| Network {
            name: n.name,
            internet: n.internet,
        }),
        wifi: sys::radio(sys::RadioKind::Wifi),
        wifi_networks: wifi::networks()
            .into_iter()
            .map(|n| WifiNetwork {
                profile: n.profile,
                name: n.ssid,
                signal: n.signal,
                connected: n.connected,
            })
            .collect(),
        bluetooth_devices: if bluetooth.is_some() {
            sys::paired_bluetooth()
                .into_iter()
                .map(|d| BluetoothDevice {
                    name: d.name,
                    connected: d.connected,
                })
                .collect()
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
        Action::SetRadio(radio, on) => sys::set_radio(radio_kind(*radio), *on),
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
