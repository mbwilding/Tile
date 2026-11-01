use std::fmt;

/// A physical key: its PC scancode (set 1), with `0xE0` in the high byte for extended keys.
///
/// Scancodes name positions, not characters, so `j` means the key where QWERTY
/// has `J`, whatever layout Windows is set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(u16);

const EXT: u16 = 0xE000;

impl Key {
    /// Escape.
    pub const ESC: Self = Self(0x01);
    /// Backspace.
    pub const BACKSPACE: Self = Self(0x0E);
    /// Enter, on the main block.
    pub const ENTER: Self = Self(0x1C);
    pub(crate) const LEFTCONTROL: Self = Self(0x1D);
    pub(crate) const LEFTSHIFT: Self = Self(0x2A);
    pub(crate) const LEFTALT: Self = Self(0x38);
    pub(crate) const RIGHTALT: Self = Self(EXT | 0x38);
    pub(crate) const LEFTMETA: Self = Self(EXT | 0x5B);

    /// The key with scancode `scan`, extended (`0xE0`-prefixed) or not.
    pub const fn new(scan: u8, extended: bool) -> Self {
        Self(scan as u16 | if extended { EXT } else { 0 })
    }

    /// The scancode, without the extended prefix.
    pub const fn scan(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// Whether the scancode has the `0xE0` prefix.
    pub const fn extended(self) -> bool {
        self.0 & EXT != 0
    }

    /// Looks up a key name, e.g. `a`, `;`, `leftshift`, `esc` or `pgup` (see [`KEYS`]).
    /// Names longer than one character ignore case; a single character must match exactly,
    /// since `A` is the shifted `a`, not a key.
    pub fn from_name(name: &str) -> Option<Self> {
        entry(name).filter(|e| e.scan != 0).map(|e| Self(e.scan))
    }

    /// The key's main name, if it has one.
    pub fn name(self) -> Option<&'static str> {
        KEYS.iter().find(|e| e.scan == self.0).map(|e| e.names[0])
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#06x}", self.0),
        }
    }
}

/// One key on the keyboard: every name it goes by, its scancode for the remapper and its
/// Windows virtual-key code for keybinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyName {
    /// The main name first, then the aliases: Windows' virtual-key names (without `VK_`) and
    /// friendlier spellings such as `pgup` or `bracketleft`.
    pub names: &'static [&'static str],
    /// The [`Key`] scancode, or `0` for a virtual key with no physical key of its own.
    pub scan: u16,
    /// The Windows virtual-key code, or `0` for a key binds can't tell apart from another.
    pub vk: u16,
}

const fn k(names: &'static [&'static str], scan: u16, vk: u16) -> KeyName {
    KeyName { names, scan, vk }
}

/// Every key name, shared by the remapper (by scancode) and keybinds (by virtual key), so
/// both accept the same names.
pub const KEYS: &[KeyName] = &[
    k(&["esc", "escape"], 0x01, 0x1B),
    k(&["1"], 0x02, 0x31),
    k(&["2"], 0x03, 0x32),
    k(&["3"], 0x04, 0x33),
    k(&["4"], 0x05, 0x34),
    k(&["5"], 0x06, 0x35),
    k(&["6"], 0x07, 0x36),
    k(&["7"], 0x08, 0x37),
    k(&["8"], 0x09, 0x38),
    k(&["9"], 0x0A, 0x39),
    k(&["0"], 0x0B, 0x30),
    k(&["minus", "-", "oem_minus"], 0x0C, 0xBD),
    k(&["equal", "=", "plus", "oem_plus"], 0x0D, 0xBB),
    k(&["backspace", "back"], 0x0E, 0x08),
    k(&["tab"], 0x0F, 0x09),
    k(&["q"], 0x10, 0x51),
    k(&["w"], 0x11, 0x57),
    k(&["e"], 0x12, 0x45),
    k(&["r"], 0x13, 0x52),
    k(&["t"], 0x14, 0x54),
    k(&["y"], 0x15, 0x59),
    k(&["u"], 0x16, 0x55),
    k(&["i"], 0x17, 0x49),
    k(&["o"], 0x18, 0x4F),
    k(&["p"], 0x19, 0x50),
    k(&["leftbrace", "[", "bracketleft", "oem_4"], 0x1A, 0xDB),
    k(&["rightbrace", "]", "bracketright", "oem_6"], 0x1B, 0xDD),
    k(&["enter", "return"], 0x1C, 0x0D),
    k(
        &["leftcontrol", "leftctrl", "lcontrol", "lctrl"],
        0x1D,
        0xA2,
    ),
    k(&["a"], 0x1E, 0x41),
    k(&["s"], 0x1F, 0x53),
    k(&["d"], 0x20, 0x44),
    k(&["f"], 0x21, 0x46),
    k(&["g"], 0x22, 0x47),
    k(&["h"], 0x23, 0x48),
    k(&["j"], 0x24, 0x4A),
    k(&["k"], 0x25, 0x4B),
    k(&["l"], 0x26, 0x4C),
    k(&["semicolon", ";", "oem_1"], 0x27, 0xBA),
    k(&["apostrophe", "'", "quote", "oem_7"], 0x28, 0xDE),
    k(&["grave", "`", "backtick", "oem_3"], 0x29, 0xC0),
    k(&["leftshift", "lshift"], 0x2A, 0xA0),
    k(&["backslash", "\\", "oem_5"], 0x2B, 0xDC),
    k(&["z"], 0x2C, 0x5A),
    k(&["x"], 0x2D, 0x58),
    k(&["c"], 0x2E, 0x43),
    k(&["v"], 0x2F, 0x56),
    k(&["b"], 0x30, 0x42),
    k(&["n"], 0x31, 0x4E),
    k(&["m"], 0x32, 0x4D),
    k(&["comma", ",", "oem_comma"], 0x33, 0xBC),
    k(&["dot", ".", "period", "oem_period"], 0x34, 0xBE),
    k(&["slash", "/", "oem_2"], 0x35, 0xBF),
    k(&["rightshift", "rshift"], 0x36, 0xA1),
    k(&["kpasterisk", "multiply"], 0x37, 0x6A),
    k(&["leftalt", "lalt", "lmenu"], 0x38, 0xA4),
    k(&["space"], 0x39, 0x20),
    k(&["capslock", "capital"], 0x3A, 0x14),
    k(&["f1"], 0x3B, 0x70),
    k(&["f2"], 0x3C, 0x71),
    k(&["f3"], 0x3D, 0x72),
    k(&["f4"], 0x3E, 0x73),
    k(&["f5"], 0x3F, 0x74),
    k(&["f6"], 0x40, 0x75),
    k(&["f7"], 0x41, 0x76),
    k(&["f8"], 0x42, 0x77),
    k(&["f9"], 0x43, 0x78),
    k(&["f10"], 0x44, 0x79),
    k(&["scrolllock", "scroll"], 0x46, 0x91),
    k(&["kp7", "numpad7"], 0x47, 0x67),
    k(&["kp8", "numpad8"], 0x48, 0x68),
    k(&["kp9", "numpad9"], 0x49, 0x69),
    k(&["kpminus", "subtract"], 0x4A, 0x6D),
    k(&["kp4", "numpad4"], 0x4B, 0x64),
    k(&["kp5", "numpad5"], 0x4C, 0x65),
    k(&["kp6", "numpad6"], 0x4D, 0x66),
    k(&["kpplus", "add"], 0x4E, 0x6B),
    k(&["kp1", "numpad1"], 0x4F, 0x61),
    k(&["kp2", "numpad2"], 0x50, 0x62),
    k(&["kp3", "numpad3"], 0x51, 0x63),
    k(&["kp0", "numpad0"], 0x52, 0x60),
    k(&["kpdot", "decimal"], 0x53, 0x6E),
    k(&["102nd", "oem_102"], 0x56, 0xE2),
    k(&["f11"], 0x57, 0x7A),
    k(&["f12"], 0x58, 0x7B),
    k(&["f13"], 0x64, 0x7C),
    k(&["f14"], 0x65, 0x7D),
    k(&["f15"], 0x66, 0x7E),
    k(&["f16"], 0x67, 0x7F),
    k(&["f17"], 0x68, 0x80),
    k(&["f18"], 0x69, 0x81),
    k(&["f19"], 0x6A, 0x82),
    k(&["f20"], 0x6B, 0x83),
    k(&["f21"], 0x6C, 0x84),
    k(&["f22"], 0x6D, 0x85),
    k(&["f23"], 0x6E, 0x86),
    k(&["f24"], 0x76, 0x87),
    k(&["previoussong", "media_prev_track"], EXT | 0x10, 0xB1),
    k(&["nextsong", "media_next_track"], EXT | 0x19, 0xB0),
    // Windows gives keypad Enter the same virtual key as Enter.
    k(&["kpenter"], EXT | 0x1C, 0),
    k(
        &["rightcontrol", "rightctrl", "rcontrol", "rctrl"],
        EXT | 0x1D,
        0xA3,
    ),
    k(&["mute", "volume_mute"], EXT | 0x20, 0xAD),
    k(&["calc", "launch_app2"], EXT | 0x21, 0xB7),
    k(&["playpause", "media_play_pause"], EXT | 0x22, 0xB3),
    k(&["stopcd", "media_stop"], EXT | 0x24, 0xB2),
    k(&["volumedown", "volume_down"], EXT | 0x2E, 0xAE),
    k(&["volumeup", "volume_up"], EXT | 0x30, 0xAF),
    k(&["homepage", "browser_home"], EXT | 0x32, 0xAC),
    k(&["kpslash", "divide"], EXT | 0x35, 0x6F),
    k(
        &["sysrq", "print", "printscreen", "snapshot"],
        EXT | 0x37,
        0x2C,
    ),
    k(&["rightalt", "ralt", "rmenu"], EXT | 0x38, 0xA5),
    k(&["numlock"], EXT | 0x45, 0x90),
    k(&["home"], EXT | 0x47, 0x24),
    k(&["up"], EXT | 0x48, 0x26),
    k(&["pageup", "pgup", "prior"], EXT | 0x49, 0x21),
    k(&["left"], EXT | 0x4B, 0x25),
    k(&["right"], EXT | 0x4D, 0x27),
    k(&["end"], EXT | 0x4F, 0x23),
    k(&["down"], EXT | 0x50, 0x28),
    k(&["pagedown", "pgdn", "next"], EXT | 0x51, 0x22),
    k(&["insert", "ins"], EXT | 0x52, 0x2D),
    k(&["delete", "del"], EXT | 0x53, 0x2E),
    k(&["leftmeta", "lwin", "win"], EXT | 0x5B, 0x5B),
    k(&["rightmeta", "rwin"], EXT | 0x5C, 0x5C),
    k(&["compose", "menu", "apps"], EXT | 0x5D, 0x5D),
    k(&["sleep"], EXT | 0x5F, 0x5F),
    k(&["search", "browser_search"], EXT | 0x65, 0xAA),
    k(&["bookmarks", "browser_favorites"], EXT | 0x66, 0xAB),
    k(&["refresh", "browser_refresh"], EXT | 0x67, 0xA8),
    k(&["stop", "browser_stop"], EXT | 0x68, 0xA9),
    k(&["forward", "browser_forward"], EXT | 0x69, 0xA7),
    k(&["browser_back"], EXT | 0x6A, 0xA6),
    k(&["computer", "launch_app1"], EXT | 0x6B, 0xB6),
    k(&["mail", "launch_mail"], EXT | 0x6C, 0xB4),
    k(&["media", "launch_media_select"], EXT | 0x6D, 0xB5),
    // Virtual keys only: generic modifiers and keys with no scancode of their own.
    k(&["shift"], 0, 0x10),
    k(&["ctrl", "control"], 0, 0x11),
    k(&["alt"], 0, 0x12),
    k(&["clear"], 0, 0x0C),
    k(&["pause"], 0, 0x13),
    k(&["select"], 0, 0x29),
    k(&["execute"], 0, 0x2B),
    k(&["help"], 0, 0x2F),
    k(&["separator"], 0, 0x6C),
    k(&["oem_8"], 0, 0xDF),
    k(&["attn"], 0, 0xF6),
    k(&["crsel"], 0, 0xF7),
    k(&["exsel"], 0, 0xF8),
    k(&["play"], 0, 0xFA),
    k(&["zoom"], 0, 0xFB),
    k(&["pa1"], 0, 0xFD),
    k(&["oem_clear"], 0, 0xFE),
];

fn entry(name: &str) -> Option<&'static KeyName> {
    let matches = |n: &&str| {
        if name.chars().nth(1).is_some() {
            n.eq_ignore_ascii_case(name)
        } else {
            *n == name
        }
    };
    KEYS.iter().find(|e| e.names.iter().any(matches))
}

/// The Windows virtual-key code a keybind's key names: any name in [`KEYS`] in any case,
/// optionally with Windows' `VK_` prefix, or a raw code such as `0x29`.
pub fn vk_from_name(name: &str) -> Result<u16, String> {
    let lower = name.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        return match u16::from_str_radix(hex, 16) {
            Ok(vk @ 1..=0xFE) => Ok(vk),
            _ => Err(format!("invalid virtual-key code `{name}`")),
        };
    }
    let bare = lower.strip_prefix("vk_").unwrap_or(&lower);
    match entry(bare) {
        Some(e) if e.vk != 0 => Ok(e.vk),
        Some(_) => Err(format!("`{name}` can't be told apart from another key")),
        None => Err(format!("unknown key `{name}`")),
    }
}

/// The first name of the key with virtual-key code `vk`, if it has one.
pub fn vk_name(vk: u16) -> Option<&'static str> {
    KEYS.iter().find(|e| e.vk == vk).map(|e| e.names[0])
}

/// Characters accepted as shorthand for a shifted key, e.g. `!` for `S-1` (US positions).
pub(crate) fn shifted(c: char) -> Option<Key> {
    let base = match c {
        '!' => "1",
        '@' => "2",
        '#' => "3",
        '$' => "4",
        '%' => "5",
        '^' => "6",
        '&' => "7",
        '*' => "8",
        '(' => "9",
        ')' => "0",
        '_' => "minus",
        '+' => "equal",
        '{' => "leftbrace",
        '}' => "rightbrace",
        '|' => "backslash",
        ':' => "semicolon",
        '"' => "apostrophe",
        '<' => "comma",
        '>' => "dot",
        '?' => "slash",
        '~' => "grave",
        'A'..='Z' => return Key::from_name(&c.to_ascii_lowercase().to_string()),
        _ => return None,
    };
    Key::from_name(base)
}

/// Modifiers held by a layer or added by a key sequence such as `C-left`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct Mods(u8);

impl Mods {
    pub const NONE: Self = Self(0);
    pub const CONTROL: Self = Self(1);
    pub const SHIFT: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const META: Self = Self(8);
    pub const ALTGR: Self = Self(16);

    /// Each modifier with the key that produces it, in the order they are pressed.
    pub const KEYS: [(Self, Key); 5] = [
        (Self::CONTROL, Key::LEFTCONTROL),
        (Self::SHIFT, Key::LEFTSHIFT),
        (Self::ALT, Key::LEFTALT),
        (Self::META, Key::LEFTMETA),
        (Self::ALTGR, Key::RIGHTALT),
    ];

    /// The modifier letters: `C`, `S`, `A`, `M` and `G`.
    pub fn from_letter(c: char) -> Option<Self> {
        match c {
            'C' => Some(Self::CONTROL),
            'S' => Some(Self::SHIFT),
            'A' => Some(Self::ALT),
            'M' => Some(Self::META),
            'G' => Some(Self::ALTGR),
            _ => None,
        }
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn names_round_trip_to_canonical() {
        assert_eq!(Key::from_name(";"), Key::from_name("semicolon"));
        assert_eq!(Key::from_name("escape").unwrap().name(), Some("esc"));
        assert_eq!(Key::from_name("leftshift"), Some(Key::LEFTSHIFT));
        // Uppercase letters are shifted keys (`A` is `S-a`), not names.
        assert_eq!(Key::from_name("A"), None);
        let left = Key::from_name("left").unwrap();
        assert_eq!((left.scan(), left.extended()), (0x4B, true));
        assert_eq!(Key::from_name("nope"), None);
    }

    #[test]
    fn bind_spellings_name_remap_keys() {
        assert_eq!(Key::from_name("bracketleft"), Key::from_name("leftbrace"));
        assert_eq!(Key::from_name("period"), Key::from_name("dot"));
        assert_eq!(Key::from_name("quote"), Key::from_name("'"));
        assert_eq!(Key::from_name("PgUp"), Key::from_name("pageup"));
        assert_eq!(Key::from_name("CapsLock"), Key::from_name("capslock"));
        assert_eq!(Key::from_name("ctrl"), None);
    }

    #[test]
    fn remap_spellings_name_virtual_keys() {
        assert_eq!(vk_from_name("leftbrace"), vk_from_name("bracketleft"));
        assert_eq!(vk_from_name("["), Ok(0xDB));
        assert_eq!(vk_from_name("dot"), vk_from_name("OEM_PERIOD"));
        assert_eq!(vk_from_name("Q"), Ok(0x51));
        assert_eq!(vk_from_name("VK_RETURN"), Ok(0x0D));
        assert_eq!(vk_from_name("0x29"), Ok(0x29));
        assert_eq!(vk_from_name("print"), Ok(0x2C));
        assert!(vk_from_name("kpenter").is_err());
        assert!(vk_from_name("0x").is_err());
        assert!(vk_from_name("hyper").is_err());
        assert_eq!(vk_name(0xDB), Some("leftbrace"));
    }

    #[test]
    fn every_name_and_code_is_unique() {
        let mut names = HashSet::new();
        let (mut scans, mut vks) = (HashSet::new(), HashSet::new());
        for e in KEYS {
            for name in e.names {
                assert!(names.insert(name.to_ascii_lowercase()), "{name}");
            }
            assert!(e.scan == 0 || scans.insert(e.scan), "{:?}", e.names);
            assert!(e.vk == 0 || vks.insert(e.vk), "{:?}", e.names);
        }
    }

    #[test]
    fn shifted_characters() {
        assert_eq!(shifted('?'), Key::from_name("/"));
        assert_eq!(shifted('Q'), Key::from_name("q"));
        assert_eq!(shifted('q'), None);
    }
}
