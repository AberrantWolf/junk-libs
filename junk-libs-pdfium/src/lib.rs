//! GUI-agnostic PDFium render core: open a PDF, rasterize pages to RGBA, and
//! extract the text layer as per-character boxes.
//!
//! Shared between the print-junk and expat-junk tools. Nothing here depends on a
//! UI toolkit. Binding is dynamic at runtime: this crate's `build.rs` downloads
//! `libpdfium` into `OUT_DIR` and exports its directory as `PDFIUM_LIB_DIR`,
//! which [`instance`] loads via `option_env!`, falling back to a system library.
//!
//! The Cargo `pdfium_NNNN` feature and `build.rs`'s `PDFIUM_VERSION` must name the
//! same PDFium build, or binding fails with `LoadLibraryError: undefined symbol`.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use pdfium_render::prelude::*;

/// Cap any single render dimension to avoid OOM at extreme zoom.
const MAX_RENDER_DIMENSION: i32 = 4096;

/// Bind PDFium: prefer the build-vendored library, fall back to the system one.
fn bind() -> Result<Pdfium> {
    if let Some(dir) = option_env!("PDFIUM_LIB_DIR")
        && let Ok(bindings) =
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir))
    {
        return Ok(Pdfium::new(bindings));
    }
    Pdfium::bind_to_system_library()
        .map(Pdfium::new)
        .context("failed to bind to a vendored or system PDFium library")
}

/// The process-wide PDFium instance.
///
/// PDFium may only be initialized **once per process** (a second bind fails with
/// `PdfiumLibraryBindingsAlreadyInitialized`), so we bind lazily and share one
/// handle. Safe to call repeatedly — e.g. opening multiple documents, or from
/// several tests. The `thread_safe` crate feature serializes the underlying API,
/// so the shared `&'static` reference is fine to use across threads.
pub fn instance() -> Result<&'static Pdfium> {
    // Cache the error too, so a missing library doesn't re-attempt binding
    // (and re-trip the already-initialized guard) on every call.
    static INSTANCE: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| bind().map_err(|e| format!("{e:#}")))
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))
}

/// Quantize a zoom fraction to a discrete percentage, for render-cache keys.
/// Steps: every 25% from 25–100, every 50% from 100–400.
pub fn quantize_zoom(zoom: f32) -> u32 {
    let percent = (zoom * 100.0).round() as i32;
    let clamped = percent.clamp(25, 400);
    if clamped <= 100 {
        ((clamped + 12) / 25 * 25) as u32
    } else {
        ((clamped + 25) / 50 * 50) as u32
    }
}

/// Render target dimensions (in pixels) for a page of the given point size at `zoom`.
fn render_dimensions(width_pts: f32, height_pts: f32, zoom: f32) -> (i32, i32) {
    let w = (width_pts * zoom).round() as i32;
    let h = (height_pts * zoom).round() as i32;
    (w.min(MAX_RENDER_DIMENSION), h.min(MAX_RENDER_DIMENSION))
}

fn make_render_config(width_pts: f32, height_pts: f32, zoom: f32) -> PdfRenderConfig {
    let (w, h) = render_dimensions(width_pts, height_pts, zoom);
    PdfRenderConfig::new()
        .set_target_width(w)
        .set_maximum_height(h)
}

/// A character from the PDF text layer with its bounding box.
///
/// Bounds are in page points with a **top-left origin (y grows downward)** — the
/// same space as the rasterized [`RenderedPage`], so boxes overlay the page image
/// directly. PDFium reports text in PDF-native bottom-left coordinates;
/// [`render_pdf`] flips them here once.
pub struct CharBox {
    pub ch: char,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One rasterized page plus its native size and text layer.
pub struct RenderedPage {
    pub image: image::RgbaImage,
    /// Native page size in PDF points (origin-independent, used for overlays).
    pub size_pts: (f32, f32),
    /// Per-character boxes for snap-to-text selection and the native text layer.
    /// Empty for pages (or documents) with no extractable text.
    pub char_boxes: Vec<CharBox>,
}

/// Load a PDF and render every page to RGBA at the given zoom (1.0 = 100%).
pub fn render_pdf(pdfium: &Pdfium, path: &Path, zoom: f32) -> Result<Vec<RenderedPage>> {
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("loading PDF {}", path.display()))?;

    let mut pages = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let width_pts = page.width().value;
        let height_pts = page.height().value;
        let config = make_render_config(width_pts, height_pts, zoom);
        let bitmap = page
            .render_with_config(&config)
            .with_context(|| format!("rendering page {index}"))?;
        let image = bitmap
            .as_image()
            .with_context(|| format!("converting page {index} bitmap"))?
            .into_rgba8();
        pages.push(RenderedPage {
            image,
            size_pts: (width_pts, height_pts),
            char_boxes: extract_char_boxes(&page, height_pts),
        });
    }
    Ok(pages)
}

/// Render a single page to RGBA at the given scale (pixels per point) — bitmap
/// only, no text extraction. For re-rendering a page at a higher resolution when
/// the viewer zooms in. Returns the image and the page's native size in points.
pub fn render_page_bitmap(
    pdfium: &Pdfium,
    path: &Path,
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("loading PDF {}", path.display()))?;
    let page = document
        .pages()
        .get(page_index as i32)
        .with_context(|| format!("page {page_index} out of range"))?;
    let width_pts = page.width().value;
    let height_pts = page.height().value;
    let config = make_render_config(width_pts, height_pts, scale);
    let bitmap = page
        .render_with_config(&config)
        .with_context(|| format!("rendering page {page_index}"))?;
    let image = bitmap
        .as_image()
        .with_context(|| format!("converting page {page_index} bitmap"))?
        .into_rgba8();
    Ok((image, (width_pts, height_pts)))
}

/// Extract the text layer of `page` as character boxes in top-left page space.
/// Returns empty if the page has no text. Uses *loose* glyph bounds (the full
/// character cell, including side bearing) so adjacent chars tile cleanly into
/// blocks for selection.
fn extract_char_boxes(page: &PdfPage, page_height_pts: f32) -> Vec<CharBox> {
    let Ok(text) = page.text() else {
        return Vec::new();
    };
    let mut boxes = Vec::new();
    for ch in text.chars().iter() {
        let Some(c) = ch.unicode_char() else { continue };
        let Ok(bounds) = ch.loose_bounds() else {
            continue;
        };
        let left = bounds.left().value;
        let top = bounds.top().value;
        boxes.push(CharBox {
            ch: c,
            x: left,
            // Flip PDF's bottom-left origin to our top-left origin.
            y: page_height_pts - top,
            w: bounds.right().value - left,
            h: top - bounds.bottom().value,
        });
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end proof the native pipeline works: bind the vendored PDFium,
    /// generate a one-page A4 PDF with it, then render that PDF back to RGBA.
    /// No display required, so this runs in CI.
    #[test]
    fn renders_a_generated_pdf() {
        let pdfium = instance().expect("bind PDFium");

        let path = std::env::temp_dir().join("junk-libs-pdfium-render-test.pdf");
        {
            let mut document = pdfium.create_new_pdf().expect("create pdf");
            document
                .pages_mut()
                .create_page_at_start(PdfPagePaperSize::a4())
                .expect("create page");
            document.save_to_file(&path).expect("save pdf");
        }

        let pages = render_pdf(pdfium, &path, 1.0).expect("render pdf");
        let _ = std::fs::remove_file(&path);

        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        // A4 = 210×297mm = 595×842pt; at zoom 1.0 the raster matches in pixels.
        assert!(
            (page.size_pts.0 - 595.0).abs() < 5.0 && (page.size_pts.1 - 842.0).abs() < 5.0,
            "unexpected page size in points: {:?}",
            page.size_pts
        );
        assert!(
            (570..=620).contains(&page.image.width())
                && (820..=870).contains(&page.image.height()),
            "unexpected raster size: {}×{}",
            page.image.width(),
            page.image.height()
        );
    }

    /// Place known text on a page, then verify we extract it with boxes in the
    /// expected top-left page-point location.
    #[test]
    fn extracts_text_layer() {
        let pdfium = instance().expect("bind PDFium");

        let path = std::env::temp_dir().join("junk-libs-pdfium-text-test.pdf");
        {
            let mut document = pdfium.create_new_pdf().expect("create pdf");
            let font = document.fonts_mut().helvetica();
            {
                let mut page = document
                    .pages_mut()
                    .create_page_at_start(PdfPagePaperSize::a4())
                    .expect("create page");
                // Baseline at (72, 720) in PDF bottom-left points.
                page.objects_mut()
                    .create_text_object(
                        PdfPoints::new(72.0),
                        PdfPoints::new(720.0),
                        "Hello",
                        font,
                        PdfPoints::new(24.0),
                    )
                    .expect("add text object");
            }
            document.save_to_file(&path).expect("save pdf");
        }

        let pages = render_pdf(pdfium, &path, 1.0).expect("render pdf");
        let _ = std::fs::remove_file(&path);

        let boxes = &pages[0].char_boxes;
        let text: String = boxes.iter().map(|b| b.ch).collect();
        assert!(text.contains("Hello"), "extracted text layer was {text:?}");

        // 'H' should sit at x≈72pt from the left and, flipped into top-left
        // space, y ≈ 842 − 720 ≈ 122pt down from the top.
        let h = boxes.iter().find(|b| b.ch == 'H').expect("found 'H' box");
        assert!((55.0..95.0).contains(&h.x), "H.x = {}", h.x);
        assert!((90.0..150.0).contains(&h.y), "H.y = {}", h.y);
        assert!(h.w > 0.0 && h.h > 0.0, "degenerate H box {}×{}", h.w, h.h);
    }
}
