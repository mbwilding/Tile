use crate::geometry::Rect;

/// A CSS-style cubic bezier easing curve through (0,0), (x1,y1), (x2,y2), (1,1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bezier {
    /// The first control point's x.
    pub x1: f32,
    /// The first control point's y.
    pub y1: f32,
    /// The second control point's x.
    pub x2: f32,
    /// The second control point's y.
    pub y2: f32,
}

impl Bezier {
    /// The curve with control points (x1,y1) and (x2,y2).
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn sample(a1: f32, a2: f32, u: f32) -> f32 {
        let inv = 1.0 - u;
        3.0 * inv * inv * u * a1 + 3.0 * inv * u * u * a2 + u * u * u
    }

    fn slope(a1: f32, a2: f32, u: f32) -> f32 {
        let inv = 1.0 - u;
        3.0 * inv * inv * a1 + 6.0 * inv * u * (a2 - a1) + 3.0 * u * u * (1.0 - a2)
    }

    /// Maps linear progress `t` (0..=1) to eased progress.
    pub fn ease(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        // Newton-Raphson to find the curve parameter whose x equals t, bisection as fallback.
        let mut u = t;
        for _ in 0..8 {
            let x = Self::sample(self.x1, self.x2, u) - t;
            if x.abs() < 1e-5 {
                return Self::sample(self.y1, self.y2, u);
            }
            let d = Self::slope(self.x1, self.x2, u);
            if d.abs() < 1e-6 {
                break;
            }
            u -= x / d;
        }
        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
        u = t;
        for _ in 0..32 {
            let x = Self::sample(self.x1, self.x2, u);
            if (x - t).abs() < 1e-5 {
                break;
            }
            if x < t {
                lo = u;
            } else {
                hi = u;
            }
            u = (lo + hi) / 2.0;
        }
        Self::sample(self.y1, self.y2, u)
    }
}

/// Interpolates between two rectangles; `t` may overshoot 1.0 for springy curves.
pub fn lerp_rect(from: Rect, to: Rect, t: f32) -> Rect {
    let mix = |a: i32, b: i32| a + crate::geometry::scale(b - a, t);
    Rect::new(
        mix(from.x, to.x),
        mix(from.y, to.y),
        mix(from.w, to.w),
        mix(from.h, to.h),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_curve_is_identity() {
        let b = Bezier::new(0.0, 0.0, 1.0, 1.0);
        for i in 0..=10 {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 / 10.0;
            assert!((b.ease(t) - t).abs() < 1e-3);
        }
    }

    #[test]
    fn ease_out_is_ahead_of_linear() {
        let b = Bezier::new(0.05, 0.9, 0.1, 1.0);
        assert!(b.ease(0.3) > 0.5);
        assert!((b.ease(1.0) - 1.0).abs() < 1e-4);
        assert!(b.ease(0.0).abs() < 1e-4);
    }

    #[test]
    fn lerp_midpoint() {
        let r = lerp_rect(
            Rect::new(0, 0, 100, 100),
            Rect::new(100, 100, 200, 200),
            0.5,
        );
        assert_eq!(r, Rect::new(50, 50, 150, 150));
    }
}
