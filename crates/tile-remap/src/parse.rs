//! Remap layer names and actions, compiled into a [`Config`].

use std::collections::BTreeMap;

use crate::{Action, Chord, Config, Key, Layer, LayerId, Mods, keys::shifted};

/// `C-A` (from `name:C-A`) as modifiers.
fn parse_mods(text: &str) -> Result<Mods, String> {
    text.split('-').try_fold(Mods::NONE, |mods, letter| {
        let mut chars = letter.chars();
        match (chars.next().and_then(Mods::from_letter), chars.next()) {
            (Some(m), None) => Ok(mods | m),
            _ => Err(format!("unknown modifier `{letter}` (use C, S, A, M or G)")),
        }
    })
}

/// Creates the layer `name` or `name:MODS` names, or finds it if it exists (including the
/// built-in `main`, `control`, `shift`, `alt`, `meta` and `altgr`).
fn declare_layer(config: &mut Config, text: &str) -> Result<LayerId, String> {
    let (name, mods) = match text.split_once(':') {
        Some((name, mods)) => (name.trim(), Some(parse_mods(mods.trim())?)),
        None => (text.trim(), None),
    };
    if name.is_empty() {
        return Err(format!("invalid layer name `{name}`"));
    }
    if let Some(id) = config.layer(name) {
        if let Some(mods) = mods {
            config.layers[id].mods = mods;
        }
        return Ok(id);
    }
    config
        .layers
        .push(Layer::new(name, mods.unwrap_or_default()));
    Ok(config.layers.len() - 1)
}

/// Creates the composite layer `a+b` names.
fn declare_composite(config: &mut Config, text: &str) -> Result<LayerId, String> {
    let parts = text
        .split('+')
        .map(|part| {
            let part = part.trim();
            match config.layer(part) {
                Some(id) if !config.layers[id].is_composite() => Ok(id),
                _ => Err(format!("unknown layer `{part}`")),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(id) = config.layer(text) {
        return Ok(id);
    }
    let mut layer = Layer::new(text, Mods::NONE);
    layer.constituents = parts;
    config.layers.push(layer);
    Ok(config.layers.len() - 1)
}

fn layer_ref(config: &Config, name: &str) -> Result<LayerId, String> {
    match config.layer(name) {
        Some(id) if config.layers[id].is_composite() => {
            Err(format!("composite layer `{name}` can't be activated"))
        }
        Some(id) => Ok(id),
        None => Err(format!("unknown layer `{name}`")),
    }
}

/// A key with optional modifier prefixes, e.g. `left`, `C-left` or `A-S-x`, or a shifted
/// character such as `!` or `A`.
fn parse_chord(text: &str) -> Result<Chord, String> {
    let mut mods = Mods::NONE;
    let mut rest = text;
    while let [letter, b'-', _, ..] = rest.as_bytes()
        && let Some(m) = Mods::from_letter(*letter as char)
    {
        mods |= m;
        rest = &rest[2..];
    }
    if let Some(key) = Key::from_name(rest) {
        return Ok(Chord { mods, key });
    }
    let mut chars = rest.chars();
    match (chars.next().and_then(shifted), chars.next()) {
        (Some(key), None) => Ok(Chord {
            mods: mods | Mods::SHIFT,
            key,
        }),
        _ => Err(format!("unknown key `{rest}`")),
    }
}

fn parse_action(config: &Config, text: &str) -> Result<Action, String> {
    let text = text.trim();
    if text == "noop" {
        return Ok(Action::Noop);
    }
    let call = text
        .find('(')
        .filter(|&open| open > 0 && text.ends_with(')'));
    let Some(open) = call else {
        return parse_chord(text).map(Action::Chord);
    };
    let name = text[..open].trim();
    let args: Vec<&str> = text[open + 1..text.len() - 1]
        .split(',')
        .map(str::trim)
        .collect();
    let arity = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!(
                "`{name}` takes {n} argument(s), got {}",
                args.len()
            ))
        }
    };
    match name {
        "layer" => {
            arity(1)?;
            Ok(Action::Layer(layer_ref(config, args[0])?))
        }
        "overload" => {
            arity(2)?;
            Ok(Action::Overload {
                layer: layer_ref(config, args[0])?,
                tap: parse_chord(args[1])?,
            })
        }
        "overloadt" => {
            arity(3)?;
            let timeout_ms = args[2]
                .parse()
                .map_err(|_| format!("invalid timeout `{}`", args[2]))?;
            Ok(Action::OverloadTimeout {
                layer: layer_ref(config, args[0])?,
                tap: parse_chord(args[1])?,
                timeout_ms,
            })
        }
        _ => Err(format!("`{name}` isn't supported yet")),
    }
}

impl Config {
    /// Compiles remap layers: each entry is named `main`, `nav`, `nav:C` (with modifiers) or
    /// `nav+shift` (a composite) and maps key names to actions. Lists everything wrong on failure.
    pub fn compile(
        layers: &BTreeMap<String, BTreeMap<String, String>>,
        modifier_guard: bool,
    ) -> Result<Self, Vec<String>> {
        let mut config = Self {
            modifier_guard,
            ..Self::default()
        };
        let mut errors = Vec::new();

        // Declare every layer first, so actions can refer to any of them, and plain layers
        // before composites, which are made of them.
        let mut declared = Vec::new();
        for composite in [false, true] {
            for name in layers.keys() {
                if name.contains('+') != composite {
                    continue;
                }
                let id = if composite {
                    declare_composite(&mut config, name)
                } else {
                    declare_layer(&mut config, name)
                };
                match id {
                    Ok(id) => declared.push((name, id)),
                    Err(e) => errors.push(format!("layer `{name}`: {e}")),
                }
            }
        }

        for (name, id) in declared {
            for (key, action) in &layers[name] {
                let bound = Key::from_name(key.trim())
                    .ok_or_else(|| format!("unknown key `{key}`"))
                    .and_then(|k| Ok((k, parse_action(&config, action)?)));
                match bound {
                    Ok((k, action)) => {
                        config.layers[id].binds.insert(k, action);
                    }
                    Err(e) => errors.push(format!("layer `{name}`: `{key} = \"{action}\"`: {e}")),
                }
            }
        }
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors)
        }
    }
}

/// Compiles layers written as `[name]` headers, each followed by `key = action` lines.
#[cfg(test)]
pub(crate) fn sections(text: &str) -> Result<Config, Vec<String>> {
    let mut layers: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_owned();
            layers.entry(section.clone()).or_default();
        } else if let Some((key, action)) = line.split_once(" = ") {
            layers
                .entry(section.clone())
                .or_default()
                .insert(key.trim().to_owned(), action.trim().to_owned());
        }
    }
    Config::compile(&layers, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAIN;

    fn key(name: &str) -> Key {
        Key::from_name(name).unwrap()
    }

    #[test]
    fn layers_and_actions() {
        let config = sections(
            "
[main]
capslock = overload(nav, esc)
a = overloadt(control, a, 200)
= = C-S-x
x = !
[nav:C-A]
h = left
[nav+shift]
h = noop
",
        )
        .unwrap();
        let main = &config.layers[MAIN].binds;
        let nav = config.layer("nav").unwrap();
        let control = config.layer("control").unwrap();
        assert_eq!(
            main[&key("capslock")],
            Action::Overload {
                layer: nav,
                tap: Chord::key(Key::ESC)
            }
        );
        assert_eq!(
            main[&key("a")],
            Action::OverloadTimeout {
                layer: control,
                tap: Chord::key(key("a")),
                timeout_ms: 200
            }
        );
        assert_eq!(
            main[&key("=")],
            Action::Chord(Chord {
                mods: Mods::CONTROL | Mods::SHIFT,
                key: key("x")
            })
        );
        assert_eq!(
            main[&key("x")],
            Action::Chord(Chord {
                mods: Mods::SHIFT,
                key: key("1")
            })
        );
        assert_eq!(config.layers[nav].mods, Mods::CONTROL | Mods::ALT);
        let composite = &config.layers[config.layer("nav+shift").unwrap()];
        assert_eq!(
            composite.constituents,
            vec![nav, config.layer("shift").unwrap()]
        );
        assert_eq!(composite.binds[&key("h")], Action::Noop);
    }

    #[test]
    fn reports_every_problem() {
        let errors = sections(
            "
[main]
nope = a
b = nope
c = layer(missing)
d = oneshot(shift)
e = f
[x:Q]
g = h
[y+missing]
g = h
",
        )
        .unwrap_err();
        assert_eq!(errors.len(), 6, "{errors:#?}");
        assert!(errors.iter().any(|e| e.contains("unknown key `nope`")));
    }

    #[test]
    fn layer_modifiers_and_arity() {
        assert!(parse_mods("C-X").is_err());
        assert_eq!(parse_mods("M-G"), Ok(Mods::META | Mods::ALTGR));
        let errors =
            sections("[main]\na = overload(shift)\nb = overloadt(shift, b, soon)").unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("takes 2 argument(s), got 1"))
        );
        assert!(errors.iter().any(|e| e.contains("invalid timeout `soon`")));
    }

    #[test]
    fn composites_cannot_be_activated() {
        let errors = sections("[nav]\n[nav+shift]\n[main]\na = layer(nav+shift)").unwrap_err();
        assert!(errors[0].contains("can't be activated"), "{errors:?}");
    }

    #[test]
    fn modifier_guard() {
        let layers = BTreeMap::new();
        assert!(Config::compile(&layers, true).unwrap().modifier_guard);
        assert!(!Config::compile(&layers, false).unwrap().modifier_guard);
    }
}
