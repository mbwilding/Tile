//! The Tile icon, drawn in code so the tray and the executable's icon share one source.
//! `crates/tile/build.rs` includes this file directly.

/// RGBA pixels of a `size` x `size` icon (a multiple of 32): four rounded tiles, the top left one
/// in the accent colour, or red when `error` is set.
pub fn rgba(size: u32, error: bool) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (x, y) = (x * 32 / size, y * 32 / size);
            let in_tile = |v: u32| (2..15).contains(&v) || (17..30).contains(&v);
            let corner = |v: u32| matches!(v, 2 | 14 | 17 | 29);
            let filled = in_tile(x) && in_tile(y) && !(corner(x) && corner(y));
            let primary = x < 16 && y < 16;
            let pixel = match (filled, primary) {
                (false, _) => [0, 0, 0, 0],
                (true, true) if error => [0xf3, 0x8b, 0xa8, 0xff],
                (true, true) => [0x89, 0xb4, 0xfa, 0xff],
                (true, false) => [0xcd, 0xd6, 0xf4, 0xff],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}
