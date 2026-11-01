use std::{fmt, str::FromStr};

/// Modifier keys required by a keybind. A bind fires only when exactly these are held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Either Win key.
    pub super_key: bool,
    /// Either Shift.
    pub shift: bool,
    /// Either Ctrl.
    pub ctrl: bool,
    /// Either Alt.
    pub alt: bool,
}

/// A parsed key combination such as `super+shift+q`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Keybind {
    /// The modifiers held with the key.
    pub modifiers: Modifiers,
    /// The key's Windows virtual-key code, resolved from its name when the bind is parsed.
    pub vk: u16,
}

/// The `L` key, which Win+L locks the workstation with.
const VK_L: u16 = 0x4C;

impl FromStr for Keybind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::default();
        let mut parts = s.split('+').map(str::trim);
        let key = parts.next_back().unwrap_or_default();
        if key.is_empty() {
            return Err("missing key".into());
        }
        for m in parts {
            match m.to_ascii_lowercase().as_str() {
                "super" | "win" | "mod4" | "meta" => modifiers.super_key = true,
                "shift" => modifiers.shift = true,
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" | "mod1" => modifiers.alt = true,
                other => return Err(format!("unknown modifier `{other}`")),
            }
        }
        Ok(Self {
            modifiers,
            vk: tile_remap::vk_from_name(key)?,
        })
    }
}

impl Keybind {
    /// Win+L locks the workstation below any keyboard hook; binds to it only work with
    /// `general.override_win_l`.
    pub fn is_win_l(&self) -> bool {
        let m = self.modifiers;
        m.super_key && !m.shift && !m.ctrl && !m.alt && self.vk == VK_L
    }
}

impl fmt::Display for Keybind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, name) in [
            (m.super_key, "super"),
            (m.ctrl, "ctrl"),
            (m.alt, "alt"),
            (m.shift, "shift"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match tile_remap::vk_name(self.vk) {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#04x}", self.vk),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk(keys: &str) -> u16 {
        keys.parse::<Keybind>().unwrap().vk
    }

    #[test]
    fn parses_modifiers_and_aliases() {
        let k: Keybind = "SUPER+Shift+enter".parse().unwrap();
        assert!(k.modifiers.super_key && k.modifiers.shift && !k.modifiers.ctrl);
        assert_eq!(k.vk, 0x0D);
        assert_eq!(vk("alt+comma"), 0xBC);
        assert_eq!(vk("super+1"), 0x31);
        assert_eq!(vk("super+OEM_102"), 0xE2);
        assert_eq!(vk("super+pgup"), vk("super+PRIOR"));
        assert_eq!(vk("super+leftbrace"), vk("super+bracketleft"));
        assert_eq!(
            "super+ctrl+[".parse::<Keybind>().unwrap().to_string(),
            "super+ctrl+leftbrace"
        );
    }

    #[test]
    fn rejects_unknown_and_flags_win_l() {
        assert!("super+l".parse::<Keybind>().unwrap().is_win_l());
        assert!(!"super+shift+l".parse::<Keybind>().unwrap().is_win_l());
        assert!("hyper+a".parse::<Keybind>().is_err());
        assert!("super+".parse::<Keybind>().is_err());
        let err = "super+bracket".parse::<Keybind>().unwrap_err();
        assert_eq!(err, "unknown key `bracket`");
    }
}
