//! The Tile icon, drawn in code so the tray and the executable's icon share one source.
//! `crates/tile/build.rs` includes this file directly.

/// Samples per pixel along each axis, for smooth edges.
const SAMPLES: u32 = 4;
/// Corner radius of a tile, in 32nds of the icon.
const RADIUS: f32 = 3.0;

/// Whether the point, relative to a tile's corner, is inside the rounded square of `side`.
fn inside(px: f32, py: f32, side: f32) -> bool {
    let dx = (RADIUS - px).max(px - (side - RADIUS)).max(0.0);
    let dy = (RADIUS - py).max(py - (side - RADIUS)).max(0.0);
    px >= 0.0 && px <= side && py >= 0.0 && py <= side && dx * dx + dy * dy <= RADIUS * RADIUS
}

/// RGBA pixels of a `size` x `size` icon: four rounded tiles, the top left one in the accent
/// colour, or red when `error` is set.
pub fn rgba(size: u32, error: bool) -> Vec<u8> {
    let primary = if error {
        [0xf3, 0x8b, 0xa8]
    } else {
        [0x89, 0xb4, 0xfa]
    };
    let rest = [0xcd, 0xd6, 0xf4];
    let scale = 32.0 / (size * SAMPLES) as f32;

    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Coverage of each tile, indexed by (right, bottom).
            let mut hits = [0u32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = ((x * SAMPLES + sx) as f32 + 0.5) * scale;
                    let py = ((y * SAMPLES + sy) as f32 + 0.5) * scale;
                    for (i, hit) in hits.iter_mut().enumerate() {
                        let lo_x = if i & 1 == 0 { 2.0 } else { 17.0 };
                        let lo_y = if i & 2 == 0 { 2.0 } else { 17.0 };
                        if inside(px - lo_x, py - lo_y, 13.0) {
                            *hit += 1;
                        }
                    }
                }
            }
            // The tiles are 2 apart, so a pixel touches at most one.
            let (tile, count) =
                hits.iter().enumerate().fold(
                    (0, 0),
                    |best, (i, &c)| if c > best.1 { (i, c) } else { best },
                );
            let rgb = if tile == 0 { primary } else { rest };
            let alpha = (count * 255 + SAMPLES * SAMPLES / 2) / (SAMPLES * SAMPLES);
            out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], alpha as u8]);
        }
    }
    out
}
