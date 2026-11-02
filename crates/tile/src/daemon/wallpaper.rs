//! The wallpaper: which image the desktop shows, and changing it on a timer.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use tile_config::{CycleOrder, Wallpaper};
use tile_core::WallpaperAction;
use tokio::time::Instant;

use super::Daemon;

/// What the wallpaper's images are called, by extension.
const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jfif", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif", "avif",
];

/// How many earlier images `wallpaper prev` can step back through.
const HISTORY: usize = 50;

/// The image on show, the ones already shown in this round of a random cycle and the ones
/// before it, newest last.
#[derive(Default)]
pub(super) struct Shown {
    current: Option<PathBuf>,
    played: HashSet<PathBuf>,
    history: Vec<PathBuf>,
    /// The timed cycle is stopped until `wallpaper pause` again or a reload.
    paused: bool,
}

/// The images in `directory` (and the folders under it, if `recursive`), by path. Symlinked
/// folders aren't followed, so a loop can't trap it.
fn images(directory: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![directory.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if recursive {
                    folders.push(path);
                }
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// A number that differs from run to run, for picking at random: nothing here needs more than
/// that.
fn random(below: usize) -> usize {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    // One round of xorshift, so successive nanosecond readings don't pick neighbours.
    let mut x = nanos | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x % below.max(1) as u64) as usize
}

/// The image after `shown.current` among `candidates` (sorted): the next by path, or a random
/// one not yet played in this round. `None` without candidates.
fn pick(order: CycleOrder, candidates: &[PathBuf], shown: &mut Shown) -> Option<PathBuf> {
    let first = candidates.first()?;
    let picked = match order {
        CycleOrder::Alphabetical => shown
            .current
            .as_ref()
            .and_then(|current| candidates.iter().find(|c| *c > current))
            .unwrap_or(first),
        CycleOrder::Random => {
            let mut fresh: Vec<_> = candidates
                .iter()
                .filter(|c| !shown.played.contains(*c) && Some(*c) != shown.current.as_ref())
                .collect();
            if fresh.is_empty() {
                shown.played.clear();
                fresh = candidates
                    .iter()
                    .filter(|c| Some(*c) != shown.current.as_ref())
                    .collect();
            }
            // Only the one image there is, if it is the one on show.
            fresh.get(random(fresh.len())).copied().unwrap_or(first)
        }
    };
    shown.played.insert(picked.clone());
    Some(picked.clone())
}

impl Daemon {
    fn wallpaper_config(&self) -> &Wallpaper {
        &self.loaded.config.wallpaper
    }

    /// The images the wallpaper picks from.
    fn wallpaper_images(&self) -> Vec<PathBuf> {
        let w = self.wallpaper_config();
        w.directory
            .as_deref()
            .map(|d| images(Path::new(d), w.recursive))
            .unwrap_or_default()
    }

    /// Chooses the image to start with: `path`, else one from `directory`.
    fn first_wallpaper(&mut self) {
        let w = self.wallpaper_config();
        let order = w.cycle.order;
        let path = w.path.as_deref().filter(|p| !p.trim().is_empty());
        let mut shown = Shown::default();
        if let Some(path) = path {
            shown.played.insert(PathBuf::from(path));
            shown.current = Some(PathBuf::from(path));
        } else {
            shown.current = pick(order, &self.wallpaper_images(), &mut shown);
        }
        self.shown = shown;
    }

    /// Puts the desktop up, with the wallpaper to start with.
    pub(super) fn start_backdrop(&mut self) {
        self.first_wallpaper();
        self.backdrop =
            tile_platform::Backdrop::start(self.wallpaper_config(), self.shown.current.clone());
        self.schedule_wallpaper();
    }

    /// Takes the desktop down, with the timer that changes it.
    pub(super) fn stop_backdrop(&mut self) {
        self.backdrop = None;
        self.wallpaper_at = None;
    }

    /// The wallpaper settings changed: starts over from the image to start with.
    pub(super) fn reload_wallpaper(&mut self) {
        if self.backdrop.is_none() {
            return;
        }
        self.first_wallpaper();
        self.show_wallpaper();
    }

    /// Changes the image, from the timer or a `wallpaper` command. Does nothing unless Tile is
    /// painting the desktop.
    pub(super) fn change_wallpaper(&mut self, action: WallpaperAction) {
        if self.backdrop.is_none() {
            tracing::debug!("wallpaper: Tile isn't painting the desktop");
            return;
        }
        let configured = self.wallpaper_config().cycle.order;
        let mut shown = std::mem::take(&mut self.shown);
        let (next, back) = match &action {
            WallpaperAction::Next => (
                pick(configured, &self.wallpaper_images(), &mut shown),
                false,
            ),
            WallpaperAction::Random => (
                pick(CycleOrder::Random, &self.wallpaper_images(), &mut shown),
                false,
            ),
            WallpaperAction::Prev => (shown.history.pop(), true),
            WallpaperAction::Set(path) => (
                Path::new(path).is_file().then(|| PathBuf::from(path)),
                false,
            ),
            WallpaperAction::Pause => {
                shown.paused = !shown.paused;
                self.shown = shown;
                self.schedule_wallpaper();
                return;
            }
        };
        if let Some(next) = next {
            tracing::debug!("wallpaper: {}", next.display());
            if let Some(current) = shown.current.take()
                && !back
                && current != next
            {
                if shown.history.len() == HISTORY {
                    shown.history.remove(0);
                }
                shown.history.push(current);
            }
            shown.played.insert(next.clone());
            shown.current = Some(next);
        }
        self.shown = shown;
        self.show_wallpaper();
    }

    /// Shows the current image and sets the timer for the next change.
    fn show_wallpaper(&mut self) {
        if let Some(backdrop) = &self.backdrop {
            backdrop.show(&self.loaded.config.wallpaper, self.shown.current.clone());
        }
        self.schedule_wallpaper();
    }

    /// Sets when the wallpaper next changes, if it cycles.
    fn schedule_wallpaper(&mut self) {
        let cycle = &self.wallpaper_config().cycle;
        self.wallpaper_at = (self.backdrop.is_some() && cycle.enabled && !self.shown.paused)
            .then(|| Instant::now() + cycle.interval());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn alphabetical_wraps_round() {
        let all = paths(&["a.png", "b.png", "c.png"]);
        let mut shown = Shown::default();
        let mut seen = Vec::new();
        for _ in 0..4 {
            let next = pick(CycleOrder::Alphabetical, &all, &mut shown).unwrap();
            shown.current = Some(next.clone());
            seen.push(next);
        }
        assert_eq!(seen, paths(&["a.png", "b.png", "c.png", "a.png"]));
    }

    #[test]
    fn random_plays_every_image_before_repeating() {
        let all = paths(&["a.png", "b.png", "c.png"]);
        let mut shown = Shown::default();
        let mut round = HashSet::new();
        for _ in 0..3 {
            let next = pick(CycleOrder::Random, &all, &mut shown).unwrap();
            assert!(round.insert(next.clone()), "{next:?} came twice");
            shown.current = Some(next);
        }
        let again = pick(CycleOrder::Random, &all, &mut shown).unwrap();
        assert_ne!(Some(&again), shown.current.as_ref());
    }

    #[test]
    fn a_single_image_stays() {
        let all = paths(&["a.png"]);
        let mut shown = Shown {
            current: Some(PathBuf::from("a.png")),
            ..Shown::default()
        };
        assert_eq!(
            pick(CycleOrder::Random, &all, &mut shown),
            Some(PathBuf::from("a.png"))
        );
        assert_eq!(pick(CycleOrder::Random, &[], &mut shown), None);
    }

    #[test]
    fn only_images_are_found() {
        let dir = std::env::temp_dir().join(format!("tile-wallpaper-{}", std::process::id()));
        let inner = dir.join("inner");
        fs::create_dir_all(&inner).unwrap();
        for file in ["a.PNG", "notes.txt", "inner/b.jpg"] {
            fs::write(dir.join(file), b"").unwrap();
        }
        assert_eq!(images(&dir, false), vec![dir.join("a.PNG")]);
        assert_eq!(
            images(&dir, true),
            vec![dir.join("a.PNG"), dir.join("inner/b.jpg")]
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
