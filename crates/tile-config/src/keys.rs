use std::{fmt, str::FromStr};

/// Modifier keys required by a keybind. A bind fires only when exactly these are held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub super_key: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// A parsed key combination such as `super+shift+q`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Keybind {
    pub modifiers: Modifiers,
    /// A Windows virtual-key name understood by `win-hotkeys` (e.g. `RETURN`, `Q`, `OEM_COMMA`).
    pub key: String,
}

/// Maps friendly key names onto Windows virtual-key names.
fn key_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let mapped = match lower.as_str() {
        "enter" | "return" => "RETURN",
        "esc" | "escape" => "ESCAPE",
        "backspace" => "BACK",
        "del" | "delete" => "DELETE",
        "ins" | "insert" => "INSERT",
        "pageup" | "pgup" => "PRIOR",
        "pagedown" | "pgdn" => "NEXT",
        "print" | "printscreen" => "SNAPSHOT",
        "comma" => "OEM_COMMA",
        "period" | "dot" => "OEM_PERIOD",
        "minus" => "OEM_MINUS",
        "equal" | "plus" => "OEM_PLUS",
        "semicolon" => "OEM_1",
        "slash" => "OEM_2",
        "grave" | "backtick" => "OEM_3",
        "bracketleft" => "OEM_4",
        "backslash" => "OEM_5",
        "bracketright" => "OEM_6",
        "apostrophe" | "quote" => "OEM_7",
        _ => return name.to_ascii_uppercase(),
    };
    mapped.to_string()
}

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
            key: key_name(key),
        })
    }
}

impl Keybind {
    /// Win+L locks the workstation below any keyboard hook; binds to it only work with
    /// `general.override_win_l`.
    pub fn is_win_l(&self) -> bool {
        let m = self.modifiers;
        m.super_key && !m.shift && !m.ctrl && !m.alt && self.key == "L"
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
        f.write_str(&self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_aliases() {
        let k: Keybind = "SUPER+Shift+enter".parse().unwrap();
        assert!(k.modifiers.super_key && k.modifiers.shift && !k.modifiers.ctrl);
        assert_eq!(k.key, "RETURN");
        assert_eq!("alt+comma".parse::<Keybind>().unwrap().key, "OEM_COMMA");
        assert_eq!("super+1".parse::<Keybind>().unwrap().key, "1");
    }

    #[test]
    fn rejects_unknown_and_flags_win_l() {
        assert!("super+l".parse::<Keybind>().unwrap().is_win_l());
        assert!(!"super+shift+l".parse::<Keybind>().unwrap().is_win_l());
        assert!("hyper+a".parse::<Keybind>().is_err());
        assert!("super+".parse::<Keybind>().is_err());
    }
}
