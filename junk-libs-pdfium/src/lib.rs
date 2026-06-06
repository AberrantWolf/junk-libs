//! GUI-agnostic PDFium render core: open a PDF, rasterize pages to RGBA, and
//! extract the text layer as per-character boxes.
//!
//! Shared between the print-junk and expat-junk tools. Nothing here depends on a
//! UI toolkit. Binding is dynamic at runtime, searched in this order:
//!
//! 1. the build-vendored library — this crate's `build.rs` downloads `libpdfium`
//!    into `OUT_DIR` and exports its directory as `PDFIUM_LIB_DIR`, which
//!    [`instance`] reads via `option_env!` (present for in-tree dev/test builds);
//! 2. a copy shipped **next to the executable** (packaged apps), searched
//!    relative to the binary so it works regardless of the launch directory and
//!    deterministically prefers our pinned binary over any system one;
//! 3. a system-installed PDFium, as a last resort.
//!
//! The Cargo `pdfium_NNNN` feature and `build.rs`'s `PDFIUM_VERSION` must name the
//! same PDFium build, or binding fails with `LoadLibraryError: undefined symbol`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result};
use pdfium_render::prelude::*;

/// Cap any single render dimension to avoid OOM at extreme zoom.
const MAX_RENDER_DIMENSION: i32 = 4096;

/// Serialize whole render operations across threads.
///
/// The `thread_safe` crate feature guards each individual PDFium FFI call, but a
/// single render is a *sequence* of calls (load document → iterate pages → render
/// → read bitmap → extract text) over shared library state. Letting two such
/// sequences interleave corrupts that state and segfaults, so each render entry
/// point holds this lock for its whole duration. Rendering is the bottleneck
/// anyway; correctness wins over the lost parallelism. Poison-tolerant: a panic
/// mid-render shouldn't wedge every later render.
fn render_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bind PDFium, preferring our pinned binary over any system copy (see the
/// module docs for the search order). Each candidate is tried with an explicit
/// path so we never depend on the OS loader's implicit search — which could bind
/// a mismatched system library and fail with an undefined-symbol error.
fn bind() -> Result<Pdfium> {
    // 1. The build-vendored library. Its baked OUT_DIR path won't exist on an end
    //    user's machine, so this is skipped there and we fall through to (2).
    if let Some(dir) = option_env!("PDFIUM_LIB_DIR")
        && let Ok(bindings) =
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir))
    {
        return Ok(Pdfium::new(bindings));
    }
    // 2. A copy shipped alongside the executable (packaged apps).
    for dir in bundled_lib_dirs() {
        if let Ok(bindings) =
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
        {
            return Ok(Pdfium::new(bindings));
        }
    }
    // 3. A system-installed PDFium (version must match the pinned build).
    Pdfium::bind_to_system_library()
        .map(Pdfium::new)
        .context("failed to bind to a vendored, bundled, or system PDFium library")
}

/// Directories to search for a PDFium library shipped next to the executable,
/// covering the layouts our release artifacts use: beside the binary (Windows
/// `.dll`), `../lib` (Linux AppImage `usr/bin` → `usr/lib`), and `../Frameworks`
/// (macOS `.app` `Contents/MacOS` → `Contents/Frameworks`). Resolved relative to
/// the executable, not the working directory, so it holds wherever the app is
/// launched from. Empty if the executable path can't be determined.
fn bundled_lib_dirs() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| {
        exe.parent().map(Path::to_path_buf)
    }) else {
        return Vec::new();
    };
    vec![dir.join("../lib"), dir.join("../Frameworks"), dir]
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
    let _guard = render_lock();
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("loading PDF {}", path.display()))?;

    let mut pages = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let (image, size_pts) =
            render_page_image(&page, zoom).with_context(|| format!("rendering page {index}"))?;
        let char_boxes = extract_char_boxes(&page, size_pts.1);
        pages.push(RenderedPage {
            image,
            size_pts,
            char_boxes,
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
    let _guard = render_lock();
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("loading PDF {}", path.display()))?;
    render_doc_page(&document, page_index, scale)
}

/// Like [`render_page_bitmap`] but for an in-memory PDF — e.g. a freshly typeset
/// or imported document not yet written to disk. The byte buffer only needs to
/// outlive the call.
pub fn render_page_bitmap_from_bytes(
    pdfium: &Pdfium,
    bytes: &[u8],
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    let _guard = render_lock();
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .context("loading PDF from memory")?;
    render_doc_page(&document, page_index, scale)
}

/// Number of pages in a PDF, without rendering any of them — for sizing a viewer
/// before the first page is rendered.
pub fn page_count(pdfium: &Pdfium, path: &Path) -> Result<usize> {
    let _guard = render_lock();
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("loading PDF {}", path.display()))?;
    Ok(document.pages().len() as usize)
}

/// Render page `page_index` of an already-open document to RGBA at `scale`.
/// Shared by the file- and bytes-based single-page entry points.
fn render_doc_page(
    document: &PdfDocument,
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    let page = document
        .pages()
        .get(page_index as i32)
        .with_context(|| format!("page {page_index} out of range"))?;
    render_page_image(&page, scale).with_context(|| format!("rendering page {page_index}"))
}

/// Render one already-loaded page to RGBA at `scale` (pixels per point), with its
/// native point size. The shared core of every render entry point.
fn render_page_image(page: &PdfPage, scale: f32) -> Result<(image::RgbaImage, (f32, f32))> {
    let width_pts = page.width().value;
    let height_pts = page.height().value;
    let config = make_render_config(width_pts, height_pts, scale);
    let image = page
        .render_with_config(&config)?
        .as_image()
        .context("converting page bitmap")?
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

    /// Render a single page straight from an in-memory PDF buffer (no file path),
    /// the path the GUI uses for freshly typeset/imported documents.
    #[test]
    fn renders_a_page_from_bytes() {
        let pdfium = instance().expect("bind PDFium");

        let path = std::env::temp_dir().join("junk-libs-pdfium-bytes-test.pdf");
        {
            let mut document = pdfium.create_new_pdf().expect("create pdf");
            document
                .pages_mut()
                .create_page_at_start(PdfPagePaperSize::a4())
                .expect("create page");
            document.save_to_file(&path).expect("save pdf");
        }
        let bytes = std::fs::read(&path).expect("read pdf bytes");
        let _ = std::fs::remove_file(&path);

        let (image, size_pts) =
            render_page_bitmap_from_bytes(pdfium, &bytes, 0, 1.0).expect("render from bytes");
        assert!(
            (size_pts.0 - 595.0).abs() < 5.0 && (size_pts.1 - 842.0).abs() < 5.0,
            "unexpected page size in points: {size_pts:?}"
        );
        assert!(
            (570..=620).contains(&image.width()) && (820..=870).contains(&image.height()),
            "unexpected raster size: {}×{}",
            image.width(),
            image.height()
        );
    }

    /// The bundled-library search dirs must be derived relative to the executable
    /// (not the cwd) and cover the release bundle layouts.
    #[test]
    fn bundled_lib_dirs_are_exe_relative() {
        let dirs = bundled_lib_dirs();
        assert!(
            !dirs.is_empty(),
            "should derive candidate dirs from current_exe"
        );
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(dirs.contains(&exe_dir), "same dir as the binary (Windows)");
        assert!(
            dirs.contains(&exe_dir.join("../lib")),
            "../lib (Linux AppImage)"
        );
        assert!(
            dirs.contains(&exe_dir.join("../Frameworks")),
            "../Frameworks (macOS .app)"
        );
    }

    /// Page count must come back without rendering, and match the pages created.
    #[test]
    fn counts_pages_without_rendering() {
        let pdfium = instance().expect("bind PDFium");

        let path = std::env::temp_dir().join("junk-libs-pdfium-count-test.pdf");
        {
            let mut document = pdfium.create_new_pdf().expect("create pdf");
            for _ in 0..3 {
                document
                    .pages_mut()
                    .create_page_at_start(PdfPagePaperSize::a4())
                    .expect("create page");
            }
            document.save_to_file(&path).expect("save pdf");
        }

        let count = page_count(pdfium, &path).expect("count pages");
        let _ = std::fs::remove_file(&path);
        assert_eq!(count, 3);
    }
}
