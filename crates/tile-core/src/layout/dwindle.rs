use super::{Layout, LayoutSettings, Tile, clamp_ratio};
use crate::{
    WindowId,
    geometry::{Axis, Rect, ratio_of},
};

/// Dwindle: a binary tree where each new window splits the focused one.
#[derive(Debug, Clone, Default)]
pub struct Dwindle {
    root: Option<Node>,
}

#[derive(Debug, Clone)]
enum Node {
    Leaf(WindowId),
    Split {
        /// `None` follows the aspect ratio of the node; `Some` was forced by `togglesplit`.
        axis: Option<Axis>,
        ratio: f32,
        children: Box<[Node; 2]>,
    },
}

enum Search {
    Missing,
    Found,
    Handled,
}

impl Node {
    /// The axis a split divides `rect` along (`None` follows its shape), and its two parts.
    fn child_rects(axis: Option<Axis>, ratio: f32, rect: Rect, gap: i32) -> (Axis, Rect, Rect) {
        let axis = axis.unwrap_or_else(|| Axis::for_rect(&rect));
        let (a, b) = rect.split(axis, ratio, gap);
        (axis, a, b)
    }

    fn leaves(&self, out: &mut Vec<WindowId>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Split { children, .. } => {
                children[0].leaves(out);
                children[1].leaves(out);
            }
        }
    }

    fn set_ratios(&mut self, to: f32) {
        if let Self::Split {
            ratio, children, ..
        } = self
        {
            *ratio = clamp_ratio(to);
            let [a, b] = children.as_mut();
            a.set_ratios(to);
            b.set_ratios(to);
        }
    }

    fn for_each_leaf_mut(&mut self, f: &mut impl FnMut(&mut WindowId)) {
        match self {
            Self::Leaf(id) => f(id),
            Self::Split { children, .. } => {
                let [a, b] = children.as_mut();
                a.for_each_leaf_mut(f);
                b.for_each_leaf_mut(f);
            }
        }
    }

    fn find_leaf_mut(&mut self, id: WindowId) -> Option<&mut Self> {
        match self {
            Self::Leaf(leaf) if *leaf == id => Some(self),
            Self::Leaf(_) => None,
            Self::Split { children, .. } => {
                let [a, b] = children.as_mut();
                a.find_leaf_mut(id).or_else(|| b.find_leaf_mut(id))
            }
        }
    }

    fn last_leaf_mut(&mut self) -> &mut Self {
        match self {
            Self::Leaf(_) => self,
            Self::Split { children, .. } => children[1].last_leaf_mut(),
        }
    }

    fn without(self, id: WindowId) -> Option<Self> {
        match self {
            Self::Leaf(leaf) if leaf == id => None,
            Self::Leaf(_) => Some(self),
            Self::Split {
                axis,
                ratio,
                children,
            } => {
                let [a, b] = *children;
                match (a.without(id), b.without(id)) {
                    (Some(a), Some(b)) => Some(Self::Split {
                        axis,
                        ratio,
                        children: Box::new([a, b]),
                    }),
                    (Some(n), None) | (None, Some(n)) => Some(n),
                    (None, None) => None,
                }
            }
        }
    }

    fn arrange(&self, rect: Rect, gap: i32, out: &mut Vec<Tile>) {
        match self {
            Self::Leaf(id) => out.push(Tile {
                id: *id,
                rect,
                visible: true,
            }),
            Self::Split {
                axis,
                ratio,
                children,
            } => {
                let (_, a, b) = Self::child_rects(*axis, *ratio, rect, gap);
                children[0].arrange(a, gap, out);
                children[1].arrange(b, gap, out);
            }
        }
    }

    fn is_leaf(&self, id: WindowId) -> bool {
        matches!(self, Self::Leaf(leaf) if *leaf == id)
    }

    fn toggle_split(&mut self, id: WindowId, rect: Rect, gap: i32) -> bool {
        let Self::Split {
            axis,
            ratio,
            children,
        } = self
        else {
            return false;
        };
        let (effective, ra, rb) = Self::child_rects(*axis, *ratio, rect, gap);
        if children.iter().any(|c| c.is_leaf(id)) {
            *axis = Some(effective.flip());
            return true;
        }
        let [a, b] = children.as_mut();
        a.toggle_split(id, ra, gap) || b.toggle_split(id, rb, gap)
    }

    /// Grows `id` by `delta` px along `along`, adjusting the nearest matching ancestor split.
    fn resize(&mut self, id: WindowId, rect: Rect, gap: i32, along: Axis, delta: i32) -> Search {
        match self {
            Self::Leaf(leaf) if *leaf == id => Search::Found,
            Self::Leaf(_) => Search::Missing,
            Self::Split {
                axis,
                ratio,
                children,
            } => {
                let (effective, ra, rb) = Self::child_rects(*axis, *ratio, rect, gap);
                let [a, b] = children.as_mut();
                let (result, first) = match a.resize(id, ra, gap, along, delta) {
                    Search::Missing => (b.resize(id, rb, gap, along, delta), false),
                    found => (found, true),
                };
                match result {
                    Search::Found if effective == along => {
                        let change = ratio_of(delta, rect.length(along));
                        let signed = if first { change } else { -change };
                        *ratio = clamp_ratio(*ratio + signed);
                        Search::Handled
                    }
                    other => other,
                }
            }
        }
    }
}

impl Dwindle {
    /// Gives every split the same ratio.
    pub fn set_ratios(&mut self, ratio: f32) {
        if let Some(root) = &mut self.root {
            root.set_ratios(ratio);
        }
    }

    /// Flips the direction of the split `id` is part of. `area` is the tiling area.
    pub fn toggle_split(&mut self, id: WindowId, area: Rect, s: &LayoutSettings) {
        if let Some(root) = &mut self.root {
            root.toggle_split(id, area, s.gaps_in);
        }
    }

    /// Swaps `id` with the main window (the first one, top-left), or the main window with the
    /// next one, like `swapwithmaster` in the master layout.
    pub fn swap_with_main(&mut self, id: WindowId) {
        let target = match self.windows().as_slice() {
            [main, next, ..] if *main == id => *next,
            [main, ..] => *main,
            [] => return,
        };
        self.swap(id, target);
    }
}

impl Layout for Dwindle {
    fn insert(&mut self, id: WindowId, focused: Option<WindowId>, _area: Rect, s: &LayoutSettings) {
        let Some(root) = &mut self.root else {
            self.root = Some(Node::Leaf(id));
            return;
        };
        let target = match focused {
            Some(f) if root.find_leaf_mut(f).is_some() => root.find_leaf_mut(f),
            _ => None,
        };
        let target = match target {
            Some(t) => t,
            None => root.last_leaf_mut(),
        };
        let old = std::mem::replace(target, Node::Leaf(id));
        *target = Node::Split {
            axis: None,
            ratio: s.dwindle_split_ratio,
            children: Box::new([old, Node::Leaf(id)]),
        };
    }

    fn remove(&mut self, id: WindowId) {
        self.root = self.root.take().and_then(|r| r.without(id));
    }

    fn windows(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.leaves(&mut out);
        }
        out
    }

    fn arrange(&mut self, area: Rect, _focused: Option<WindowId>, s: &LayoutSettings) -> Vec<Tile> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.arrange(area, s.gaps_in, &mut out);
        }
        out
    }

    fn swap(&mut self, a: WindowId, b: WindowId) {
        let Some(root) = &mut self.root else { return };
        if root.find_leaf_mut(a).is_none() || root.find_leaf_mut(b).is_none() {
            return;
        }
        root.for_each_leaf_mut(&mut |leaf| {
            if *leaf == a {
                *leaf = b;
            } else if *leaf == b {
                *leaf = a;
            }
        });
    }

    fn resize(&mut self, id: WindowId, dx: i32, dy: i32, area: Rect, s: &LayoutSettings) {
        let Some(root) = &mut self.root else { return };
        if dx != 0 {
            root.resize(id, area, s.gaps_in, Axis::Horizontal, dx);
        }
        if dy != 0 {
            root.resize(id, area, s.gaps_in, Axis::Vertical, dy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Direction;

    const AREA: Rect = Rect::new(0, 0, 1920, 1080);

    fn settings() -> LayoutSettings {
        LayoutSettings {
            gaps_in: 0,
            gaps_out: 0,
            ..LayoutSettings::default()
        }
    }

    fn rect_of(tiles: &[Tile], id: i64) -> Rect {
        tiles
            .iter()
            .find(|t| t.id == WindowId(id as isize))
            .unwrap()
            .rect
    }

    #[test]
    fn dwindles_by_aspect_ratio() {
        let s = settings();
        let mut d = Dwindle::default();
        d.insert(WindowId(1), None, AREA, &s);
        d.insert(WindowId(2), Some(WindowId(1)), AREA, &s);
        d.insert(WindowId(3), Some(WindowId(2)), AREA, &s);
        let tiles = d.arrange(AREA, None, &s);
        assert_eq!(rect_of(&tiles, 1), Rect::new(0, 0, 960, 1080));
        assert_eq!(rect_of(&tiles, 2), Rect::new(960, 0, 960, 540));
        assert_eq!(rect_of(&tiles, 3), Rect::new(960, 540, 960, 540));
    }

    #[test]
    fn remove_collapses_parent() {
        let s = settings();
        let mut d = Dwindle::default();
        for (id, f) in [(1, None), (2, Some(1)), (3, Some(2))] {
            d.insert(WindowId(id), f.map(WindowId), AREA, &s);
        }
        d.remove(WindowId(2));
        let tiles = d.arrange(AREA, None, &s);
        assert_eq!(rect_of(&tiles, 3), Rect::new(960, 0, 960, 1080));
        d.remove(WindowId(1));
        d.remove(WindowId(3));
        assert!(d.windows().is_empty());
    }

    #[test]
    fn resize_moves_shared_edge() {
        let s = settings();
        let mut d = Dwindle::default();
        d.insert(WindowId(1), None, AREA, &s);
        d.insert(WindowId(2), Some(WindowId(1)), AREA, &s);
        d.resize(WindowId(2), 192, 0, AREA, &s);
        let tiles = d.arrange(AREA, None, &s);
        assert_eq!(rect_of(&tiles, 2).w, 1152);
    }

    #[test]
    fn swap_with_main_toggles_with_the_first_window() {
        let s = settings();
        let mut d = Dwindle::default();
        for (id, f) in [(1, None), (2, Some(1)), (3, Some(2))] {
            d.insert(WindowId(id), f.map(WindowId), AREA, &s);
        }
        d.swap_with_main(WindowId(3));
        assert_eq!(d.windows(), [3, 2, 1].map(WindowId));
        d.swap_with_main(WindowId(3));
        assert_eq!(d.windows(), [2, 3, 1].map(WindowId));
    }

    #[test]
    fn toggle_split_and_swap() {
        let s = settings();
        let mut d = Dwindle::default();
        d.insert(WindowId(1), None, AREA, &s);
        d.insert(WindowId(2), Some(WindowId(1)), AREA, &s);
        d.toggle_split(WindowId(2), AREA, &s);
        let tiles = d.arrange(AREA, None, &s);
        assert_eq!(rect_of(&tiles, 1), Rect::new(0, 0, 1920, 540));
        assert!(d.move_window(WindowId(1), Direction::Down, &tiles));
        let tiles = d.arrange(AREA, None, &s);
        assert_eq!(rect_of(&tiles, 2), Rect::new(0, 0, 1920, 540));
    }
}
