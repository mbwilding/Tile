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
    /// The window that had focus at the last arrangement, to tell when focus moved.
    last_focus: Option<WindowId>,
    /// How long the strip was at the last arrangement, to tell when it shrank.
    last_total: i32,
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
        // A lone column has the whole area, whatever its width is when there are more.
        if self.columns.len() == 1 {
            return vec![usable.w.max(1)];
        }
        self.columns
            .iter()
            .map(|c| (scale(usable.w + gap, c.proportion) - gap).max(1))
            .collect()
    }

    /// Scrolls like niri: the view stays still while the focused column is in it, and otherwise
    /// moves just far enough to bring the column in. `OnOverflow` centres a newly focused
    /// column only when it doesn't fit in view together with the column that had focus.
    fn update_offset(
        &mut self,
        xs: &[i32],
        widths: &[i32],
        view: i32,
        mode: CenterMode,
        previous: Option<usize>,
    ) {
        let fc = self.focused_column;
        let (Some(&x), Some(&w)) = (xs.get(fc), widths.get(fc)) else {
            self.offset = 0;
            return;
        };
        let centered = x + w / 2 - view / 2;
        let total = xs.last().copied().unwrap_or(0) + widths.last().copied().unwrap_or(0);
        let overflows = previous.filter(|p| *p != fc).is_some_and(|p| {
            let (px, pw) = (xs[p], widths[p]);
            (x + w).max(px + pw) - x.min(px) > view
        });
        let end = (total - view).max(0);
        let centre = self.center_once
            || mode == CenterMode::Always
            || (mode == CenterMode::OnOverflow && overflows);
        self.offset = if centre {
            centered
        } else if x < self.offset {
            x.min(end)
        } else if x + w > self.offset + view {
            (x + w - view).max(0)
        } else {
            self.offset
        };
        self.center_once = false;
        // Closing columns shortens the strip: pull the view back so it isn't left past the end,
        // with only the first column showing.
        if mode == CenterMode::Never || total < self.last_total {
            self.offset = self.offset.min(end).max(0);
        }
        self.last_total = total;
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
        let previous = self
            .last_focus
            .filter(|last| Some(*last) != focused)
            .and_then(|last| self.locate(last))
            .map(|(c, _)| c);
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
        self.update_offset(&xs, &widths, area.w, s.scrolling_center, previous);
        self.last_focus = focused.or(self.last_focus);

        // Columns reaching into the outer gap still show, cut off at the monitor's edge.
        let view = Rect::new(area.x - s.gaps_out, area.y, area.w + s.gaps_out * 2, area.h);
        let mut tiles = Vec::new();
        for ((col, x), w) in self.columns.iter().zip(&xs).zip(&widths) {
            let rect = Rect::new(area.x + x - self.offset, area.y, *w, area.h);
            let visible = rect.overlaps(&view);
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
    fn focus_moves_the_view_only_when_the_column_is_out_of_it() {
        let s = settings(CenterMode::Never);
        let mut l = filled(&s, 4);
        let x = |l: &mut Scrolling, id| {
            l.arrange(AREA, Some(WindowId(id)), &s)
                .iter()
                .find(|t| t.id.0 == id)
                .map(|t| t.rect.x)
        };
        assert_eq!(x(&mut l, 1), Some(0));
        assert_eq!(x(&mut l, 2), Some(500));
        assert_eq!(x(&mut l, 3), Some(500));
        assert_eq!(x(&mut l, 2), Some(0));
    }

    #[test]
    fn on_overflow_centres_only_what_does_not_fit_with_the_previous_column() {
        let mut s = settings(CenterMode::OnOverflow);
        let mut l = filled(&s, 4);
        l.arrange(AREA, Some(WindowId(1)), &s);
        let tiles = l.arrange(AREA, Some(WindowId(2)), &s);
        assert_eq!(tiles[1].rect.x, 500);
        s.scrolling_proportion = 0.5;
        let mut wide = Scrolling::default();
        wide.insert(WindowId(1), None, AREA, &s);
        wide.insert(WindowId(2), Some(WindowId(1)), AREA, &s);
        wide.set_proportion(WindowId(1), 0.7);
        wide.set_proportion(WindowId(2), 0.7);
        wide.arrange(AREA, Some(WindowId(1)), &s);
        let tiles = wide.arrange(AREA, Some(WindowId(2)), &s);
        assert_eq!(tiles[1].rect.x, 150);
        let tiles = wide.arrange(AREA, Some(WindowId(2)), &s);
        assert_eq!(tiles[1].rect.x, 150, "a centred column stays put");
    }

    #[test]
    fn a_lone_window_has_the_whole_area() {
        let s = settings(CenterMode::Never);
        let mut l = filled(&s, 1);
        assert_eq!(l.arrange(AREA, Some(WindowId(1)), &s)[0].rect, AREA);
        l.set_proportion(WindowId(1), 0.3);
        assert_eq!(l.arrange(AREA, Some(WindowId(1)), &s)[0].rect, AREA);
        l.insert(WindowId(2), Some(WindowId(1)), AREA, &s);
        let tiles = l.arrange(AREA, Some(WindowId(2)), &s);
        assert_eq!(tiles[1].rect.w, 500);
    }

    #[test]
    fn closing_the_last_column_keeps_two_in_view() {
        for mode in [CenterMode::Never, CenterMode::OnOverflow] {
            let s = settings(mode);
            let mut l = filled(&s, 3);
            l.arrange(AREA, Some(WindowId(3)), &s);
            l.remove(WindowId(3));
            let tiles = l.arrange(AREA, Some(WindowId(2)), &s);
            assert_eq!(tiles[0].rect.x, 0, "{mode:?}");
            assert_eq!(tiles[1].rect.x, 500, "{mode:?}");
        }
    }

    #[test]
    fn always_centers() {
        let s = settings(CenterMode::Always);
        let mut l = filled(&s, 3);
        let tiles = l.arrange(AREA, Some(WindowId(2)), &s);
        assert_eq!(tiles[1].rect.x, 250);
        assert!(tiles.iter().all(|t| t.visible));
        assert_eq!(tiles[0].rect.x, -250);
        assert_eq!(tiles[2].rect.x, 750);
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
