use std::collections::HashMap;

use crate::{Key, Mods};

/// Index of a layer in [`Config::layers`].
pub(crate) type LayerId = usize;

/// The `[main]` layer, always present.
pub(crate) const MAIN: LayerId = 0;

/// A key plus the modifiers to hold while it's pressed, e.g. `C-left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chord {
    pub mods: Mods,
    pub key: Key,
}

impl Chord {
    pub const fn key(key: Key) -> Self {
        Self {
            mods: Mods::NONE,
            key,
        }
    }
}

/// What a key does when pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Nothing.
    Noop,
    /// Sends a key (with modifiers) for as long as the key is held.
    Chord(Chord),
    /// `layer(name)`: the layer is active while the key is held.
    Layer(LayerId),
    /// `overload(layer, tap)`: the layer while held, `tap` if released with no other key
    /// pressed in between.
    Overload { layer: LayerId, tap: Chord },
    /// `overloadt(layer, tap, timeout)`: the layer once held for `timeout_ms`, `tap` if
    /// released before then. Keys pressed meanwhile wait for the decision.
    OverloadTimeout {
        layer: LayerId,
        tap: Chord,
        timeout_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layer {
    pub name: String,
    /// Applied to keys this layer doesn't bind while it's active (`[name:C]`).
    pub mods: Mods,
    pub binds: HashMap<Key, Action>,
    /// For a composite layer (`[a+b]`), the layers that must all be active.
    pub constituents: Vec<LayerId>,
}

impl Layer {
    pub fn new(name: impl Into<String>, mods: Mods) -> Self {
        Self {
            name: name.into(),
            mods,
            binds: HashMap::new(),
            constituents: Vec::new(),
        }
    }

    pub fn is_composite(&self) -> bool {
        !self.constituents.is_empty()
    }
}

/// Compiled remap layers, ready for an [`Engine`](crate::Engine). Built by
/// [`Config::compile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `main` first, then the built-in modifier layers, then the config's own.
    pub(crate) layers: Vec<Layer>,
    /// Tap a dummy key before releasing an Alt or Win that nothing else was pressed with, so
    /// it doesn't open a menu or Start (the modifier guard).
    pub(crate) modifier_guard: bool,
}

impl Default for Config {
    /// No bindings: every key does what it always did.
    fn default() -> Self {
        let layers = [
            ("main", Mods::NONE),
            ("control", Mods::CONTROL),
            ("shift", Mods::SHIFT),
            ("alt", Mods::ALT),
            ("meta", Mods::META),
            ("altgr", Mods::ALTGR),
        ]
        .into_iter()
        .map(|(name, mods)| Layer::new(name, mods))
        .collect();
        Self {
            layers,
            modifier_guard: true,
        }
    }
}

impl Config {
    pub(crate) fn layer(&self, name: &str) -> Option<LayerId> {
        self.layers.iter().position(|l| l.name == name)
    }

    /// No layer binds a key, so the remapper changes nothing.
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.binds.is_empty())
    }
}
