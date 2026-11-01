use super::{Layout, LayoutSettings, Tile, WindowList};
use crate::{
    WindowId,
    geometry::{Direction, Rect},
};

/// Every window fills the workspace; focus cycles through them.
#[derive(Debug, Clone, Default)]
pub struct Monocle {
    windows: WindowList,
}

impl Layout for Monocle {
    fn insert(
        &mut self,
        id: WindowId,
        _focused: Option<WindowId>,
        _area: Rect,
        _s: &LayoutSettings,
    ) {
        self.windows.0.push(id);
    }

    fn remove(&mut self, id: WindowId) {
        self.windows.remove(id);
    }

    fn windows(&self) -> Vec<WindowId> {
        self.windows.0.clone()
    }

    fn arrange(
        &mut self,
        area: Rect,
        _focused: Option<WindowId>,
        _s: &LayoutSettings,
    ) -> Vec<Tile> {
        self.windows
            .0
            .iter()
            .map(|id| Tile {
                id: *id,
                rect: area,
                visible: true,
            })
            .collect()
    }

    fn swap(&mut self, a: WindowId, b: WindowId) {
        self.windows.swap(a, b);
    }

    fn neighbor(&self, id: WindowId, dir: Direction, _tiles: &[Tile]) -> Option<WindowId> {
        let windows = &self.windows.0;
        let len = windows.len();
        let pos = self.windows.position(id)?;
        if len < 2 {
            return None;
        }
        let next = if dir.is_backward() {
            (pos + len - 1) % len
        } else {
            (pos + 1) % len
        };
        Some(windows[next])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: WindowId = WindowId(1);
    const B: WindowId = WindowId(2);
    const C: WindowId = WindowId(3);

    fn filled(ids: &[WindowId]) -> (Monocle, LayoutSettings) {
        let s = LayoutSettings::default();
        let mut m = Monocle::default();
        for id in ids {
            m.insert(*id, None, Rect::default(), &s);
        }
        (m, s)
    }

    #[test]
    fn every_window_fills_the_area() {
        let (mut m, s) = filled(&[A, B]);
        let expected = Rect::new(0, 0, 1000, 800);
        let tiles = m.arrange(expected, None, &s);
        assert_eq!(tiles.len(), 2);
        assert!(tiles.iter().all(|t| t.rect == expected && t.visible));
    }

    #[test]
    fn windows_keep_insertion_order_and_can_be_removed() {
        let (mut m, _) = filled(&[A, B, C]);
        assert_eq!(m.windows(), vec![A, B, C]);
        assert!(m.contains(B));
        m.remove(B);
        assert_eq!(m.windows(), vec![A, C]);
        assert!(!m.contains(B));
    }

    #[test]
    fn swap_exchanges_positions_and_ignores_strangers() {
        let (mut m, _) = filled(&[A, B, C]);
        m.swap(A, C);
        assert_eq!(m.windows(), vec![C, B, A]);
        m.swap(A, WindowId(99));
        assert_eq!(m.windows(), vec![C, B, A]);
    }

    #[test]
    fn neighbours_cycle_and_wrap() {
        let (m, _) = filled(&[A, B, C]);
        assert_eq!(m.neighbor(A, Direction::Right, &[]), Some(B));
        assert_eq!(m.neighbor(C, Direction::Down, &[]), Some(A));
        assert_eq!(m.neighbor(A, Direction::Left, &[]), Some(C));
        assert_eq!(m.neighbor(B, Direction::Up, &[]), Some(A));
        assert_eq!(m.neighbor(WindowId(99), Direction::Right, &[]), None);
    }

    #[test]
    fn a_lone_window_has_no_neighbour_and_resize_does_nothing() {
        let (mut m, s) = filled(&[A]);
        assert_eq!(m.neighbor(A, Direction::Right, &[]), None);
        m.resize(A, 50, 50, Rect::new(0, 0, 100, 100), &s);
        assert_eq!(m.windows(), vec![A]);
    }
}
