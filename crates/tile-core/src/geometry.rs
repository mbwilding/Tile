use serde::{Deserialize, Serialize};

/// An axis-aligned rectangle in physical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn from_ltrb(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self::new(left, top, right - left, bottom - top)
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub const fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    pub const fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Shrinks the rectangle by `amount` on every side, never going below 1px.
    pub fn shrink(&self, amount: i32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.w - amount * 2).max(1),
            (self.h - amount * 2).max(1),
        )
    }

    /// The width for [`Axis::Horizontal`], the height for [`Axis::Vertical`].
    pub const fn length(&self, axis: Axis) -> i32 {
        match axis {
            Axis::Horizontal => self.w,
            Axis::Vertical => self.h,
        }
    }

    /// Splits along `axis`, giving the first part `ratio` of the space left after the gap.
    pub fn split(&self, axis: Axis, ratio: f32, gap: i32) -> (Self, Self) {
        match self.split_weighted(axis, &[ratio, 1.0 - ratio], gap)[..] {
            [a, b] => (a, b),
            _ => unreachable!("two weights make two parts"),
        }
    }

    /// Splits along `axis` into `weights.len()` parts proportional to the weights.
    pub fn split_weighted(&self, axis: Axis, weights: &[f32], gap: i32) -> Vec<Self> {
        if weights.is_empty() {
            return Vec::new();
        }
        let total: f32 = weights.iter().sum();
        let count = i32::try_from(weights.len()).unwrap_or(i32::MAX);
        let avail = self.length(axis) - gap * (count - 1);
        let mut out = Vec::with_capacity(weights.len());
        let mut offset = 0;
        let mut used = 0;
        for (i, weight) in weights.iter().enumerate() {
            let size = if i + 1 == weights.len() {
                avail - used
            } else {
                scale(avail, weight / total)
            };
            out.push(match axis {
                Axis::Horizontal => Self::new(self.x + offset, self.y, size, self.h),
                Axis::Vertical => Self::new(self.x, self.y + offset, self.w, size),
            });
            used += size;
            offset += size + gap;
        }
        out
    }
}

/// Multiplies a pixel length by a ratio, rounding to the nearest pixel.
pub fn scale(length: i32, ratio: f32) -> i32 {
    // Pixel values comfortably fit in f32's exact integer range.
    (length as f32 * ratio).round() as i32
}

/// Converts a pixel delta into a ratio delta relative to `length`.
pub fn ratio_of(delta: i32, length: i32) -> f32 {
    if length == 0 {
        return 0.0;
    }
    delta as f32 / length as f32
}

/// The orientation of a split. `Horizontal` places children side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// The natural split axis for a rectangle: split the longer side.
    pub const fn for_rect(rect: &Rect) -> Self {
        if rect.w >= rect.h {
            Self::Horizontal
        } else {
            Self::Vertical
        }
    }

    pub const fn flip(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Up | Self::Down => Axis::Vertical,
        }
    }

    /// Whether moving in this direction goes towards lower coordinates.
    pub const fn is_backward(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }
}

/// Finds the rectangle nearest to `from` in `dir`: candidates must lie
/// beyond the source edge, overlap on the perpendicular axis is preferred, then distance.
pub fn nearest_in_direction<T: Copy>(
    from: &Rect,
    dir: Direction,
    candidates: impl IntoIterator<Item = (T, Rect)>,
) -> Option<T> {
    let (fx, fy) = from.center();
    candidates
        .into_iter()
        .filter_map(|(id, rect)| {
            let (cx, cy) = rect.center();
            let beyond = match dir {
                Direction::Left => rect.right() <= from.x + 1 || cx < from.x,
                Direction::Right => rect.x >= from.right() - 1 || cx > from.right(),
                Direction::Up => rect.bottom() <= from.y + 1 || cy < from.y,
                Direction::Down => rect.y >= from.bottom() - 1 || cy > from.bottom(),
            };
            if !beyond {
                return None;
            }
            let overlap = match dir.axis() {
                Axis::Horizontal => rect.y.max(from.y) < rect.bottom().min(from.bottom()),
                Axis::Vertical => rect.x.max(from.x) < rect.right().min(from.right()),
            };
            let (primary, secondary) = match dir.axis() {
                Axis::Horizontal => ((cx - fx).abs(), (cy - fy).abs()),
                Axis::Vertical => ((cy - fy).abs(), (cx - fx).abs()),
            };
            // Overlapping candidates always win over non-overlapping ones.
            let score = (
                i64::from(!overlap),
                i64::from(primary),
                i64::from(secondary),
            );
            Some((score, id))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_horizontal_with_gap() {
        let (a, b) = Rect::new(0, 0, 1010, 500).split(Axis::Horizontal, 0.5, 10);
        assert_eq!(a, Rect::new(0, 0, 500, 500));
        assert_eq!(b, Rect::new(510, 0, 500, 500));
    }

    #[test]
    fn split_weighted_fills_exactly() {
        let parts = Rect::new(0, 0, 100, 302).split_weighted(Axis::Vertical, &[1.0, 1.0, 1.0], 1);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2].bottom(), 302);
        assert_eq!(parts[1].y, parts[0].bottom() + 1);
    }

    #[test]
    fn nearest_prefers_overlap() {
        let from = Rect::new(0, 0, 100, 100);
        let candidates = [
            (1, Rect::new(110, 500, 100, 100)),
            (2, Rect::new(300, 0, 100, 100)),
        ];
        assert_eq!(
            nearest_in_direction(&from, Direction::Right, candidates),
            Some(2)
        );
        assert_eq!(
            nearest_in_direction(&from, Direction::Left, candidates),
            None
        );
    }
}
