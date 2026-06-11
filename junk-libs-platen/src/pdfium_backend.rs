//! The legacy engine, delegating to `junk-libs-pdfium` (which manages the
//! PDFium binary, the once-per-process bind, and the global render lock
//! internally). Kept for A/B comparison during the hayro migration and as a
//! dev-time test oracle; not intended to ship once the cutover sticks.

use std::path::Path;

use crate::{CharBox, Error, RenderedPage, Result};

/// pdfium reports failures as `anyhow` strings; everything becomes
/// [`Error::Engine`]. Good enough for a backend whose job is comparison.
fn engine_err(e: anyhow::Error) -> Error {
    Error::Engine(format!("{e:#}"))
}

pub fn render_pdf(path: &Path, zoom: f32) -> Result<Vec<RenderedPage>> {
    let pdfium = junk_libs_pdfium::instance().map_err(engine_err)?;
    let rendered = junk_libs_pdfium::render_pdf(pdfium, path, zoom).map_err(engine_err)?;
    Ok(rendered
        .into_iter()
        .map(|p| RenderedPage {
            image: p.image,
            size_pts: p.size_pts,
            char_boxes: p
                .char_boxes
                .into_iter()
                .map(|c| CharBox {
                    ch: c.ch,
                    x: c.x,
                    y: c.y,
                    w: c.w,
                    h: c.h,
                })
                .collect(),
        })
        .collect())
}

pub fn render_page_bitmap(
    path: &Path,
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    let pdfium = junk_libs_pdfium::instance().map_err(engine_err)?;
    junk_libs_pdfium::render_page_bitmap(pdfium, path, page_index, scale).map_err(engine_err)
}

pub fn render_page_bitmap_from_bytes(
    bytes: &[u8],
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    let pdfium = junk_libs_pdfium::instance().map_err(engine_err)?;
    junk_libs_pdfium::render_page_bitmap_from_bytes(pdfium, bytes, page_index, scale)
        .map_err(engine_err)
}

pub fn page_count(path: &Path) -> Result<usize> {
    let pdfium = junk_libs_pdfium::instance().map_err(engine_err)?;
    junk_libs_pdfium::page_count(pdfium, path).map_err(engine_err)
}
