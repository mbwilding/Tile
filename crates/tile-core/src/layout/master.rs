use std::collections::HashMap;

use super::{Layout, LayoutSettings, Tile, WindowList, clamp_ratio};
use crate::{
    WindowId,
    geometry::{Axis, Rect, ratio_of},
};

/// Master/stack: `count` master windows on the left, the rest stacked on the right.
#[derive(Debug, Clone)]
pub struct Master {
    windows: WindowList,
    ratio: f32,
    /// How tall each window is next to the others of its column; 1.0 where unset.
    weights: HashMap<WindowId, f32>,
}

/// The least share of its column a window can be given.
const MIN_WEIGHT: f32 = 0.1;

impl Master {
    /// An empty layout with the master column at `s.master_ratio`.
    pub fn new(s: &LayoutSettings) -> Self {
        Self {
            windows: WindowList::default(),
            ratio: s.master_ratio,
            weights: HashMap::new(),
        }
    }

    /// Sets the master column's share of the width.
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = clamp_ratio(ratio);
    }

    /// Swaps `id` with the first master, or a master with the first stacked window.
    pub fn swap_with_master(&mut self, id: WindowId, s: &LayoutSettings) {
        let Some(pos) = self.windows.position(id) else {
            return;
        };
        let windows = &mut self.windows.0;
        if pos < s.master_count {
            if windows.len() > s.master_count {
                windows.swap(pos, s.master_count);
            }
        } else {
            windows.swap(pos, 0);
        }
    }

    fn weight(&self, id: WindowId) -> f32 {
        self.weights.get(&id).copied().unwrap_or(1.0)
    }

    /// Stacks `ids` in `rect`, each as tall as its weight says.
    fn column(&self, rect: Rect, ids: &[WindowId], gap: i32) -> Vec<Tile> {
        let weights: Vec<f32> = ids.iter().map(|id| self.weight(*id)).collect();
        rect.split_weighted(Axis::Vertical, &weights, gap)
            .into_iter()
            .zip(ids)
            .map(|(rect, id)| Tile {
                id: *id,
                rect,
                visible: true,
            })
            .collect()
    }
}

impl Layout for Master {
    fn insert(
        &mut self,
        id: WindowId,
        _focused: Option<WindowId>,
        _area: Rect,
        s: &LayoutSettings,
    ) {
        if s.master_new_is_master {
            self.windows.0.insert(0, id);
        } else {
            self.windows.0.push(id);
        }
    }

    fn remove(&mut self, id: WindowId) {
        self.windows.remove(id);
        self.weights.remove(&id);
    }

    fn windows(&self) -> Vec<WindowId> {
        self.windows.0.clone()
    }

    fn arrange(&mut self, area: Rect, _focused: Option<WindowId>, s: &LayoutSettings) -> Vec<Tile> {
        let windows = &self.windows.0;
        if windows.len() <= s.master_count {
            return self.column(area, windows, s.gaps_in);
        }
        let (master_area, stack_area) = area.split(Axis::Horizontal, self.ratio, s.gaps_in);
        let (masters, stack) = windows.split_at(s.master_count);
        let mut tiles = self.column(master_area, masters, s.gaps_in);
        tiles.extend(self.column(stack_area, stack, s.gaps_in));
        tiles
    }

    fn swap(&mut self, a: WindowId, b: WindowId) {
        self.windows.swap(a, b);
    }

    fn resize(&mut self, id: WindowId, dx: i32, dy: i32, area: Rect, s: &LayoutSettings) {
        let Some(pos) = self.windows.position(id) else {
            return;
        };
        let change = ratio_of(dx, area.w);
        let signed = if pos < s.master_count {
            change
        } else {
            -change
        };
        self.ratio = clamp_ratio(self.ratio + signed);

        let windows = &self.windows.0;
        let column = if windows.len() <= s.master_count || pos < s.master_count {
            &windows[..s.master_count.min(windows.len())]
        } else {
            &windows[s.master_count..]
        };
        if dy != 0 && column.len() > 1 {
            let total: f32 = column.iter().map(|w| self.weight(*w)).sum();
            let grown = self.weight(id) + ratio_of(dy, area.h) * total;
            self.weights.insert(id, grown.max(MIN_WEIGHT));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_and_stack() {
        let s = LayoutSettings {
            gaps_in: 0,
            gaps_out: 0,
            master_ratio: 0.5,
            ..LayoutSettings::default()
        };
        let area = Rect::new(0, 0, 1000, 900);
        let mut m = Master::new(&s);
        for id in 1..=4 {
            m.insert(WindowId(id), None, area, &s);
        }
        let tiles = m.arrange(area, None, &s);
        assert_eq!(tiles[0].rect, Rect::new(0, 0, 500, 900));
        assert_eq!(tiles[1].rect, Rect::new(500, 0, 500, 300));
        assert_eq!(tiles[3].rect, Rect::new(500, 600, 500, 300));

        m.swap_with_master(WindowId(3), &s);
        assert_eq!(m.windows()[0], WindowId(3));

        m.resize(WindowId(3), 100, 0, area, &s);
        assert_eq!(m.arrange(area, None, &s)[0].rect.w, 600);
    }

    #[test]
    fn resizing_vertically_trades_height_within_the_column() {
        let s = LayoutSettings {
            gaps_in: 0,
            gaps_out: 0,
            ..LayoutSettings::default()
        };
        let area = Rect::new(0, 0, 1000, 900);
        let mut m = Master::new(&s);
        for id in 1..=3 {
            m.insert(WindowId(id), None, area, &s);
        }
        m.resize(WindowId(2), 0, 150, area, &s);
        let tiles = m.arrange(area, None, &s);
        assert_eq!(tiles[0].rect.w, 500);
        assert_eq!(tiles[1].rect.h + tiles[2].rect.h, 900);
        assert!(tiles[1].rect.h > 450 && tiles[2].rect.h < 450);
        // A lone master has no neighbour to trade with.
        let mut lone = Master::new(&s);
        lone.insert(WindowId(1), None, area, &s);
        lone.resize(WindowId(1), 0, 150, area, &s);
        assert_eq!(lone.arrange(area, None, &s)[0].rect.h, 900);
    }
}
