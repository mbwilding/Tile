//! Wi-Fi networks through the WLAN API: which saved networks are in range, and connecting to
//! one or disconnecting. Only networks with a saved profile can be connected to from here;
//! joining a new one needs its password, which is Settings' job.

use std::ptr;

use windows_sys::{
    Win32::{
        Foundation::{ERROR_SUCCESS, HANDLE},
        NetworkManagement::WiFi::{
            WLAN_AVAILABLE_NETWORK, WLAN_AVAILABLE_NETWORK_CONNECTED,
            WLAN_AVAILABLE_NETWORK_HAS_PROFILE, WLAN_AVAILABLE_NETWORK_LIST,
            WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanConnect,
            WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList,
            WlanOpenHandle, WlanScan, dot11_BSS_type_any, wlan_connection_mode_profile,
        },
    },
    core::GUID,
};

use crate::wide;

/// A saved network that is in range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    /// The name of its saved profile, which connecting goes by.
    pub profile: String,
    /// The network's name as it is broadcast.
    pub ssid: String,
    /// 0 to 100.
    pub signal: u8,
    /// Whether this machine is connected to it.
    pub connected: bool,
}

/// An open WLAN client handle, closed on drop, with the first wireless adapter.
struct Wlan {
    handle: HANDLE,
    interface: GUID,
}

impl Wlan {
    /// Opens the API and finds the first adapter, or `None` without Wi-Fi.
    fn open() -> Option<Self> {
        let mut version = 0;
        let mut handle: HANDLE = ptr::null_mut();
        // SAFETY: `version` and `handle` are live locals the call writes to.
        if unsafe { WlanOpenHandle(2, ptr::null(), &raw mut version, &raw mut handle) }
            != ERROR_SUCCESS
        {
            return None;
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = ptr::null_mut();
        // SAFETY: `handle` is open; on success `list` is memory the API allocated, which is
        // freed below, and its first item exists when the count says so.
        let interface = unsafe {
            let status = WlanEnumInterfaces(handle, ptr::null(), &raw mut list);
            let interface =
                (status == ERROR_SUCCESS && !list.is_null() && (*list).dwNumberOfItems > 0)
                    .then(|| (*list).InterfaceInfo[0].InterfaceGuid);
            if !list.is_null() {
                WlanFreeMemory(list.cast());
            }
            interface
        };
        let Some(interface) = interface else {
            // SAFETY: closes the handle opened above.
            unsafe { WlanCloseHandle(handle, ptr::null()) };
            return None;
        };
        Some(Self { handle, interface })
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        // SAFETY: closes the handle `open` made, once.
        unsafe { WlanCloseHandle(self.handle, ptr::null()) };
    }
}

/// The text in `units` up to its first NUL.
fn from_wide(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// The saved networks in range, the connected one and the strongest first. Asks Windows to scan
/// again for next time.
pub fn networks() -> Vec<Network> {
    let Some(wlan) = Wlan::open() else {
        return Vec::new();
    };
    let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = ptr::null_mut();
    let mut found = Vec::new();
    // SAFETY: the handle and adapter are valid; on success `list` is memory the API allocated
    // with `dwNumberOfItems` entries, read here and freed below.
    unsafe {
        let status = WlanGetAvailableNetworkList(
            wlan.handle,
            &wlan.interface,
            0,
            ptr::null(),
            &raw mut list,
        );
        if status == ERROR_SUCCESS && !list.is_null() {
            let items = std::slice::from_raw_parts(
                (*list).Network.as_ptr(),
                (*list).dwNumberOfItems as usize,
            );
            found.extend(items.iter().filter_map(describe));
        }
        if !list.is_null() {
            WlanFreeMemory(list.cast());
        }
        // So the list is fresher next time; the answer arrives later.
        WlanScan(
            wlan.handle,
            &wlan.interface,
            ptr::null(),
            ptr::null(),
            ptr::null(),
        );
    }
    found.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.signal.cmp(&a.signal))
            .then(a.ssid.cmp(&b.ssid))
    });
    // The same network can be listed once for each of its access points.
    found.dedup_by(|a, b| a.profile == b.profile);
    found
}

fn describe(network: &WLAN_AVAILABLE_NETWORK) -> Option<Network> {
    if network.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE == 0 {
        return None;
    }
    let length = (network.dot11Ssid.uSSIDLength as usize).min(32);
    Some(Network {
        profile: from_wide(&network.strProfileName),
        ssid: String::from_utf8_lossy(&network.dot11Ssid.ucSSID[..length]).into_owned(),
        signal: network.wlanSignalQuality.min(100) as u8,
        connected: network.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
    })
}

/// Connects to the saved network with this profile name.
pub fn connect(profile: &str) -> bool {
    let Some(wlan) = Wlan::open() else {
        return false;
    };
    let name = wide(profile);
    let parameters = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: name.as_ptr(),
        pDot11Ssid: ptr::null_mut(),
        pDesiredBssidList: ptr::null_mut(),
        dot11BssType: dot11_BSS_type_any,
        dwFlags: 0,
    };
    // SAFETY: the handle and adapter are valid; `parameters` points at `name`, which outlives
    // the call.
    let status = unsafe { WlanConnect(wlan.handle, &wlan.interface, &parameters, ptr::null()) };
    status == ERROR_SUCCESS
}

/// Disconnects from the current Wi-Fi network.
pub fn disconnect() -> bool {
    let Some(wlan) = Wlan::open() else {
        return false;
    };
    // SAFETY: the handle and adapter are valid.
    let status = unsafe { WlanDisconnect(wlan.handle, &wlan.interface, ptr::null()) };
    status == ERROR_SUCCESS
}
