//! Decoding a wallpaper with the Windows Imaging Component, which reads what Windows can: PNG,
//! JPEG, BMP, GIF, TIFF and, with its codecs installed, WebP and HEIC.

use std::path::Path;

use windows::{
    Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource,
            IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
            WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    },
    core::{HSTRING, Interface},
};

use super::fit::{self, Fill, Surface};
use crate::win::context;

/// The imaging factory, made on the thread that uses it.
pub(super) fn factory() -> Result<IWICImagingFactory, String> {
    // SAFETY: a standard in-process COM class; the thread has initialised COM.
    context(
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) },
        "no imaging factory",
    )
}

/// An opened image, read as BGRA.
pub(super) struct Picture {
    source: IWICBitmapSource,
    width: u32,
    height: u32,
}

impl Picture {
    /// Opens the image at `path`.
    pub(super) fn open(factory: &IWICImagingFactory, path: &Path) -> Result<Self, String> {
        let name = HSTRING::from(path.as_os_str());
        // SAFETY: plain COM calls on interfaces this function owns.
        let (source, width, height) = unsafe {
            let decoder = context(
                factory.CreateDecoderFromFilename(
                    &name,
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                ),
                "can't read the image",
            )?;
            let frame = context(decoder.GetFrame(0), "no image in the file")?;
            let converter = context(factory.CreateFormatConverter(), "no converter")?;
            context(
                converter.Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                ),
                "can't convert the image",
            )?;
            let (mut w, mut h) = (0, 0);
            context(converter.GetSize(&mut w, &mut h), "no image size")?;
            let source: IWICBitmapSource = context(converter.cast(), "no image source")?;
            (source, w, h)
        };
        if width == 0 || height == 0 {
            return Err("the image is empty".into());
        }
        Ok(Self {
            source,
            width,
            height,
        })
    }

    /// The picture on a `width` by `height` monitor as BGRA rows, top first, with `color` (BGR)
    /// wherever it doesn't reach.
    pub(super) fn render(
        &self,
        factory: &IWICImagingFactory,
        fill: Fill,
        color: [u8; 3],
        (width, height): (u32, u32),
    ) -> Result<Vec<u8>, String> {
        let mut out = Surface::filled(width, height, [color[0], color[1], color[2], 255]);
        let place = fit::place(fill, (self.width, self.height), (width, height));
        let picture = Surface {
            w: place.w,
            h: place.h,
            pixels: self.read(factory, (place.w, place.h))?,
        };
        if fill == Fill::Tile {
            out.tile(&picture);
        } else {
            out.blit(&picture, place.x, place.y);
        }
        Ok(out.pixels)
    }

    /// The picture's pixels at `w` by `h`: itself when that is its own size, else scaled.
    fn read(&self, factory: &IWICImagingFactory, (w, h): (u32, u32)) -> Result<Vec<u8>, String> {
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        // SAFETY: plain COM calls; `pixels` has the room for `h` rows of `w` pixels that
        // `CopyPixels` is told of.
        unsafe {
            if (w, h) == (self.width, self.height) {
                context(
                    self.source.CopyPixels(std::ptr::null(), w * 4, &mut pixels),
                    "can't read the image",
                )?;
            } else {
                let scaler = context(factory.CreateBitmapScaler(), "no scaler")?;
                context(
                    scaler.Initialize(&self.source, w, h, WICBitmapInterpolationModeFant),
                    "can't scale the image",
                )?;
                context(
                    scaler.CopyPixels(std::ptr::null(), w * 4, &mut pixels),
                    "can't read the image",
                )?;
            }
        }
        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    use super::*;

    /// A 1 by 1 red 24-bit BMP.
    fn red_bmp() -> Vec<u8> {
        let mut bmp = b"BM".to_vec();
        bmp.extend(58u32.to_le_bytes());
        bmp.extend([0; 4]);
        bmp.extend(54u32.to_le_bytes());
        bmp.extend(40u32.to_le_bytes());
        bmp.extend(1i32.to_le_bytes());
        bmp.extend(1i32.to_le_bytes());
        bmp.extend(1u16.to_le_bytes());
        bmp.extend(24u16.to_le_bytes());
        bmp.extend([0; 24]);
        // One pixel (blue, green, red) padded to a 4-byte row.
        bmp.extend([0, 0, 255, 0]);
        bmp
    }

    #[test]
    fn reads_and_fits_a_picture() {
        let path = std::env::temp_dir().join(format!("tile-picture-{}.bmp", std::process::id()));
        std::fs::write(&path, red_bmp()).unwrap();
        // SAFETY: initialises COM for this test's thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let factory = factory().expect("a factory");
        let picture = Picture::open(&factory, &path).expect("opens");
        // Fit a square picture into a wide monitor: red in the middle, blue around it.
        let pixels = picture
            .render(&factory, Fill::Fit, [255, 0, 0], (4, 2))
            .expect("renders");
        drop(picture);
        std::fs::remove_file(&path).unwrap();
        let pixel = |x: usize, y: usize| &pixels[(y * 4 + x) * 4..][..4];
        assert_eq!(pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(1, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(2, 1), [0, 0, 255, 255]);
        assert_eq!(pixel(3, 1), [255, 0, 0, 255]);
    }
}
