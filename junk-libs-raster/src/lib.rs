//! Small GUI-agnostic raster helpers shared by the egui docview widget and any
//! backend that bakes content onto a page bitmap. Depends only on `image`, so it
//! can be used by both the egui side (the live overlay preview) and pure-backend
//! export code without dragging egui into the latter.

use image::{Rgba, RgbaImage};

/// An integer pixel rectangle into a bitmap. Coordinates may fall outside the
/// image; helpers clamp as needed.
#[derive(Clone, Copy)]
pub struct PixelRect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// Average color of a 1px ring just outside `rect` (clamped to the image), used
/// to fill a region's background so an overlay blends with the surrounding page.
/// Returns opaque white when the ring samples nothing (e.g. a region covering the
/// whole image).
pub fn sample_background(img: &RgbaImage, rect: PixelRect) -> Rgba<u8> {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    let mut sample = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && x < iw && y < ih {
            let p = img.get_pixel(x as u32, y as u32);
            r += p[0] as u64;
            g += p[1] as u64;
            b += p[2] as u64;
            n += 1;
        }
    };
    for x in rect.x..rect.x + rect.w {
        sample(x, rect.y - 1);
        sample(x, rect.y + rect.h);
    }
    for y in rect.y..rect.y + rect.h {
        sample(rect.x - 1, y);
        sample(rect.x + rect.w, y);
    }
    if n == 0 {
        Rgba([255, 255, 255, 255])
    } else {
        Rgba([(r / n) as u8, (g / n) as u8, (b / n) as u8, 255])
    }
}
