//! The desktop, for when no Explorer is there to paint one: a window at the bottom of the stack
//! on each monitor, showing the wallpaper's colour and image. Tile never manages them, and they
//! are gone when the [`Backdrop`] is dropped.

use std::path::PathBuf;

use tile_config::{FillMode, Wallpaper};
use tile_shell::backdrop::{Fill, Look};

use crate::monitors::monitors;

/// The desktop windows, kept to the monitors.
pub struct Backdrop(tile_shell::backdrop::Backdrop);

fn areas() -> Vec<tile_shell::backdrop::Area> {
    monitors()
        .iter()
        .map(|m| [m.rect.x, m.rect.y, m.rect.w, m.rect.h])
        .collect()
}

/// What `wallpaper` asks for, showing `image` as the picture.
fn look(wallpaper: &Wallpaper, image: Option<PathBuf>) -> Look {
    let c = wallpaper.color;
    Look {
        color: [c.r, c.g, c.b],
        image,
        fill: match wallpaper.fill_mode {
            FillMode::Crop => Fill::Crop,
            FillMode::Fit => Fill::Fit,
            FillMode::Stretch => Fill::Stretch,
            FillMode::Center => Fill::Center,
            FillMode::Tile => Fill::Tile,
        },
    }
}

impl Backdrop {
    /// Covers every monitor with the wallpaper's colour and `image`. `None` if the windows
    /// couldn't be made.
    pub fn start(wallpaper: &Wallpaper, image: Option<PathBuf>) -> Option<Self> {
        tile_shell::backdrop::Backdrop::start(areas(), look(wallpaper, image))
            .inspect_err(|e| tracing::warn!("couldn't paint the desktop: {e}"))
            .ok()
            .map(Self)
    }

    /// Shows the wallpaper's colour and fill with `image` as the picture.
    pub fn show(&self, wallpaper: &Wallpaper, image: Option<PathBuf>) {
        self.0.set_look(look(wallpaper, image));
    }

    /// Fits the windows to the monitors, after they changed.
    pub fn refresh(&self) {
        self.0.set_areas(areas());
    }
}
