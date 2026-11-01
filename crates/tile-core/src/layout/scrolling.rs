use super::{CenterMode, Layout, LayoutSettings, Tile, clamp_proportion};
use crate::{
    WindowId,
    geometry::{Axis, Direction, Rect, ratio_of, scale},
};

const MIN_WEIGHT: f32 = 0.1;

/// Scrolling: an endless horizontal strip of columns, scrolled to keep the focus in view.
#[derive(Debug, Clone, Default)]
pub struct Scrolling {
    columns: Vec<Column>,
    /// Horizontal scroll position of the viewport, relative to the strip start.
    offset: i32,
    focused_column: usize,
    center_once: bool,
}

#[derive(Debug, Clone)]
struct Column {
    windows: Vec<WindowId>,
    weights: Vec<f32>,
    proportion: f32,
    active: usize,
}

impl Column {
    fn new(id: WindowId, proportion: f32) -> Self {
        Self {
            windows: vec![id],
            weights: vec![1.0],
            proportion,
            active: 0,
        }
    }

    fn push(&mut self, id: WindowId) {
        self.windows.push(id);
        self.weights.push(1.0);
        self.active = self.windows.len() - 1;
    }

    fn take(&mut self, index: usize) -> WindowId {
        self.weights.remove(index);
        let id = self.windows.remove(index);
        self.active = self.active.min(self.windows.len().saturating_sub(1));
        id
    }
}

impl Scrolling {
    fn locate(&self, id: WindowId) -> Option<(usize, usize)> {
        self.columns
            .iter()
            .enumerate()
            .find_map(|(c, col)| col.windows.iter().position(|w| *w == id).map(|r| (c, r)))
    }

    fn drop_empty(&mut self) {
        self.columns.retain(|c| !c.windows.is_empty());
        self.focused_column = self
            .focused_column
            .min(self.columns.len().saturating_sub(1));
    }

    /// Sets the width of `id`'s column, as a share of the width.
    pub fn set_proportion(&mut self, id: WindowId, proportion: f32) {
        if let Some((c, _)) = self.locate(id) {
            self.columns[c].proportion = clamp_proportion(proportion);
        }
    }

    /// Steps `id`'s column to the next of the width presets, wrapping round.
    pub fn cycle_proportion(&mut self, id: WindowId, s: &LayoutSettings) {
        let Some((c, _)) = self.locate(id) else {
            return;
        };
        let current = self.columns[c].proportion;
        let next = s
            .scrolling_presets
            .iter()
            .copied()
            .find(|p| *p > current + 0.01)
            .or_else(|| s.scrolling_presets.first().copied())
            .unwrap_or(s.scrolling_proportion);
        self.columns[c].proportion = clamp_proportion(next);
    }

    /// Moves `id` into the neighbouring column in `dir`.
    pub fn consume(&mut self, id: WindowId, dir: Direction) {
        let Some((c, r)) = self.locate(id) else {
            return;
        };
        let target = match dir {
            Direction::Left if c > 0 => c - 1,
            Direction::Right if c + 1 < self.columns.len() => c + 1,
            _ => return,
        };
        let id = self.columns[c].take(r);
        self.columns[target].push(id);
        self.focused_column = target;
        self.drop_empty();
    }

    /// Pops `id` out of its column into a new column to the right.
    pub fn expel(&mut self, id: WindowId, s: &LayoutSettings) {
        self.expel_to(id, Direction::Right, s.scrolling_proportion);
    }

    fn expel_to(&mut self, id: WindowId, dir: Direction, proportion: f32) {
        let Some((c, r)) = self.locate(id) else {
            return;
        };
        if self.columns[c].windows.len() < 2 {
            return;
        }
        let id = self.columns[c].take(r);
        let at = if dir == Direction::Left { c } else { c + 1 };
        self.columns.insert(at, Column::new(id, proportion));
        self.focused_column = at;
    }

    /// Centres the focused column at the next arrangement.
    pub fn center(&mut self) {
        self.center_once = true;
    }

    fn column_widths(&self, usable: Rect, gap: i32) -> Vec<i32> {
        self.columns
            .iter()
            .map(|c| (scale(usable.w + gap, c.proportion) - gap).max(1))
            .collect()
    }

    fn update_offset(&mut self, xs: &[i32], widths: &[i32], view: i32, mode: CenterMode) {
        let fc = self.focused_column;
        let (Some(&x), Some(&w)) = (xs.get(fc), widths.get(fc)) else {
            self.offset = 0;
            return;
        };
        let centered = x + w / 2 - view / 2;
        let off_screen = x < self.offset || x + w > self.offset + view;
        let total = xs.last().copied().unwrap_or(0) + widths.last().copied().unwrap_or(0);
        self.offset = if self.center_once || mode == CenterMode::Always {
            centered
        } else if !off_screen {
            self.offset
        } else if mode == CenterMode::OnOverflow {
            centered
        } else if x < self.offset {
            x
        } else {
            x + w - view
        };
        self.center_once = false;
        if mode != CenterMode::Always {
            self.offset = self.offset.clamp(0, (total - view).max(0));
        }
    }
}

impl Layout for Scrolling {
    fn insert(&mut self, id: WindowId, focused: Option<WindowId>, _area: Rect, s: &LayoutSettings) {
        let at = focused
            .and_then(|f| self.locate(f))
            .map_or(self.columns.len(), |(c, _)| c + 1);
        self.columns
            .insert(at, Column::new(id, s.scrolling_proportion));
        self.focused_column = at;
    }

    fn remove(&mut self, id: WindowId) {
        if let Some((c, r)) = self.locate(id) {
            self.columns[c].take(r);
            if self.columns[c].windows.is_empty() && c < self.focused_column {
                self.focused_column -= 1;
            }
            self.drop_empty();
        }
    }

    fn windows(&self) -> Vec<WindowId> {
        self.columns
            .iter()
            .flat_map(|c| c.windows.iter().copied())
            .collect()
    }

    fn arrange(&mut self, area: Rect, focused: Option<WindowId>, s: &LayoutSettings) -> Vec<Tile> {
        if let Some((c, r)) = focused.and_then(|f| self.locate(f)) {
            self.focused_column = c;
            self.columns[c].active = r;
        }
        let widths = self.column_widths(area, s.gaps_in);
        let xs: Vec<i32> = widths
            .iter()
            .scan(0, |x, w| {
                let start = *x;
                *x += w + s.gaps_in;
                Some(start)
            })
            .collect();
        self.update_offset(&xs, &widths, area.w, s.scrolling_center);

        let mut tiles = Vec::new();
        for ((col, x), w) in self.columns.iter().zip(&xs).zip(&widths) {
            let rect = Rect::new(area.x + x - self.offset, area.y, *w, area.h);
            let visible = rect.x >= area.x - 1 && rect.right() <= area.right() + 1;
            let parts = rect.split_weighted(Axis::Vertical, &col.weights, s.gaps_in);
            tiles.extend(parts.into_iter().zip(&col.windows).map(|(rect, id)| Tile {
                id: *id,
                rect,
                visible,
            }));
        }
        tiles
    }

    fn swap(&mut self, a: WindowId, b: WindowId) {
        let (Some((ca, ra)), Some((cb, rb))) = (self.locate(a), self.locate(b)) else {
            return;
        };
        self.columns[ca].windows[ra] = b;
        self.columns[cb].windows[rb] = a;
    }

    fn resize(&mut self, id: WindowId, dx: i32, dy: i32, area: Rect, _s: &LayoutSettings) {
        let Some((c, r)) = self.locate(id) else {
            return;
        };
        let col = &mut self.columns[c];
        col.proportion = clamp_proportion(col.proportion + ratio_of(dx, area.w));
        if dy != 0 && col.windows.len() > 1 {
            let total: f32 = col.weights.iter().sum();
            let change = ratio_of(dy, area.h) * total;
            col.weights[r] = (col.weights[r] + change).max(MIN_WEIGHT);
        }
    }

    fn neighbor(&self, id: WindowId, dir: Direction, _tiles: &[Tile]) -> Option<WindowId> {
        let (c, r) = self.locate(id)?;
        match dir {
            Direction::Up => r.checked_sub(1).map(|r| self.columns[c].windows[r]),
            Direction::Down => self.columns[c].windows.get(r + 1).copied(),
            Direction::Left => {
                let col = self.columns.get(c.checked_sub(1)?)?;
                col.windows.get(col.active).copied()
            }
            Direction::Right => {
                let col = self.columns.get(c + 1)?;
                col.windows.get(col.active).copied()
            }
        }
    }

    fn move_window(&mut self, id: WindowId, dir: Direction, _tiles: &[Tile]) -> bool {
        let Some((c, r)) = self.locate(id) else {
            return false;
        };
        match dir {
            Direction::Up | Direction::Down => {
                let col = &mut self.columns[c];
                let target = if dir == Direction::Up {
                    r.checked_sub(1)
                } else {
                    Some(r + 1)
                };
                match target.filter(|t| *t < col.windows.len()) {
                    Some(t) => {
                        col.windows.swap(r, t);
                        col.weights.swap(r, t);
                        col.active = t;
                        true
                    }
                    None => false,
                }
            }
            Direction::Left | Direction::Right => {
                if self.columns[c].windows.len() > 1 {
                    self.expel_to(id, dir, self.columns[c].proportion);
                    return true;
                }
                let target = if dir == Direction::Left {
                    c.checked_sub(1)
                } else {
                    Some(c + 1)
                };
                match target.filter(|t| *t < self.columns.len()) {
                    Some(t) => {
                        self.columns.swap(c, t);
                        self.focused_column = t;
                        true
                    }
                    None => false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 800);

    fn settings(center: CenterMode) -> LayoutSettings {
        LayoutSettings {
            gaps_in: 0,
            gaps_out: 0,
            scrolling_proportion: 0.5,
            scrolling_center: center,
            ..LayoutSettings::default()
        }
    }

    fn filled(s: &LayoutSettings, n: isize) -> Scrolling {
        let mut l = Scrolling::default();
        let mut prev = None;
        for id in 1..=n {
            l.insert(WindowId(id), prev, AREA, s);
            prev = Some(WindowId(id));
        }
        l
    }

    #[test]
    fn scrolls_focused_column_into_view() {
        let s = settings(CenterMode::Never);
        let mut l = filled(&s, 4);
        let tiles = l.arrange(AREA, Some(WindowId(1)), &s);
        let visible: Vec<_> = tiles.iter().filter(|t| t.visible).map(|t| t.id.0).collect();
        assert_eq!(visible, vec![1, 2]);

        let tiles = l.arrange(AREA, Some(WindowId(4)), &s);
        let visible: Vec<_> = tiles.iter().filter(|t| t.visible).map(|t| t.id.0).collect();
        assert_eq!(visible, vec![3, 4]);
        assert_eq!(tiles[3].rect, Rect::new(500, 0, 500, 800));
    }

    #[test]
    fn always_centers() {
        let s = settings(CenterMode::Always);
        let mut l = filled(&s, 3);
        let tiles = l.arrange(AREA, Some(WindowId(2)), &s);
        assert_eq!(tiles[1].rect.x, 250);
        assert!(!tiles[0].visible && !tiles[2].visible);
    }

    #[test]
    fn consume_and_expel() {
        let s = settings(CenterMode::Never);
        let mut l = filled(&s, 3);
        l.consume(WindowId(3), Direction::Left);
        let tiles = l.arrange(AREA, Some(WindowId(3)), &s);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[1].rect, Rect::new(500, 0, 500, 400));
        assert_eq!(tiles[2].rect, Rect::new(500, 400, 500, 400));
        assert_eq!(
            l.neighbor(WindowId(3), Direction::Up, &tiles),
            Some(WindowId(2))
        );

        l.expel(WindowId(3), &s);
        assert_eq!(l.columns.len(), 3);
        assert_eq!(l.windows(), vec![WindowId(1), WindowId(2), WindowId(3)]);
    }

    #[test]
    fn cycle_proportion_wraps() {
        let s = settings(CenterMode::Never);
        let mut l = filled(&s, 1);
        l.cycle_proportion(WindowId(1), &s);
        assert_eq!(l.columns[0].proportion, 1.0);
        l.cycle_proportion(WindowId(1), &s);
        assert_eq!(l.columns[0].proportion, 0.25);
        l.cycle_proportion(WindowId(1), &s);
        assert_eq!(l.columns[0].proportion, 0.333);
    }

    #[test]
    fn move_window_swaps_columns() {
        let s = settings(CenterMode::Never);
        let mut l = filled(&s, 2);
        assert!(l.move_window(WindowId(1), Direction::Right, &[]));
        assert_eq!(l.windows(), vec![WindowId(2), WindowId(1)]);
        assert!(!l.move_window(WindowId(1), Direction::Right, &[]));
    }
}
