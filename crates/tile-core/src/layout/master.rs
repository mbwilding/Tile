use super::{Layout, LayoutSettings, Tile, WindowList, clamp_ratio, equal_tiles};
use crate::{
    WindowId,
    geometry::{Axis, Rect, ratio_of},
};

/// Master/stack: `count` master windows on the left, the rest stacked on the right.
#[derive(Debug, Clone)]
pub struct Master {
    windows: WindowList,
    mfact: f32,
}

impl Master {
    /// An empty layout with the master column at `s.master_mfact`.
    pub fn new(s: &LayoutSettings) -> Self {
        Self {
            windows: WindowList::default(),
            mfact: s.master_mfact,
        }
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
    }

    fn windows(&self) -> Vec<WindowId> {
        self.windows.0.clone()
    }

    fn arrange(&mut self, area: Rect, _focused: Option<WindowId>, s: &LayoutSettings) -> Vec<Tile> {
        let windows = &self.windows.0;
        if windows.len() <= s.master_count {
            return equal_tiles(area, Axis::Vertical, windows, s.gaps_in);
        }
        let (master_area, stack_area) = area.split(Axis::Horizontal, self.mfact, s.gaps_in);
        let (masters, stack) = windows.split_at(s.master_count);
        let mut tiles = equal_tiles(master_area, Axis::Vertical, masters, s.gaps_in);
        tiles.extend(equal_tiles(stack_area, Axis::Vertical, stack, s.gaps_in));
        tiles
    }

    fn swap(&mut self, a: WindowId, b: WindowId) {
        self.windows.swap(a, b);
    }

    fn resize(&mut self, id: WindowId, dx: i32, _dy: i32, area: Rect, s: &LayoutSettings) {
        let Some(pos) = self.windows.position(id) else {
            return;
        };
        let change = ratio_of(dx, area.w);
        let signed = if pos < s.master_count {
            change
        } else {
            -change
        };
        self.mfact = clamp_ratio(self.mfact + signed);
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
            master_mfact: 0.5,
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
}
