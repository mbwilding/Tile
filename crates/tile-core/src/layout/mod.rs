mod dwindle;
mod even;
mod master;
mod monocle;

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

pub use dwindle::Dwindle;
pub use even::Even;
pub use master::Master;
pub use monocle::Monocle;

use crate::{
    WindowId,
    geometry::{Axis, Direction, Rect, nearest_in_direction},
};

/// The tiling layouts a workspace can use. See `docs/layouts.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutKind {
    /// A binary tree: each new window splits the focused one.
    Dwindle,
    /// Master windows on the left, the rest stacked on the right.
    Master,
    /// Every window fills the workspace.
    Monocle,
    /// Equal shares, side by side or stacked.
    Even,
}

impl LayoutKind {
    /// Every layout, in the order `cyclelayout` steps through them.
    pub const ALL: [Self; 4] = [Self::Dwindle, Self::Master, Self::Monocle, Self::Even];

    /// The layout after this one, wrapping round.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

impl FromStr for LayoutKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "dwindle" => Ok(Self::Dwindle),
            "master" => Ok(Self::Master),
            "monocle" => Ok(Self::Monocle),
            "even" => Ok(Self::Even),
            _ => Err(()),
        }
    }
}

impl LayoutKind {
    /// The name as shown to the user, like `Master`.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dwindle => "Dwindle",
            Self::Master => "Master",
            Self::Monocle => "Monocle",
            Self::Even => "Even",
        }
    }
}

impl fmt::Display for LayoutKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dwindle => "dwindle",
            Self::Master => "master",
            Self::Monocle => "monocle",
            Self::Even => "even",
        })
    }
}

/// Layout tunables, derived from the user config. The config's defaults and limits are the
/// constants here.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutSettings {
    /// Space between neighbouring windows.
    pub gaps_in: i32,
    /// Space between windows and the edge of the area.
    pub gaps_out: i32,
    /// The share of a new dwindle split given to the window that was there.
    pub dwindle_split_ratio: f32,
    /// The master column's share of the width.
    pub master_ratio: f32,
    /// How many windows share the master column, at least [`Self::MIN_MASTER_COUNT`].
    pub master_count: usize,
    /// New windows become the master instead of joining the stack.
    pub master_new_is_master: bool,
    /// Each monitor has workspaces of its own. Off, switching workspace switches every monitor.
    pub independent_workspaces: bool,
}

impl LayoutSettings {
    /// Default [`gaps_in`](Self::gaps_in), in pixels.
    pub const DEFAULT_GAPS_IN: i32 = 20;
    /// Default [`gaps_out`](Self::gaps_out), in pixels.
    pub const DEFAULT_GAPS_OUT: i32 = 20;
    /// Default [`dwindle_split_ratio`](Self::dwindle_split_ratio).
    pub const DEFAULT_SPLIT_RATIO: f32 = 0.5;
    /// Default [`master_ratio`](Self::master_ratio).
    pub const DEFAULT_MASTER_RATIO: f32 = 0.5;
    /// Default [`master_count`](Self::master_count).
    pub const DEFAULT_MASTER_COUNT: usize = 1;

    /// The smallest share of a dwindle split or the master column.
    pub const MIN_RATIO: f32 = 0.1;
    /// The largest share of a dwindle split or the master column.
    pub const MAX_RATIO: f32 = 0.9;
    /// The fewest windows in the master column.
    pub const MIN_MASTER_COUNT: usize = 1;

    /// These settings with every value brought within its limits.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.dwindle_split_ratio = clamp_ratio(self.dwindle_split_ratio);
        self.master_ratio = clamp_ratio(self.master_ratio);
        self.master_count = self.master_count.max(Self::MIN_MASTER_COUNT);
        self
    }

    /// The part of a monitor's work area that windows tile in: inside the outer gap.
    pub fn tiling_area(&self, work_area: Rect) -> Rect {
        work_area.shrink(self.gaps_out)
    }
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            gaps_in: Self::DEFAULT_GAPS_IN,
            gaps_out: Self::DEFAULT_GAPS_OUT,
            dwindle_split_ratio: Self::DEFAULT_SPLIT_RATIO,
            master_ratio: Self::DEFAULT_MASTER_RATIO,
            master_count: Self::DEFAULT_MASTER_COUNT,
            master_new_is_master: false,
            independent_workspaces: true,
        }
    }
}

fn clamp_ratio(ratio: f32) -> f32 {
    ratio.clamp(LayoutSettings::MIN_RATIO, LayoutSettings::MAX_RATIO)
}

/// The windows of a list-based layout, in layout order.
#[derive(Debug, Clone, Default)]
struct WindowList(Vec<WindowId>);

impl WindowList {
    fn position(&self, id: WindowId) -> Option<usize> {
        self.0.iter().position(|w| *w == id)
    }

    fn remove(&mut self, id: WindowId) {
        self.0.retain(|w| *w != id);
    }

    fn swap(&mut self, a: WindowId, b: WindowId) {
        if let (Some(pa), Some(pb)) = (self.position(a), self.position(b)) {
            self.0.swap(pa, pb);
        }
    }
}

/// Tiles `ids` in equal parts of `rect`, side by side or stacked along `axis`.
fn equal_tiles(rect: Rect, axis: Axis, ids: &[WindowId], gap: i32) -> Vec<Tile> {
    rect.split_weighted(axis, &vec![1.0; ids.len()], gap)
        .into_iter()
        .zip(ids)
        .map(|(rect, id)| Tile {
            id: *id,
            rect,
            visible: true,
        })
        .collect()
}

/// A window's computed position. Invisible tiles must be hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    /// The window.
    pub id: WindowId,
    /// Where it goes.
    pub rect: Rect,
    /// Whether it is on screen.
    pub visible: bool,
}

/// Behaviour common to every tiling layout. `area` is always the
/// [tiling area](LayoutSettings::tiling_area), already inside the outer gap.
pub trait Layout {
    /// Adds a window, placing it relative to `focused` when that is part of the layout.
    fn insert(&mut self, id: WindowId, focused: Option<WindowId>, area: Rect, s: &LayoutSettings);
    /// Takes a window out.
    fn remove(&mut self, id: WindowId);
    /// All windows in a stable, layout-defined order.
    fn windows(&self) -> Vec<WindowId>;
    /// Where every window goes in `area`, with `focused` kept in view.
    fn arrange(&mut self, area: Rect, focused: Option<WindowId>, s: &LayoutSettings) -> Vec<Tile>;
    /// Exchanges two windows' places, if both are in the layout.
    fn swap(&mut self, a: WindowId, b: WindowId);

    /// Grows (or shrinks) a window by pixels. Layouts without sizes ignore it.
    fn resize(&mut self, _id: WindowId, _dx: i32, _dy: i32, _area: Rect, _s: &LayoutSettings) {}

    /// Whether `id` is in the layout.
    fn contains(&self, id: WindowId) -> bool {
        self.windows().contains(&id)
    }

    /// The window `dir` of `id`, given where they are.
    fn neighbor(&self, id: WindowId, dir: Direction, tiles: &[Tile]) -> Option<WindowId> {
        let from = tiles.iter().find(|t| t.id == id)?.rect;
        nearest_in_direction(
            &from,
            dir,
            tiles.iter().filter(|t| t.id != id).map(|t| (t.id, t.rect)),
        )
    }

    /// Moves a window within the layout. Returns `false` when it is already at the edge,
    /// letting the caller move it to another monitor instead.
    fn move_window(&mut self, id: WindowId, dir: Direction, tiles: &[Tile]) -> bool {
        match self.neighbor(id, dir, tiles) {
            Some(other) => {
                self.swap(id, other);
                true
            }
            None => false,
        }
    }
}

/// The active layout of a workspace. Its methods take the monitor's work area and lay windows
/// out in its [tiling area](LayoutSettings::tiling_area).
#[derive(Debug, Clone)]
pub enum LayoutState {
    /// See [`Dwindle`].
    Dwindle(Dwindle),
    /// See [`Master`].
    Master(Master),
    /// See [`Monocle`].
    Monocle(Monocle),
    /// See [`Even`].
    Even(Even),
}

impl LayoutState {
    /// An empty layout of `kind`.
    pub fn new(kind: LayoutKind, s: &LayoutSettings) -> Self {
        match kind {
            LayoutKind::Dwindle => Self::Dwindle(Dwindle::default()),
            LayoutKind::Master => Self::Master(Master::new(s)),
            LayoutKind::Monocle => Self::Monocle(Monocle::default()),
            LayoutKind::Even => Self::Even(Even::default()),
        }
    }

    /// Brings the sizes this layout keeps for itself in line with `new` where they differ from
    /// `old`, as a config reload does: the master column's width and every dwindle split's ratio.
    /// The rest of the settings are read as they are used.
    pub fn adopt_settings(&mut self, old: &LayoutSettings, new: &LayoutSettings) {
        match self {
            Self::Master(l) if old.master_ratio != new.master_ratio => {
                l.set_ratio(new.master_ratio);
            }
            Self::Dwindle(l) if old.dwindle_split_ratio != new.dwindle_split_ratio => {
                l.set_ratios(new.dwindle_split_ratio);
            }
            _ => {}
        }
    }

    /// Which layout this is.
    pub const fn kind(&self) -> LayoutKind {
        match self {
            Self::Dwindle(_) => LayoutKind::Dwindle,
            Self::Master(_) => LayoutKind::Master,
            Self::Monocle(_) => LayoutKind::Monocle,
            Self::Even(_) => LayoutKind::Even,
        }
    }

    /// Rebuilds the layout as `kind`, keeping the window order.
    pub fn convert(&self, kind: LayoutKind, work_area: Rect, s: &LayoutSettings) -> Self {
        let mut next = Self::new(kind, s);
        let mut prev = None;
        for id in self.windows() {
            next.insert(id, prev, work_area, s);
            prev = Some(id);
        }
        next
    }

    fn inner(&self) -> &dyn Layout {
        match self {
            Self::Dwindle(l) => l,
            Self::Master(l) => l,
            Self::Monocle(l) => l,
            Self::Even(l) => l,
        }
    }

    fn inner_mut(&mut self) -> &mut dyn Layout {
        match self {
            Self::Dwindle(l) => l,
            Self::Master(l) => l,
            Self::Monocle(l) => l,
            Self::Even(l) => l,
        }
    }

    /// See [`Layout::insert`].
    pub fn insert(
        &mut self,
        id: WindowId,
        focused: Option<WindowId>,
        work_area: Rect,
        s: &LayoutSettings,
    ) {
        self.inner_mut()
            .insert(id, focused, s.tiling_area(work_area), s);
    }

    /// See [`Layout::remove`].
    pub fn remove(&mut self, id: WindowId) {
        self.inner_mut().remove(id);
    }

    /// See [`Layout::windows`].
    pub fn windows(&self) -> Vec<WindowId> {
        self.inner().windows()
    }

    /// See [`Layout::contains`].
    pub fn contains(&self, id: WindowId) -> bool {
        self.inner().contains(id)
    }

    /// See [`Layout::arrange`].
    pub fn arrange(
        &mut self,
        work_area: Rect,
        focused: Option<WindowId>,
        s: &LayoutSettings,
    ) -> Vec<Tile> {
        self.inner_mut()
            .arrange(s.tiling_area(work_area), focused, s)
    }

    /// See [`Layout::swap`].
    pub fn swap(&mut self, a: WindowId, b: WindowId) {
        self.inner_mut().swap(a, b);
    }

    /// See [`Layout::resize`].
    pub fn resize(&mut self, id: WindowId, dx: i32, dy: i32, work_area: Rect, s: &LayoutSettings) {
        self.inner_mut()
            .resize(id, dx, dy, s.tiling_area(work_area), s);
    }

    /// See [`Layout::neighbor`].
    pub fn neighbor(&self, id: WindowId, dir: Direction, tiles: &[Tile]) -> Option<WindowId> {
        self.inner().neighbor(id, dir, tiles)
    }

    /// See [`Layout::move_window`].
    pub fn move_window(&mut self, id: WindowId, dir: Direction, tiles: &[Tile]) -> bool {
        self.inner_mut().move_window(id, dir, tiles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: WindowId = WindowId(1);
    const B: WindowId = WindowId(2);
    const C: WindowId = WindowId(3);
    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1000,
        h: 800,
    };

    fn filled(kind: LayoutKind) -> (LayoutState, LayoutSettings) {
        let s = LayoutSettings::default();
        let mut l = LayoutState::new(kind, &s);
        let mut prev = None;
        for id in [A, B, C] {
            l.insert(id, prev, AREA, &s);
            prev = Some(id);
        }
        (l, s)
    }

    #[test]
    fn kinds_cycle_through_all_and_wrap() {
        let mut kind = LayoutKind::ALL[0];
        let mut seen = Vec::new();
        for _ in 0..LayoutKind::ALL.len() {
            seen.push(kind);
            kind = kind.next();
        }
        assert_eq!(seen, LayoutKind::ALL.to_vec());
        assert_eq!(kind, LayoutKind::ALL[0]);
    }

    #[test]
    fn kinds_parse_and_print_their_names() {
        for kind in LayoutKind::ALL {
            assert_eq!(kind.to_string().parse::<LayoutKind>(), Ok(kind));
        }
        assert_eq!("MASTER".parse::<LayoutKind>(), Ok(LayoutKind::Master));
        assert!("grid".parse::<LayoutKind>().is_err());
    }

    #[test]
    fn a_new_layout_reports_its_kind_and_starts_empty() {
        let s = LayoutSettings::default();
        for kind in LayoutKind::ALL {
            let l = LayoutState::new(kind, &s);
            assert_eq!(l.kind(), kind);
            assert!(l.windows().is_empty());
        }
    }

    #[test]
    fn every_layout_tracks_inserted_and_removed_windows() {
        for kind in LayoutKind::ALL {
            let (mut l, s) = filled(kind);
            let mut windows = l.windows();
            windows.sort_by_key(|w| w.0);
            assert_eq!(windows, vec![A, B, C], "{kind}");
            assert!(l.contains(B));
            l.remove(B);
            assert!(!l.contains(B), "{kind}");
            let tiles = l.arrange(AREA, Some(A), &s);
            assert!(tiles.iter().all(|t| t.id != B), "{kind}");
        }
    }

    #[test]
    fn converting_keeps_every_window() {
        for from in LayoutKind::ALL {
            for to in LayoutKind::ALL {
                let (l, s) = filled(from);
                let converted = l.convert(to, AREA, &s);
                assert_eq!(converted.kind(), to);
                let mut windows = converted.windows();
                windows.sort_by_key(|w| w.0);
                assert_eq!(windows, vec![A, B, C], "{from} to {to}");
            }
        }
    }

    #[test]
    fn arranged_tiles_stay_inside_the_area() {
        for kind in LayoutKind::ALL {
            let (mut l, s) = filled(kind);
            for t in l.arrange(AREA, Some(A), &s).iter().filter(|t| t.visible) {
                assert!(t.rect.x >= AREA.x && t.rect.y >= AREA.y, "{kind}");
                assert!(t.rect.x + t.rect.w <= AREA.x + AREA.w, "{kind}");
                assert!(t.rect.y + t.rect.h <= AREA.y + AREA.h, "{kind}");
            }
        }
    }

    #[test]
    fn swapping_exchanges_two_windows() {
        for kind in [LayoutKind::Dwindle, LayoutKind::Master, LayoutKind::Even] {
            let (mut l, s) = filled(kind);
            let rect_of = |l: &mut LayoutState, id| {
                l.arrange(AREA, Some(A), &s)
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.rect)
            };
            let (a, c) = (rect_of(&mut l, A), rect_of(&mut l, C));
            l.swap(A, C);
            assert_eq!(rect_of(&mut l, A), c, "{kind}");
            assert_eq!(rect_of(&mut l, C), a, "{kind}");
        }
    }

    #[test]
    fn moving_a_window_stops_at_the_edge() {
        let (mut l, s) = filled(LayoutKind::Even);
        let tiles = l.arrange(AREA, Some(A), &s);
        assert!(!l.move_window(A, Direction::Left, &tiles));
        assert!(l.move_window(A, Direction::Right, &tiles));
        assert_eq!(l.windows(), vec![B, A, C]);
    }

    #[test]
    fn resizing_changes_the_arrangement() {
        let (mut l, s) = filled(LayoutKind::Dwindle);
        let before = l.arrange(AREA, Some(A), &s);
        l.resize(A, 100, 0, AREA, &s);
        assert_ne!(before, l.arrange(AREA, Some(A), &s));
    }

    #[test]
    fn default_settings_are_sane() {
        let s = LayoutSettings::default();
        assert!(s.independent_workspaces);
        assert!((0.0..=1.0).contains(&s.dwindle_split_ratio));
    }
}
