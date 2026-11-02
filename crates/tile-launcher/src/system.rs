//! The launcher's system items: the audio devices and their volume, network, Wi-Fi, Bluetooth,
//! battery, clock and power actions that Explorer's taskbar would otherwise offer. This builds
//! the rows from a snapshot of the system and searches them; reading and changing the system is
//! `tile-sys`'s.

pub use tile_sys::{
    devices::{AudioDevice, Battery, Network, PairedDevice, RadioKind, Volume},
    wifi::Network as WifiNetwork,
};

use crate::search::fuzzy;

/// A group of system rows, which the launcher shows on their own when its bar's button for the
/// group is picked, like the taskbar's quick settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// The speakers or headphones: their volume and which is in use.
    Output,
    /// The microphone: its volume and which is in use.
    Input,
    /// The network, the Wi-Fi radio and saved networks.
    Wifi,
    /// The Bluetooth radio and paired devices.
    Bluetooth,
    /// Not part of a panel: found by searching.
    Other,
}

impl Group {
    /// What the search box says while the group is open.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Output => "Sound output",
            Self::Input => "Microphone",
            Self::Wifi => "Wi-Fi",
            Self::Bluetooth => "Bluetooth",
            Self::Other => "",
        }
    }

    /// Whether the group's switch is on: sound and microphone not muted, the radio on. `None`
    /// when there is nothing to switch.
    pub fn is_on(self, status: &Status) -> Option<bool> {
        match self {
            Self::Output => status.volume.map(|v| !v.muted),
            Self::Input => status.mic.map(|v| !v.muted),
            Self::Wifi => status.wifi,
            Self::Bluetooth => status.bluetooth,
            Self::Other => None,
        }
    }

    /// The action that flips the group's switch.
    pub fn toggle(self, status: &Status) -> Option<Action> {
        let on = self.is_on(status)?;
        Some(match self {
            Self::Output => Action::Mute(on),
            Self::Input => Action::MuteMic(on),
            Self::Wifi => Action::SetRadio(RadioKind::Wifi, !on),
            Self::Bluetooth => Action::SetRadio(RadioKind::Bluetooth, !on),
            Self::Other => return None,
        })
    }
}

impl Status {
    /// Records what `action` will have done, so what is shown follows a switch before the
    /// system is read again.
    pub fn expect(&mut self, action: &Action) {
        match action {
            Action::Mute(muted) => {
                if let Some(v) = &mut self.volume {
                    v.muted = *muted;
                }
            }
            Action::MuteMic(muted) => {
                if let Some(v) = &mut self.mic {
                    v.muted = *muted;
                }
            }
            Action::SetRadio(RadioKind::Wifi, on) => self.wifi = self.wifi.map(|_| *on),
            Action::SetRadio(RadioKind::Bluetooth, on) => {
                self.bluetooth = self.bluetooth.map(|_| *on);
            }
            _ => {}
        }
    }
}

/// Ending the session or putting the machine away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    /// Lock the screen.
    Lock,
    /// Show the sign-in screen, leaving this session running.
    SwitchUser,
    /// Suspend the machine.
    Sleep,
    /// Save the session to disk and turn the machine off.
    Hibernate,
    /// End the session.
    SignOut,
    /// Restart the machine.
    Restart,
    /// Turn the machine off.
    ShutDown,
}

impl Power {
    /// The command line that does it.
    pub const fn command_line(self) -> &'static str {
        match self {
            Self::Lock => "rundll32.exe user32.dll,LockWorkStation",
            // Leaves the session running and shows the sign-in screen.
            Self::SwitchUser => "tsdisconnect.exe",
            Self::Sleep => "rundll32.exe powrprof.dll,SetSuspendState 0,1,0",
            Self::Hibernate => "shutdown.exe /h",
            Self::SignOut => "shutdown.exe /l",
            Self::Restart => "shutdown.exe /r /t 0",
            Self::ShutDown => "shutdown.exe /s /t 0",
        }
    }
}

impl Action {
    /// The glyph, in Windows' icon font, that stands for the action.
    pub const fn glyph(&self) -> char {
        match self {
            Self::SetVolume(_) | Self::Mute(_) | Self::SwitchDevice(_) => '\u{E767}',
            Self::SetMicVolume(_) | Self::MuteMic(_) => '\u{E720}',
            Self::SetRadio(RadioKind::Bluetooth, _) => '\u{E702}',
            Self::SetRadio(RadioKind::Wifi, _) | Self::WifiConnect(_) | Self::WifiDisconnect => {
                '\u{E701}'
            }
            Self::Open(_) => '\u{E713}',
            Self::Tile(_) => '\u{E765}',
            Self::DisplayOff => '\u{E7F4}',
            Self::EmptyRecycleBin => '\u{E74D}',
            Self::Power(_) => '\u{E7E8}',
        }
    }
}

/// The command line of the Recycle Bin row, which the row's icon is picked by.
pub const RECYCLE_BIN_COMMAND: &str = "explorer.exe shell:RecycleBinFolder";

/// What picking a system row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Set the output's volume, 0 to 100, unmuting it.
    SetVolume(u8),
    /// Mute (`true`) or unmute the output.
    Mute(bool),
    /// Set the microphone's volume, 0 to 100, unmuting it.
    SetMicVolume(u8),
    /// Mute (`true`) or unmute the microphone.
    MuteMic(bool),
    /// Make the device with this endpoint id the one in use, output or input.
    SwitchDevice(String),
    /// Turn a radio on (`true`) or off.
    SetRadio(RadioKind, bool),
    /// Connect to the saved Wi-Fi network with this profile name.
    WifiConnect(String),
    /// Disconnect from the Wi-Fi network.
    WifiDisconnect,
    /// Open a Settings page (an `ms-settings:` URI) or a classic tool, by its command line.
    Open(&'static str),
    /// End the session or put the machine away.
    Power(Power),
    /// Run a Tile dispatcher, written as in a bind.
    Tile(&'static str),
    /// Turn the monitors off until the next input.
    DisplayOff,
    /// Empty the Recycle Bin, after Windows' own confirmation.
    EmptyRecycleBin,
}

/// What the system is doing, as far as it could be read. Whatever is missing (no battery, no
/// Bluetooth) has no rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The default output's volume.
    pub volume: Option<Volume>,
    /// The default input's volume.
    pub mic: Option<Volume>,
    /// The output devices, with the one in use marked.
    pub outputs: Vec<AudioDevice>,
    /// The input devices, with the one in use marked.
    pub inputs: Vec<AudioDevice>,
    /// The saved Wi-Fi networks in range.
    pub wifi_networks: Vec<WifiNetwork>,
    /// The paired Bluetooth devices.
    pub bluetooth_devices: Vec<PairedDevice>,
    /// The battery, on a machine that has one.
    pub battery: Option<Battery>,
    /// The network connection.
    pub network: Option<Network>,
    /// Whether the Wi-Fi radio is on, if there is one.
    pub wifi: Option<bool>,
    /// Whether the Bluetooth radio is on, if there is one.
    pub bluetooth: Option<bool>,
    /// The time and date, ready to show.
    pub clock: Option<String>,
    /// How many items the Recycle Bin holds.
    pub recycle_bin: Option<u64>,
}

/// A row: what it is called, what it says underneath, and what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What the row says.
    pub title: String,
    /// What it says underneath, often what Enter does.
    pub subtitle: String,
    /// Other words that find it.
    pub keywords: &'static str,
    /// What picking it does.
    pub action: Action,
    /// The panel it belongs to.
    pub group: Group,
}

fn entry(
    title: &str,
    subtitle: impl Into<String>,
    keywords: &'static str,
    action: Action,
) -> Entry {
    Entry {
        title: title.into(),
        subtitle: subtitle.into(),
        keywords,
        action,
        group: Group::Other,
    }
}

impl Entry {
    /// The same row, in a panel.
    fn in_group(mut self, group: Group) -> Self {
        self.group = group;
        self
    }
}

const VOLUME_STEP: u8 = 10;

/// What tells the output's volume rows from the microphone's.
struct VolumeRows {
    /// `Volume` or `Microphone`.
    name: &'static str,
    /// The keywords of the mute, up and down rows.
    keywords: [&'static str; 3],
    mute: fn(bool) -> Action,
    set: fn(u8) -> Action,
    group: Group,
}

const OUTPUT_ROWS: VolumeRows = VolumeRows {
    name: "Volume",
    keywords: [
        "sound audio speakers mute unmute",
        "louder sound audio",
        "quieter sound audio",
    ],
    mute: Action::Mute,
    set: Action::SetVolume,
    group: Group::Output,
};

const INPUT_ROWS: VolumeRows = VolumeRows {
    name: "Microphone",
    keywords: [
        "mic input audio mute unmute",
        "mic input louder",
        "mic input quieter",
    ],
    mute: Action::MuteMic,
    set: Action::SetMicVolume,
    group: Group::Input,
};

/// The mute, up and down rows of a volume.
fn volume_entries(v: Volume, rows: &VolumeRows) -> [Entry; 3] {
    let name = rows.name;
    let (title, hint) = if v.muted {
        (format!("{name}: muted"), "Enter to unmute")
    } else {
        (format!("{name}: {}%", v.percent), "Enter to mute")
    };
    let up = v.percent.saturating_add(VOLUME_STEP).min(100);
    let down = v.percent.saturating_sub(VOLUME_STEP);
    let [mute_words, up_words, down_words] = rows.keywords;
    [
        entry(&title, hint, mute_words, (rows.mute)(!v.muted)),
        entry(
            &format!("{name} up"),
            format!("To {up}%"),
            up_words,
            (rows.set)(up),
        ),
        entry(
            &format!("{name} down"),
            format!("To {down}%"),
            down_words,
            (rows.set)(down),
        ),
    ]
    .map(|e| e.in_group(rows.group))
}

fn radio_entry(
    title: &str,
    keywords: &'static str,
    radio: RadioKind,
    on: Option<bool>,
) -> Option<Entry> {
    let on = on?;
    let (state, hint) = if on {
        ("On", "Enter to turn off")
    } else {
        ("Off", "Enter to turn on")
    };
    Some(entry(
        title,
        format!("{state}. {hint}"),
        keywords,
        Action::SetRadio(radio, !on),
    ))
}

fn battery_subtitle(b: &Battery) -> String {
    if b.charging {
        "Charging".to_owned()
    } else if b.plugged_in {
        "Plugged in".to_owned()
    } else if let Some(minutes) = b.minutes_left {
        format!("On battery, {}h {:02}m left", minutes / 60, minutes % 60)
    } else {
        "On battery".to_owned()
    }
}

/// A row for switching to an audio device.
fn device_entry(kind: &str, device: &AudioDevice, keywords: &'static str) -> Entry {
    let subtitle = if device.default {
        "In use"
    } else {
        "Enter to switch to it"
    };
    entry(
        &format!("{kind}: {}", device.name),
        subtitle,
        keywords,
        Action::SwitchDevice(device.id.clone()),
    )
}

/// Every row for `status`, in the order they are listed.
pub fn entries(status: &Status) -> Vec<Entry> {
    let mut out = Vec::new();
    if let Some(v) = status.volume {
        out.extend(volume_entries(v, &OUTPUT_ROWS));
    }
    for device in &status.outputs {
        out.push(
            device_entry("Output", device, "speakers headphones sound audio")
                .in_group(Group::Output),
        );
    }
    if let Some(m) = status.mic {
        out.extend(volume_entries(m, &INPUT_ROWS));
    }
    for device in &status.inputs {
        out.push(device_entry("Input", device, "microphone mic audio").in_group(Group::Input));
    }
    if let Some(n) = &status.network {
        let title = match (&n.name, n.internet) {
            (Some(name), _) => format!("Network: {name}"),
            (None, true) => "Network: connected".to_owned(),
            (None, false) => "Network: not connected".to_owned(),
        };
        let subtitle = if n.internet {
            "Connected to the internet. Enter for network settings"
        } else {
            "No internet. Enter for network settings"
        };
        out.push(
            entry(
                &title,
                subtitle,
                "internet wifi wi-fi ethernet connection",
                Action::Open("ms-settings:network"),
            )
            .in_group(Group::Wifi),
        );
    }
    out.extend(
        radio_entry(
            "Wi-Fi",
            "wifi wireless network",
            RadioKind::Wifi,
            status.wifi,
        )
        .map(|e| e.in_group(Group::Wifi)),
    );
    for network in &status.wifi_networks {
        let (subtitle, action) = if network.connected {
            (
                format!("Connected, signal {}%. Enter to disconnect", network.signal),
                Action::WifiDisconnect,
            )
        } else {
            (
                format!(
                    "Saved network, signal {}%. Enter to connect",
                    network.signal
                ),
                Action::WifiConnect(network.profile.clone()),
            )
        };
        out.push(
            entry(
                &format!("Wi-Fi: {}", network.ssid),
                subtitle,
                "wifi wi-fi wireless network connect",
                action,
            )
            .in_group(Group::Wifi),
        );
    }
    if status.wifi.is_some() {
        out.push(
            entry(
                "Wi-Fi settings",
                "Enter to join a new network",
                "wifi wi-fi wireless network add",
                Action::Open("ms-settings:network-wifi"),
            )
            .in_group(Group::Wifi),
        );
    }
    out.extend(
        radio_entry(
            "Bluetooth",
            "wireless devices",
            RadioKind::Bluetooth,
            status.bluetooth,
        )
        .map(|e| e.in_group(Group::Bluetooth)),
    );
    for device in &status.bluetooth_devices {
        let state = if device.connected {
            "Connected"
        } else {
            "Paired"
        };
        out.push(
            entry(
                &format!("Bluetooth: {}", device.name),
                format!("{state}. Enter for Bluetooth settings to connect or remove it"),
                "bluetooth wireless headphones device paired",
                Action::Open("ms-settings:bluetooth"),
            )
            .in_group(Group::Bluetooth),
        );
    }
    if status.bluetooth.is_some() {
        out.push(
            entry(
                "Bluetooth devices",
                "Enter for Bluetooth settings, to pair or connect",
                "bluetooth wireless headphones pair connect",
                Action::Open("ms-settings:bluetooth"),
            )
            .in_group(Group::Bluetooth),
        );
    }
    if let Some(b) = &status.battery {
        out.push(entry(
            &format!("Battery: {}%", b.percent),
            format!("{}. Enter for battery settings", battery_subtitle(b)),
            "power charge charging",
            Action::Open("ms-settings:batterysaver"),
        ));
    }
    if let Some(clock) = &status.clock {
        out.push(entry(
            "Clock",
            format!("{clock}. Enter for date and time settings"),
            "time date calendar",
            Action::Open("ms-settings:dateandtime"),
        ));
    }
    out.push(entry(
        "Display",
        "Enter for display settings: resolution, scale, layout and brightness",
        "screen monitor resolution scale refresh rate orientation hdr brightness arrangement multiple displays",
        Action::Open("ms-settings:display"),
    ));
    for (title, command, subtitle, keywords) in [
        (
            "Screens: PC screen only",
            "DisplaySwitch.exe /internal",
            "Use only the main display",
            "display monitor project projector layout",
        ),
        (
            "Screens: Duplicate",
            "DisplaySwitch.exe /clone",
            "Show the same on every display",
            "display monitor project projector mirror layout",
        ),
        (
            "Screens: Extend",
            "DisplaySwitch.exe /extend",
            "Join the displays into one desktop",
            "display monitor project projector layout",
        ),
        (
            "Screens: Second screen only",
            "DisplaySwitch.exe /external",
            "Use only the other display",
            "display monitor project projector layout",
        ),
    ] {
        out.push(entry(title, subtitle, keywords, Action::Open(command)));
    }
    out.push(entry(
        "Turn off display",
        "Monitors go dark until you move the mouse or press a key",
        "screen monitor off blank",
        Action::DisplayOff,
    ));
    let items = status.recycle_bin;
    out.push(entry(
        "Recycle Bin",
        match items {
            Some(0) => "Empty. Enter to open it in File Explorer".to_owned(),
            Some(1) => "1 item. Enter to open it in File Explorer".to_owned(),
            Some(n) => format!("{n} items. Enter to open it in File Explorer"),
            None => "Enter to open it in File Explorer".to_owned(),
        },
        "trash deleted files restore",
        Action::Open(RECYCLE_BIN_COMMAND),
    ));
    if items != Some(0) {
        out.push(entry(
            "Empty Recycle Bin",
            "Windows asks to confirm first",
            "trash delete files clear",
            Action::EmptyRecycleBin,
        ));
    }
    for (title, uri, keywords) in [
        (
            "Night light",
            "ms-settings:nightlight",
            "display warm blue light colour temperature",
        ),
        (
            "Airplane mode",
            "ms-settings:network-airplanemode",
            "flight radio wireless",
        ),
    ] {
        out.push(entry(
            title,
            "Enter to open its settings",
            keywords,
            Action::Open(uri),
        ));
    }
    for (title, line, subtitle, keywords) in [
        (
            "Next wallpaper",
            "wallpaper next",
            "Needs Tile to paint the desktop (shell mode)",
            "background image change",
        ),
        (
            "Previous wallpaper",
            "wallpaper prev",
            "Needs Tile to paint the desktop (shell mode)",
            "background image back",
        ),
        (
            "Random wallpaper",
            "wallpaper random",
            "Needs Tile to paint the desktop (shell mode)",
            "background image shuffle",
        ),
        (
            "Pause wallpaper cycle",
            "wallpaper pause",
            "Stops or restarts the timed change",
            "background image resume timer",
        ),
        (
            "Toggle tiling",
            "toggletile",
            "Pause or resume the window manager",
            "tile windows",
        ),
        (
            "Pause Tile",
            "togglepause",
            "Pause or resume tiling and remapping together",
            "tile resume",
        ),
        (
            "Toggle key remapper",
            "toggleremap",
            "Pause or resume remapping",
            "tile keys remap",
        ),
        (
            "Reload config",
            "reload",
            "Read Tile's config again",
            "tile settings refresh",
        ),
        (
            "Reset layout",
            "resetlayout",
            "Undo resizing and flipped splits on this workspace",
            "tile windows",
        ),
        (
            "Do not disturb",
            "togglednd",
            "Mute or show Tile's notification popups",
            "notifications focus quiet toast",
        ),
        (
            "Open config",
            "open config",
            "Edit Tile's config file",
            "tile settings yaml",
        ),
        (
            "Open config folder",
            "open folder",
            "Show the folder of Tile's config file",
            "tile settings",
        ),
        (
            "Open logs",
            "open logs",
            "Show the folder of Tile's logs",
            "tile log debug",
        ),
        ("Quit Tile", "exit", "Stop Tile", "tile exit close"),
    ] {
        out.push(entry(title, subtitle, keywords, Action::Tile(line)));
    }
    for (title, command, keywords) in [
        (
            "System Properties",
            "SystemPropertiesAdvanced.exe",
            "advanced performance startup recovery computer name remote",
        ),
        (
            "Environment Variables",
            "rundll32 sysdm.cpl,EditEnvironmentVariables",
            "edit the system environment variables path env",
        ),
        ("Control Panel", "control.exe", "classic settings"),
        ("Device Manager", "devmgmt.msc", "drivers hardware"),
        ("Services", "services.msc", "windows services"),
    ] {
        out.push(entry(
            title,
            "Enter to open it",
            keywords,
            Action::Open(command),
        ));
    }
    for (title, keywords, power) in [
        ("Lock", "lock screen", Power::Lock),
        ("Switch user", "change account", Power::SwitchUser),
        ("Sleep", "suspend", Power::Sleep),
        ("Hibernate", "suspend to disk", Power::Hibernate),
        ("Sign out", "log off logout", Power::SignOut),
        ("Restart", "reboot", Power::Restart),
        ("Shut down", "power off turn off", Power::ShutDown),
    ] {
        out.push(entry(title, "Power", keywords, Action::Power(power)));
    }
    out
}

/// `volume 30` or `vol 30%` for the output, `mic 30` or `microphone 30%` for the input: whether
/// the input, and the percentage asked for.
fn volume_request(query: &str) -> Option<(bool, u8)> {
    let (word, number) = query.trim().split_once(char::is_whitespace)?;
    let word = word.to_lowercase();
    let input = match word.as_str() {
        "volume" | "vol" => false,
        "mic" | "microphone" => true,
        _ => return None,
    };
    let percent: u8 = number.trim().trim_end_matches('%').parse().ok()?;
    (percent <= 100).then_some((input, percent))
}

/// The `rows` that match `query` (already trimmed), best first; all of them, in order, for an
/// empty query. A match on the title counts for more than one on the keywords.
fn rank_entries(query: &str, rows: impl IntoIterator<Item = Entry>) -> Vec<Entry> {
    if query.is_empty() {
        return rows.into_iter().collect();
    }
    let mut scored: Vec<(f64, Entry)> = rows
        .into_iter()
        .filter_map(|e| {
            let by_title = fuzzy(query, &e.title);
            let by_keywords = fuzzy(query, e.keywords).map(|s| s * 0.5);
            by_title
                .into_iter()
                .chain(by_keywords)
                .max_by(f64::total_cmp)
                .map(|score| (score, e))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().map(|(_, e)| e).collect()
}

/// The rows for `status` that match `query`, best first. An empty query matches them all.
pub fn find(query: &str, status: &Status) -> Vec<Entry> {
    let query = query.trim();
    let mut out = Vec::new();
    match volume_request(query) {
        Some((false, percent)) if status.volume.is_some() => out.push(entry(
            &format!("Set volume to {percent}%"),
            "Enter to set it",
            "",
            Action::SetVolume(percent),
        )),
        Some((true, percent)) if status.mic.is_some() => out.push(entry(
            &format!("Set microphone volume to {percent}%"),
            "Enter to set it",
            "",
            Action::SetMicVolume(percent),
        )),
        _ => {}
    }
    out.extend(rank_entries(query, entries(status)));
    out
}

/// The rows of one panel that match `query`, best first; all of them, in order, for an empty
/// query.
pub fn find_group(group: Group, query: &str, status: &Status) -> Vec<Entry> {
    let rows = entries(status).into_iter().filter(|e| e.group == group);
    rank_entries(query.trim(), rows)
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `hour` (0 to 23) on a 12 hour clock, and `am` or `pm`.
const fn hour12(hour: u16) -> (u16, &'static str) {
    match hour {
        0 => (12, "am"),
        1..=11 => (hour, "am"),
        12 => (12, "pm"),
        _ => (hour - 12, "pm"),
    }
}

/// `10:42 pm, Saturday 31 October 2026`. `weekday` counts from Sunday = 0, `month` from 1.
pub fn format_clock(
    year: u16,
    month: u16,
    day: u16,
    weekday: u16,
    hour: u16,
    minute: u16,
) -> String {
    let (hour, suffix) = hour12(hour);
    let date = format_date(year, month, day, weekday);
    format!("{hour}:{minute:02} {suffix}, {date}")
}

/// `10:42:07 pm`.
pub fn format_time(hour: u16, minute: u16, second: u16) -> String {
    let (hour, suffix) = hour12(hour);
    format!("{hour}:{minute:02}:{second:02} {suffix}")
}

/// `Saturday 31 October 2026`. `weekday` counts from Sunday = 0, `month` from 1.
pub fn format_date(year: u16, month: u16, day: u16, weekday: u16) -> String {
    format!(
        "{} {day} {} {year}",
        WEEKDAYS[usize::from(weekday) % 7],
        MONTHS[usize::from(month.saturating_sub(1)) % 12],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> Status {
        Status {
            volume: Some(Volume {
                percent: 45,
                muted: false,
            }),
            battery: Some(Battery {
                percent: 80,
                charging: false,
                plugged_in: false,
                minutes_left: Some(125),
            }),
            network: Some(Network {
                name: Some("Home".into()),
                internet: true,
            }),
            wifi: Some(true),
            bluetooth: Some(false),
            clock: Some("10:42 pm, Saturday 31 October 2026".into()),
            recycle_bin: Some(3),
            mic: Some(Volume {
                percent: 70,
                muted: false,
            }),
            outputs: vec![
                device(
                    "{0.0.0.00000000}.{a}",
                    "Headphones (RME Babyface Pro)",
                    true,
                ),
                device("{0.0.0.00000000}.{b}", "Speakers (RME Babyface Pro)", false),
            ],
            inputs: vec![device(
                "{0.0.1.00000000}.{c}",
                "Mic (RME Babyface Pro)",
                true,
            )],
            bluetooth_devices: vec![PairedDevice {
                name: "WH-1000XM5".into(),
                connected: true,
            }],
            wifi_networks: vec![
                WifiNetwork {
                    profile: "Home".into(),
                    ssid: "Home".into(),
                    signal: 80,
                    connected: true,
                },
                WifiNetwork {
                    profile: "Cafe".into(),
                    ssid: "Cafe wifi".into(),
                    signal: 40,
                    connected: false,
                },
            ],
        }
    }

    fn device(id: &str, name: &str, default: bool) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            name: name.into(),
            default,
        }
    }

    #[test]
    fn audio_devices_can_be_switched_to() {
        let rows = find("speakers", &status());
        let output = rows
            .iter()
            .find(|e| e.title.starts_with("Output:"))
            .unwrap();
        assert_eq!(output.title, "Output: Speakers (RME Babyface Pro)");
        assert_eq!(output.subtitle, "Enter to switch to it");
        assert_eq!(
            output.action,
            Action::SwitchDevice("{0.0.0.00000000}.{b}".into())
        );
        let rows = find("headphones", &status());
        let current = rows
            .iter()
            .find(|e| e.title.starts_with("Output:"))
            .unwrap();
        assert_eq!(current.subtitle, "In use");
        let inputs = find("mic", &status());
        assert!(
            inputs
                .iter()
                .any(|e| e.title == "Input: Mic (RME Babyface Pro)")
        );
    }

    #[test]
    fn the_microphone_has_its_own_mute_and_volume() {
        let rows = find("microphone", &status());
        let mute = rows.iter().find(|e| e.title == "Microphone: 70%").unwrap();
        assert_eq!(mute.action, Action::MuteMic(true));
        let up = rows.iter().find(|e| e.title == "Microphone up").unwrap();
        assert_eq!(up.action, Action::SetMicVolume(80));
        let typed = find("mic 25", &status());
        assert_eq!(typed[0].action, Action::SetMicVolume(25));
        assert_eq!(typed[0].title, "Set microphone volume to 25%");
        // Volume stays the output's.
        assert_eq!(
            find("volume 25", &status())[0].action,
            Action::SetVolume(25)
        );
    }

    #[test]
    fn saved_wifi_networks_connect_and_disconnect() {
        let rows = find("wifi", &status());
        let home = rows.iter().find(|e| e.title == "Wi-Fi: Home").unwrap();
        assert_eq!(home.action, Action::WifiDisconnect);
        let cafe = rows.iter().find(|e| e.title == "Wi-Fi: Cafe wifi").unwrap();
        assert_eq!(cafe.action, Action::WifiConnect("Cafe".into()));
        assert!(cafe.subtitle.contains("signal 40%"));
    }

    #[test]
    fn a_panel_holds_only_its_own_rows() {
        let output: Vec<_> = find_group(Group::Output, "", &status())
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(
            output,
            [
                "Volume: 45%",
                "Volume up",
                "Volume down",
                "Output: Headphones (RME Babyface Pro)",
                "Output: Speakers (RME Babyface Pro)",
            ]
        );
        let input = find_group(Group::Input, "", &status());
        assert!(
            input
                .iter()
                .all(|e| e.title.starts_with("Microphone") || e.title.starts_with("Input"))
        );
        let narrowed = find_group(Group::Output, "speak", &status());
        assert_eq!(narrowed[0].title, "Output: Speakers (RME Babyface Pro)");
        assert!(
            find_group(Group::Other, "", &status())
                .iter()
                .any(|e| e.title == "Clock")
        );
    }

    #[test]
    fn a_group_reports_and_flips_its_switch() {
        let mut s = status();
        assert_eq!(Group::Output.is_on(&s), Some(true));
        assert_eq!(Group::Wifi.is_on(&s), Some(true));
        assert_eq!(Group::Other.is_on(&s), None);
        let flip = Group::Output.toggle(&s).unwrap();
        assert_eq!(flip, Action::Mute(true));
        s.expect(&flip);
        assert_eq!(Group::Output.is_on(&s), Some(false));
        assert_eq!(Group::Output.toggle(&s), Some(Action::Mute(false)));
        let flip = Group::Wifi.toggle(&s).unwrap();
        s.expect(&flip);
        assert_eq!(Group::Wifi.is_on(&s), Some(false));
        s.bluetooth = None;
        assert_eq!(Group::Bluetooth.toggle(&s), None);
    }

    #[test]
    fn the_bluetooth_panel_lists_paired_devices() {
        let rows = find_group(Group::Bluetooth, "", &status());
        let titles: Vec<_> = rows.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Bluetooth", "Bluetooth: WH-1000XM5", "Bluetooth devices"]
        );
        assert!(rows[1].subtitle.starts_with("Connected"));
    }

    #[test]
    fn bluetooth_devices_open_settings() {
        let rows = find("bluetooth", &status());
        let devices = rows
            .iter()
            .find(|e| e.title == "Bluetooth devices")
            .unwrap();
        assert_eq!(devices.action, Action::Open("ms-settings:bluetooth"));
    }

    fn titles(query: &str, status: &Status) -> Vec<String> {
        find(query, status).into_iter().map(|e| e.title).collect()
    }

    #[test]
    fn missing_hardware_has_no_rows() {
        let all = titles("", &Status::default());
        for title in [
            "Display",
            "Screens: Extend",
            "Turn off display",
            "Next wallpaper",
            "Quit Tile",
            "System Properties",
            "Lock",
            "Hibernate",
            "Shut down",
        ] {
            assert!(all.iter().any(|t| t == title), "{title} is missing");
        }
        assert!(!all.iter().any(|t| t.starts_with("Volume") || t == "Clock"));
    }

    #[test]
    fn display_opens_its_settings_first() {
        let rows = find("display", &Status::default());
        assert_eq!(rows[0].title, "Display");
        assert_eq!(rows[0].action, Action::Open("ms-settings:display"));
        let layouts: Vec<_> = rows
            .iter()
            .filter(|e| e.title.starts_with("Screens"))
            .collect();
        assert_eq!(layouts.len(), 4);
    }

    #[test]
    fn tile_rows_run_real_dispatchers() {
        for row in entries(&Status::default()) {
            if let Action::Tile(line) = row.action {
                assert!(line.parse::<tile_core::Command>().is_ok(), "{line}");
            }
        }
        let rows = find("next wallpaper", &Status::default());
        assert_eq!(rows[0].action, Action::Tile("wallpaper next"));
    }

    #[test]
    fn every_row_is_listed_for_an_empty_query() {
        let all = titles("", &status());
        for title in [
            "Volume: 45%",
            "Network: Home",
            "Wi-Fi",
            "Bluetooth",
            "Battery: 80%",
            "Clock",
        ] {
            assert!(
                all.iter().any(|t| t == title),
                "{title} is missing from {all:?}"
            );
        }
    }

    #[test]
    fn toggles_flip_the_current_state() {
        let rows = find("bluetooth", &status());
        assert_eq!(rows[0].action, Action::SetRadio(RadioKind::Bluetooth, true));
        let rows = find("wi-fi", &status());
        assert_eq!(rows[0].action, Action::SetRadio(RadioKind::Wifi, false));
        // Both the speakers and the microphone have a mute row.
        let rows = find("mute", &status());
        assert!(rows.iter().any(|e| e.action == Action::Mute(true)));
        assert!(rows.iter().any(|e| e.action == Action::MuteMic(true)));
    }

    #[test]
    fn keywords_find_rows() {
        assert_eq!(titles("reboot", &status())[0], "Restart");
        assert_eq!(titles("logout", &status())[0], "Sign out");
    }

    #[test]
    fn volume_can_be_typed() {
        let rows = find("volume 30", &status());
        assert_eq!(rows[0].action, Action::SetVolume(30));
        assert_eq!(rows[0].title, "Set volume to 30%");
        assert!(
            find("vol 101%", &status())
                .iter()
                .all(|e| !e.title.starts_with("Set volume"))
        );
        assert!(volume_request("volume").is_none());
    }

    #[test]
    fn the_recycle_bin_can_only_be_emptied_when_it_holds_something() {
        let full = status();
        assert!(titles("recycle", &full).contains(&"Empty Recycle Bin".to_owned()));
        assert_eq!(
            find("recycle bin", &full)[0].subtitle,
            "3 items. Enter to open it in File Explorer"
        );
        let empty = Status {
            recycle_bin: Some(0),
            ..full
        };
        assert_eq!(titles("recycle", &empty), ["Recycle Bin"]);
    }

    #[test]
    fn volume_steps_stay_in_range() {
        let loud = Status {
            volume: Some(Volume {
                percent: 98,
                muted: false,
            }),
            ..Status::default()
        };
        let up = find("volume up", &loud);
        assert_eq!(up[0].action, Action::SetVolume(100));
        let quiet = Status {
            volume: Some(Volume {
                percent: 4,
                muted: true,
            }),
            ..Status::default()
        };
        assert_eq!(find("volume down", &quiet)[0].action, Action::SetVolume(0));
    }

    #[test]
    fn formats_the_time_with_seconds_and_the_date() {
        assert_eq!(format_time(22, 42, 7), "10:42:07 pm");
        assert_eq!(format_time(0, 5, 0), "12:05:00 am");
        assert_eq!(format_time(12, 0, 59), "12:00:59 pm");
        assert_eq!(format_date(2026, 10, 31, 6), "Saturday 31 October 2026");
    }

    #[test]
    fn formats_the_clock() {
        assert_eq!(
            format_clock(2026, 10, 31, 6, 22, 42),
            "10:42 pm, Saturday 31 October 2026"
        );
        assert_eq!(
            format_clock(2026, 1, 1, 4, 0, 5),
            "12:05 am, Thursday 1 January 2026"
        );
        assert_eq!(
            format_clock(2026, 1, 1, 4, 12, 0),
            "12:00 pm, Thursday 1 January 2026"
        );
    }
}
