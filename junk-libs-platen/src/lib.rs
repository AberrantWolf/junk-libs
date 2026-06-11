//! Engine-agnostic PDF render core: open a PDF, rasterize pages to RGBA, and
//! extract the text layer as per-character boxes. The successor to
//! `junk-libs-pdfium`, with the engine behind a feature flag:
//!
//! - `backend-hayro` (default) — pure-Rust [hayro](https://github.com/LaurenzV/hayro).
//!   Nothing to download, nothing to ship next to the executable, and no
//!   process-global render lock: every call is self-contained, so pages render
//!   in parallel from multiple threads.
//! - `backend-pdfium` — delegates to `junk-libs-pdfium`. Kept for A/B render
//!   comparison during the migration and as a dev-time test oracle; goes away
//!   once the cutover sticks (see SPEC.md).
//!
//! The top-level functions use the hayro engine when its feature is enabled,
//! else pdfium. With both features on (the A/B configuration), each engine is
//! also addressable directly via [`hayro_backend`] / [`pdfium_backend`].
//!
//! API differences from `junk-libs-pdfium`: there is no `instance()` and no
//! `&Pdfium` parameter — construction is plain and there is no global state.
//! Errors are a typed [`Error`] instead of `anyhow`, so callers can
//! distinguish "needs a password" from "broken file"; it converts into
//! `anyhow::Error` with `?` like before.
//!
//! Text layer: both backends fill `char_boxes`. pdfium reports its own
//! `loose_bounds`; the hayro backend records glyphs through a custom
//! interpreter device (`hayro_text`), synthesizing the loose character cell
//! from the advance width and a nominal ascent/descent. The A/B test suite
//! (`tests/ab_backends.rs`, both features enabled) checks the two engines
//! agree on strings, geometry, and pixels.

#[cfg(not(any(feature = "backend-hayro", feature = "backend-pdfium")))]
compile_error!(
    "junk-libs-platen needs at least one of the `backend-hayro` or `backend-pdfium` features"
);

#[cfg(feature = "backend-hayro")]
pub mod hayro_backend;
#[cfg(feature = "backend-hayro")]
mod hayro_text;
#[cfg(feature = "backend-pdfium")]
pub mod pdfium_backend;

#[cfg(feature = "backend-hayro")]
use hayro_backend as default_backend;
#[cfg(all(feature = "backend-pdfium", not(feature = "backend-hayro")))]
use pdfium_backend as default_backend;

use std::path::Path;

/// Cap any single render dimension to avoid OOM at extreme zoom.
/// (Same value `junk-libs-pdfium` used.)
pub const MAX_RENDER_DIMENSION: u32 = 4096;

/// Rendering and document errors, typed so a viewer can react sensibly —
/// in particular, prompt for a password instead of showing "broken file".
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The document is encrypted and needs a password we weren't given.
    #[error("document is password-protected and requires a password")]
    PasswordRequired,
    /// A password was supplied and it didn't authenticate.
    #[error("incorrect password for password-protected document")]
    IncorrectPassword,
    /// Encrypted with something we can't decrypt (e.g. public-key handler).
    #[error("unsupported encryption: {0}")]
    UnsupportedEncryption(String),
    /// Not a PDF, or damaged beyond what the parser can repair.
    #[error("invalid or unrecoverably damaged PDF")]
    Invalid,
    #[error("page {index} out of range ({count} pages)")]
    PageOutOfRange { index: usize, count: usize },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Backend-specific failure (e.g. the pdfium engine failed to bind its
    /// dynamic library). Carries the engine's own message.
    #[error("render engine error: {0}")]
    Engine(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A character from the PDF text layer with its bounding box.
///
/// Bounds are in page points with a **top-left origin (y grows downward)** —
/// the same space as the rasterized [`RenderedPage`], so boxes overlay the
/// page image directly. Loose bounds (the full character cell, including side
/// bearing) so adjacent chars tile cleanly into selection blocks.
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
    /// Per-character boxes for snap-to-text selection and the native text
    /// layer. Empty for pages (or documents) with no extractable text.
    pub char_boxes: Vec<CharBox>,
}

/// Load a PDF and render every page to RGBA at the given zoom (1.0 = 100%).
pub fn render_pdf(path: &Path, zoom: f32) -> Result<Vec<RenderedPage>> {
    default_backend::render_pdf(path, zoom)
}

/// Render a single page to RGBA at the given scale (pixels per point) — bitmap
/// only, no text extraction. For re-rendering a page at a higher resolution
/// when the viewer zooms in. Returns the image and the page's native size in
/// points.
pub fn render_page_bitmap(
    path: &Path,
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    default_backend::render_page_bitmap(path, page_index, scale)
}

/// Like [`render_page_bitmap`] but for an in-memory PDF — e.g. a freshly
/// typeset or imported document not yet written to disk.
pub fn render_page_bitmap_from_bytes(
    bytes: &[u8],
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    default_backend::render_page_bitmap_from_bytes(bytes, page_index, scale)
}

/// Number of pages in a PDF, without rendering any of them — for sizing a
/// viewer before the first page is rendered.
pub fn page_count(path: &Path) -> Result<usize> {
    default_backend::page_count(path)
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

/// Clamp `scale` so neither page dimension exceeds [`MAX_RENDER_DIMENSION`].
#[cfg(feature = "backend-hayro")]
pub(crate) fn cap_scale(width_pts: f32, height_pts: f32, scale: f32) -> f32 {
    let max = MAX_RENDER_DIMENSION as f32;
    let mut capped = scale;
    if width_pts * capped > max {
        capped = max / width_pts;
    }
    if height_pts * capped > max {
        capped = max / height_pts;
    }
    capped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_zoom_steps() {
        assert_eq!(quantize_zoom(0.1), 25); // clamped low
        assert_eq!(quantize_zoom(0.30), 25);
        assert_eq!(quantize_zoom(0.40), 50);
        assert_eq!(quantize_zoom(1.0), 100);
        assert_eq!(quantize_zoom(1.3), 150);
        assert_eq!(quantize_zoom(9.0), 400); // clamped high
    }

    #[cfg(feature = "backend-hayro")]
    #[test]
    fn cap_scale_bounds_both_dimensions() {
        // A4 at zoom 10 would be 5950×8420 px; the cap binds on height.
        let capped = cap_scale(595.0, 842.0, 10.0);
        assert!((842.0 * capped) <= MAX_RENDER_DIMENSION as f32 + 0.5);
        assert!((595.0 * capped) <= MAX_RENDER_DIMENSION as f32 + 0.5);
        // Small renders pass through untouched.
        assert_eq!(cap_scale(595.0, 842.0, 1.0), 1.0);
    }
}
