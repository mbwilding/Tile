//! Fitting a picture to a monitor, in plain pixel arithmetic.

/// How a picture is fitted to a monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fill {
    /// Scaled to cover the monitor, the overflow cut off.
    #[default]
    Crop,
    /// Scaled to fit inside the monitor.
    Fit,
    /// Stretched to the monitor's shape.
    Stretch,
    /// Its own size, in the middle.
    Center,
    /// Its own size, repeated.
    Tile,
}

/// Where a picture lands on a monitor: its size and its top left corner, which is outside the
/// monitor where the picture is cropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub w: u32,
    pub h: u32,
    pub x: i32,
    pub y: i32,
}

/// Where a picture of `src` (width, height) lands on a monitor of `dst`. Every size is at least 1.
pub fn place(fill: Fill, src: (u32, u32), dst: (u32, u32)) -> Placement {
    let (sw, sh) = (u64::from(src.0.max(1)), u64::from(src.1.max(1)));
    let (dw, dh) = (u64::from(dst.0.max(1)), u64::from(dst.1.max(1)));
    // Compares dw/sw with dh/sh without dividing: the larger ratio is the one that covers.
    let width_ratio_larger = dw * sh >= dh * sw;
    let (w, h) = match fill {
        Fill::Stretch => (dw, dh),
        Fill::Center | Fill::Tile => (sw, sh),
        Fill::Crop if width_ratio_larger => (dw, (sh * dw).div_ceil(sw).max(dh)),
        Fill::Crop => ((sw * dh).div_ceil(sh).max(dw), dh),
        Fill::Fit if width_ratio_larger => ((sw * dh / sh).max(1), dh),
        Fill::Fit => (dw, (sh * dw / sw).max(1)),
    };
    let centre = |outer: u64, inner: u64| ((outer as i64 - inner as i64) / 2) as i32;
    Placement {
        w: w as u32,
        h: h as u32,
        x: centre(dw, w),
        y: centre(dh, h),
    }
}

/// A `w` by `h` picture of 4-byte pixels, rows from the top.
pub struct Surface {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
}

impl Surface {
    /// A surface all of one pixel.
    pub fn filled(w: u32, h: u32, pixel: [u8; 4]) -> Self {
        Self {
            w,
            h,
            pixels: pixel.repeat(w as usize * h as usize),
        }
    }

    /// Copies `src` on with its corner at (`x`, `y`), clipped to this surface.
    pub fn blit(&mut self, src: &Surface, x: i32, y: i32) {
        let (dw, dh, sw, sh) = (
            i64::from(self.w),
            i64::from(self.h),
            i64::from(src.w),
            i64::from(src.h),
        );
        let (x, y) = (i64::from(x), i64::from(y));
        let (from_x, to_x) = (x.max(0), (x + sw).min(dw));
        let (from_y, to_y) = (y.max(0), (y + sh).min(dh));
        if from_x >= to_x || from_y >= to_y {
            return;
        }
        let bytes = ((to_x - from_x) * 4) as usize;
        for row in from_y..to_y {
            let s = (((row - y) * sw + (from_x - x)) * 4) as usize;
            let d = ((row * dw + from_x) * 4) as usize;
            self.pixels[d..d + bytes].copy_from_slice(&src.pixels[s..s + bytes]);
        }
    }

    /// Repeats `src` over all of this surface, from the top left.
    pub fn tile(&mut self, src: &Surface) {
        let (sw, sh) = (src.w.max(1) as usize, src.h.max(1) as usize);
        for y in (0..self.h as usize).step_by(sh) {
            for x in (0..self.w as usize).step_by(sw) {
                self.blit(src, x as i32, y as i32);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: (u32, u32) = (3840, 1080);
    const MONITOR: (u32, u32) = (1920, 1080);

    #[test]
    fn crop_covers_and_centres_the_overflow() {
        let p = place(Fill::Crop, WIDE, MONITOR);
        assert_eq!((p.w, p.h), (3840, 1080));
        assert_eq!((p.x, p.y), (-960, 0));
        let p = place(Fill::Crop, (1000, 2000), MONITOR);
        assert_eq!((p.w, p.h), (1920, 3840));
        assert_eq!((p.x, p.y), (0, -1380));
    }

    #[test]
    fn fit_stays_inside_and_centres() {
        let p = place(Fill::Fit, WIDE, MONITOR);
        assert_eq!((p.w, p.h), (1920, 540));
        assert_eq!((p.x, p.y), (0, 270));
    }

    #[test]
    fn stretch_fills_exactly() {
        let p = place(Fill::Stretch, (10, 3), MONITOR);
        assert_eq!((p.w, p.h, p.x, p.y), (1920, 1080, 0, 0));
    }

    #[test]
    fn centre_keeps_the_size() {
        let p = place(Fill::Center, (100, 50), (200, 100));
        assert_eq!((p.w, p.h, p.x, p.y), (100, 50, 50, 25));
    }

    #[test]
    fn blit_clips_to_the_destination() {
        let mut dst = Surface::filled(2, 2, [0; 4]);
        let src = Surface::filled(3, 3, [7; 4]);
        dst.blit(&src, -1, 1);
        // Only the destination's bottom row is covered.
        assert_eq!(&dst.pixels[..8], &[0; 8]);
        assert_eq!(&dst.pixels[8..], &[7; 8]);
        dst.blit(&src, 5, 5);
    }

    #[test]
    fn tile_repeats_from_the_corner() {
        let mut dst = Surface::filled(3, 2, [0; 4]);
        dst.tile(&Surface::filled(1, 1, [1; 4]));
        assert!(dst.pixels.iter().all(|&b| b == 1));
    }
}
