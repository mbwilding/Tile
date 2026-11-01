//! Tile's key remapper: layers and tap/hold overloads as a pure state machine, fed physical key
//! events and returning what to send. No Windows code lives here.
//!
//! [`KEYS`] is the one table of key names, shared with keybinds: the remapper looks names up by
//! scancode ([`Key::from_name`]) and binds by virtual key ([`vk_from_name`]).

mod config;
mod engine;
mod keys;
mod parse;

pub use config::Config;
pub use engine::{Engine, KeyEvent, Out, Verdict};
pub use keys::{KEYS, Key, KeyName, vk_from_name, vk_name};

pub(crate) use config::{Action, Chord, Layer, LayerId, MAIN};
pub(crate) use keys::Mods;
