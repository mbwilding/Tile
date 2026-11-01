use embed_manifest::{embed_manifest, manifest::DpiAwareness, new_manifest};

#[path = "../tile-platform/src/tile_icon.rs"]
mod tile_icon;

/// Icon sizes embedded in the executable.
const SIZES: [u32; 3] = [32, 64, 256];
/// Resource id of the icon group; the lowest id is the one Explorer shows.
const GROUP_ID: u16 = 1;

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Per-monitor DPI awareness, so window coordinates are physical pixels everywhere.
        embed_manifest(new_manifest("Tile").dpi_awareness(DpiAwareness::PerMonitorV2))
            .expect("unable to embed manifest");

        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let res = out.join("tile-icon.res");
        std::fs::write(&res, icon_resource()).expect("unable to write the icon resource");
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
    // CI sets TILE_VERSION to the release version; local builds use the crate's.
    let version = std::env::var("TILE_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION"));
    println!("cargo:rustc-env=TILE_VERSION={version}");
    println!("cargo:rerun-if-env-changed=TILE_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../tile-platform/src/tile_icon.rs");
}

/// A compiled `.res` file holding the icon, which the MSVC linker accepts like an object file.
fn icon_resource() -> Vec<u8> {
    let mut res = Vec::new();
    // The empty first entry marks the file as 32-bit.
    record(&mut res, 0, 0, &[]);

    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    for (i, size) in SIZES.into_iter().enumerate() {
        let image = dib(size);
        let id = GROUP_ID + 1 + i as u16;
        record(&mut res, 3, id, &image);

        // 256 is stored as 0 in the one-byte dimensions.
        let dim = if size >= 256 { 0 } else { size as u8 };
        group.extend_from_slice(&[dim, dim, 0, 0]);
        group.extend_from_slice(&1u16.to_le_bytes());
        group.extend_from_slice(&32u16.to_le_bytes());
        group.extend_from_slice(&(image.len() as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    record(&mut res, 14, GROUP_ID, &group);
    res
}

/// An icon image in DIB form: 32-bit bottom-up BGRA, then the (all clear) AND mask.
fn dib(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(size * 2).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);

    let rgba = tile_icon::rgba(size, false);
    for row in rgba.chunks_exact(size as usize * 4).rev() {
        for &[r, g, b, a] in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    let mask_row = size.div_ceil(32) * 4;
    out.resize(out.len() + (mask_row * size) as usize, 0);
    out
}

/// Appends one resource of numeric `kind` and `id`.
fn record(res: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    res.extend_from_slice(&(data.len() as u32).to_le_bytes());
    res.extend_from_slice(&32u32.to_le_bytes());
    for v in [0xffffu16, kind, 0xffff, id] {
        res.extend_from_slice(&v.to_le_bytes());
    }
    res.extend_from_slice(&0u32.to_le_bytes());
    // MOVEABLE | PURE | DISCARDABLE, language neutral (the empty marker keeps all zero).
    let flags: u16 = if kind == 0 { 0 } else { 0x1030 };
    res.extend_from_slice(&flags.to_le_bytes());
    res.extend_from_slice(&[0; 10]);
    res.extend_from_slice(data);
    res.resize(res.len().next_multiple_of(4), 0);
}
