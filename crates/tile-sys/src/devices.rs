//! What the launcher shows and changes for the system tray items Explorer would otherwise
//! provide: the volume, the battery, the network and the Wi-Fi and Bluetooth radios.
//!
//! Each call initialises COM on the calling thread for its own duration, so call them from a
//! worker thread, not one with a message loop.

use windows::{
    Devices::{
        Bluetooth::{BluetoothConnectionStatus, BluetoothDevice},
        Enumeration::DeviceInformation,
        Radios::{Radio, RadioAccessStatus, RadioKind as WinRadioKind, RadioState},
    },
    Win32::{
        Media::Audio::{
            EDataFlow, Endpoints::IAudioEndpointVolume, IMMDevice, IMMDeviceEnumerator,
            MMDeviceEnumerator, eCapture, eConsole, eRender,
        },
        Networking::NetworkListManager::{
            INetwork, INetworkListManager, NLM_CONNECTIVITY_IPV4_INTERNET,
            NLM_CONNECTIVITY_IPV6_INTERNET, NLM_ENUM_NETWORK_CONNECTED, NetworkListManager,
        },
        System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoIncrementMTAUsage,
            CoInitializeEx, CoTaskMemFree, CoUninitialize,
        },
    },
    core::{GUID, IUnknown, IUnknown_Vtbl, PCWSTR, Result, interface},
};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use crate::wide;

/// Keeps COM initialised on this thread until dropped.
struct Com(bool);

impl Com {
    fn init() -> Self {
        // These calls run on short-lived threads. When the last one ends the shared apartment is
        // torn down, but the Windows Runtime caches the factories it made in it, so the next call
        // used freed memory and crashed the process now and then. Holding the apartment open
        // for the whole process keeps them valid.
        static APARTMENT: std::sync::Once = std::sync::Once::new();
        // SAFETY: takes no pointers; the usage count is never given back.
        APARTMENT.call_once(|| _ = unsafe { CoIncrementMTAUsage() });
        // SAFETY: takes no pointers. Initialising can fail if the thread is already in another
        // apartment mode, which is fine to run in; then there is nothing to undo.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() })
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `init`.
            unsafe { CoUninitialize() };
        }
    }
}

/// Which way audio goes: out to speakers and headphones, or in from a microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// To speakers and headphones.
    Output,
    /// From a microphone.
    Input,
}

impl Flow {
    fn data_flow(self) -> EDataFlow {
        match self {
            Self::Output => eRender,
            Self::Input => eCapture,
        }
    }
}

/// The volume of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    /// 0 to 100.
    pub percent: u8,
    /// Whether it is muted, whatever its volume.
    pub muted: bool,
}

/// The default device for `flow`. COM must be initialised, as a `Com` guard does.
fn default_device_of(flow: Flow) -> Option<IMMDevice> {
    // SAFETY: plain COM calls on interfaces this function owns.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        enumerator
            .GetDefaultAudioEndpoint(flow.data_flow(), eConsole)
            .ok()
    }
}

fn endpoint(flow: Flow) -> Option<IAudioEndpointVolume> {
    // SAFETY: a plain COM call on the device; COM is initialised by the caller's `Com` guard.
    unsafe {
        default_device_of(flow)?
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .ok()
    }
}

/// The volume and mute state of the default device, if there is one.
pub fn volume(flow: Flow) -> Option<Volume> {
    let _com = Com::init();
    let endpoint = endpoint(flow)?;
    // SAFETY: plain COM calls on the interface above.
    unsafe {
        let level = endpoint.GetMasterVolumeLevelScalar().ok()?;
        let muted = endpoint.GetMute().ok()?.as_bool();
        Some(Volume {
            percent: (level.clamp(0.0, 1.0) * 100.0).round() as u8,
            muted,
        })
    }
}

/// Sets the default device's volume, 0 to 100, and unmutes it.
pub fn set_volume(flow: Flow, percent: u8) -> bool {
    let _com = Com::init();
    let Some(endpoint) = endpoint(flow) else {
        return false;
    };
    let level = f32::from(percent.min(100)) / 100.0;
    // SAFETY: plain COM calls on the interface above; a null event context is allowed.
    unsafe {
        endpoint
            .SetMasterVolumeLevelScalar(level, std::ptr::null())
            .is_ok()
            && endpoint.SetMute(false, std::ptr::null()).is_ok()
    }
}

/// Mutes or unmutes the default device.
pub fn set_muted(flow: Flow, muted: bool) -> bool {
    let _com = Com::init();
    let Some(endpoint) = endpoint(flow) else {
        return false;
    };
    // SAFETY: a plain COM call on the interface above; a null event context is allowed.
    unsafe { endpoint.SetMute(muted, std::ptr::null()).is_ok() }
}

/// The endpoint id (`{0.0.0.00000000}.{guid}`) of the default device, as the system keeps it.
pub fn default_device(flow: Flow) -> Option<String> {
    let _com = Com::init();
    let device = default_device_of(flow)?;
    // SAFETY: a plain COM call; the id string is COM memory we free once after copying it.
    unsafe {
        let id = device.GetId().ok()?;
        let text = id.to_string().ok();
        CoTaskMemFree(Some(id.0.cast_const().cast()));
        text
    }
}

/// The class of the system's audio policy, which the sound settings use to change the default
/// device.
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// `IPolicyConfig`, which has no public header. It has had this layout since Vista; the methods
/// before `SetDefaultEndpoint` are placeholders that keep it in its slot.
#[interface("F8679F50-850A-41CF-9C72-430F290290C8")]
unsafe trait IPolicyConfig: IUnknown {
    fn _get_mix_format(&self);
    fn _get_device_format(&self);
    fn _reset_device_format(&self);
    fn _set_device_format(&self);
    fn _get_processing_period(&self);
    fn _set_processing_period(&self);
    fn _get_share_mode(&self);
    fn _set_share_mode(&self);
    fn _get_property_value(&self);
    fn _set_property_value(&self);
    fn set_default_endpoint(&self, id: PCWSTR, role: i32) -> Result<()>;
}

/// Makes the device with this endpoint id the default for every role (default, media and
/// communications). Returns whether Windows accepted it.
pub fn set_default_device(id: &str) -> bool {
    let _com = Com::init();
    let id = wide(id);
    // SAFETY: plain COM calls; `id` is NUL-terminated and outlives them.
    unsafe {
        let Ok(policy) =
            CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)
        else {
            return false;
        };
        (0..3).all(|role| {
            policy
                .set_default_endpoint(PCWSTR(id.as_ptr()), role)
                .is_ok()
        })
    }
}

/// The battery, on a machine that has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// How full it is, 0 to 100.
    pub percent: u8,
    /// Whether it is charging.
    pub charging: bool,
    /// Whether the machine runs on mains power.
    pub plugged_in: bool,
    /// On battery power: the minutes the system estimates are left.
    pub minutes_left: Option<u32>,
}

/// The battery's charge, or `None` on a machine without one.
pub fn battery() -> Option<Battery> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` is a live SYSTEM_POWER_STATUS to write to.
    if unsafe { GetSystemPowerStatus(&raw mut status) } == 0 {
        return None;
    }
    const NO_BATTERY: u8 = 128;
    const CHARGING: u8 = 8;
    const UNKNOWN: u8 = 255;
    if status.BatteryFlag == UNKNOWN || status.BatteryFlag & NO_BATTERY != 0 {
        return None;
    }
    let plugged_in = status.ACLineStatus == 1;
    Some(Battery {
        percent: status.BatteryLifePercent.min(100),
        charging: status.BatteryFlag & CHARGING != 0,
        plugged_in,
        minutes_left: (!plugged_in && status.BatteryLifeTime != u32::MAX)
            .then_some(status.BatteryLifeTime / 60),
    })
}

/// The network connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    /// The connected network's name, like the Wi-Fi network.
    pub name: Option<String>,
    /// Whether it reaches the internet.
    pub internet: bool,
}

/// The network connection, if the system can tell.
pub fn network() -> Option<Network> {
    let _com = Com::init();
    // SAFETY: plain COM calls on interfaces this function owns; `fetched` and `slot` are live
    // locals the calls write to.
    unsafe {
        let manager: INetworkListManager =
            CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
        let connectivity = manager.GetConnectivity().ok()?.0;
        let internet = connectivity
            & (NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0)
            != 0;
        let networks = manager.GetNetworks(NLM_ENUM_NETWORK_CONNECTED).ok()?;
        let mut name = None;
        loop {
            let mut slot: [Option<INetwork>; 1] = [None];
            let mut fetched = 0;
            if networks.Next(&mut slot, Some(&raw mut fetched)).is_err() || fetched == 0 {
                break;
            }
            if let Some(network) = slot[0].take()
                && let Ok(found) = network.GetName()
            {
                name = Some(found.to_string());
                break;
            }
        }
        Some(Network { name, internet })
    }
}

/// A paired Bluetooth device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// The name it was paired under.
    pub name: String,
    /// Whether it is connected now.
    pub connected: bool,
}

/// The Bluetooth devices that are paired with this machine, connected ones first. Asking each
/// one takes a moment, so this belongs on a worker thread.
pub fn paired_bluetooth() -> Vec<PairedDevice> {
    let _com = Com::init();
    let mut found = Vec::new();
    let devices = BluetoothDevice::GetDeviceSelectorFromPairingState(true)
        .and_then(|selector| DeviceInformation::FindAllAsyncAqsFilter(&selector))
        .and_then(|op| op.join());
    for info in devices.into_iter().flatten() {
        let Ok(id) = info.Id() else {
            continue;
        };
        let Ok(device) = BluetoothDevice::FromIdAsync(&id).and_then(|op| op.join()) else {
            continue;
        };
        let name = device.Name().map(|n| n.to_string()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let connected = device.ConnectionStatus() == Ok(BluetoothConnectionStatus::Connected);
        found.push(PairedDevice { name, connected });
    }
    found.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    found
}

/// A radio the system can switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioKind {
    /// The Wi-Fi radio.
    Wifi,
    /// The Bluetooth radio.
    Bluetooth,
}

impl RadioKind {
    fn windows(self) -> WinRadioKind {
        match self {
            Self::Wifi => WinRadioKind::WiFi,
            Self::Bluetooth => WinRadioKind::Bluetooth,
        }
    }
}

fn find_radio(kind: RadioKind) -> Option<Radio> {
    Radio::GetRadiosAsync()
        .ok()?
        .join()
        .ok()?
        .into_iter()
        .find(|r| r.Kind().ok() == Some(kind.windows()))
}

/// Whether the radio is on, or `None` if the machine has none.
pub fn radio(kind: RadioKind) -> Option<bool> {
    let _com = Com::init();
    find_radio(kind)?
        .State()
        .ok()
        .map(|state| state == RadioState::On)
}

/// Turns the radio on or off.
pub fn set_radio(kind: RadioKind, on: bool) -> bool {
    let _com = Com::init();
    let Some(radio) = find_radio(kind) else {
        return false;
    };
    let allowed = Radio::RequestAccessAsync()
        .ok()
        .and_then(|op| op.join().ok())
        .is_some_and(|status| status == RadioAccessStatus::Allowed);
    let state = if on { RadioState::On } else { RadioState::Off };
    allowed
        && radio
            .SetStateAsync(state)
            .ok()
            .and_then(|op| op.join().ok())
            .is_some_and(|status| status == RadioAccessStatus::Allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume_of_input() -> Option<Volume> {
        volume(Flow::Input)
    }

    /// Every call on a thread of its own, one after another, as the launcher does each time it
    /// opens. Before the apartment was held open this crashed the process now and then.
    #[test]
    fn reading_again_and_again_from_new_threads_is_safe() {
        for _ in 0..40 {
            std::thread::spawn(|| {
                let _ = (
                    volume(Flow::Output),
                    volume(Flow::Input),
                    radio(RadioKind::Wifi),
                    radio(RadioKind::Bluetooth),
                    paired_bluetooth(),
                    network(),
                );
            })
            .join()
            .expect("a read panicked");
        }
    }

    /// The machine decides what exists, so this only checks the calls run and agree.
    #[test]
    fn reads_the_system_without_failing() {
        let volume = volume(Flow::Output);
        if let Some(v) = volume {
            assert!(v.percent <= 100);
        }
        let _ = volume_of_input();
        let _ = default_device(Flow::Input);
        assert!(default_device(Flow::Output).is_none_or(|id| id.starts_with("{0.0.0.")));
        let _ = crate::wifi::networks();
        if let Some(b) = battery() {
            assert!(b.percent <= 100);
        }
        let _ = network();
        let _ = radio(RadioKind::Wifi);
        let _ = radio(RadioKind::Bluetooth);
        println!("volume {volume:?}, network {:?}", network());
    }
}
