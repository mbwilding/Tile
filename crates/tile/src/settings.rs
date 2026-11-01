//! The tray's switches (Tile, Remap, Shell, logging), kept in the config's `tray` section so
//! they survive a restart.

use std::path::Path;

pub use tile_config::Tray as Settings;
use tile_config::{Loaded, write_tray};

/// The switches in the config file at `path`, or the defaults if it doesn't load.
pub fn load(path: &Path) -> Settings {
    Loaded::load_or_default(path).map_or_else(|_| Settings::default(), |l| l.config.tray)
}

/// Applies `change` to `settings` and saves the switches it changed to the config file at
/// `path`. Only those are written, so the file keeps its other switches even when it doesn't
/// load and `settings` are the defaults.
pub fn update(path: &Path, settings: &mut Settings, change: impl FnOnce(&mut Settings)) {
    let before = settings.clone();
    change(settings);
    if let Err(e) = write_tray(path, &before, settings) {
        tracing::warn!("failed to save settings: {e}");
    }
}
