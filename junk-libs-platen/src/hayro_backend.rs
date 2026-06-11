//! The pure-Rust engine, built on [hayro](https://github.com/LaurenzV/hayro).
//!
//! Each call opens the document, renders, and returns — no global state, no
//! lock, so calls run in parallel freely. (hayro's `RenderCache` is `Rc`-based
//! and can't be shared across calls anyway; `render_pdf` reuses one across the
//! pages of its single document, which is where reuse pays.)
//!
//! Pages render onto an opaque white background, like the pdfium backend's
//! paper-white pages. That also makes hayro's premultiplied-RGBA output
//! identical to the straight-alpha RGBA `image::RgbaImage` expects, since
//! every pixel ends up fully opaque.

use std::path::Path;
use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};

use crate::{CharBox, Error, RenderedPage, Result, cap_scale};

/// Open a PDF from bytes, mapping hayro's load errors to ours. The empty
/// password is what unprotected and owner-password-only documents need.
fn open(bytes: Vec<u8>) -> Result<Pdf> {
    Pdf::new(bytes).map_err(|e| match e {
        // No password given, so "password protected" means "needs one".
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => Error::PasswordRequired,
        LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm) => {
            Error::UnsupportedEncryption("unsupported encryption algorithm".into())
        }
        LoadPdfError::Decryption(_) => Error::Invalid,
        LoadPdfError::Invalid => Error::Invalid,
    })
}

fn open_file(path: &Path) -> Result<Pdf> {
    open(std::fs::read(path)?)
}

/// Interpreter settings wired to forward hayro's warnings to `log`, so
/// fidelity problems are visible instead of silent.
fn interpreter_settings() -> InterpreterSettings {
    InterpreterSettings {
        warning_sink: Arc::new(|warning| {
            log::warn!("hayro render warning: {warning:?}");
        }),
        ..Default::default()
    }
}

/// Render one page at `scale` (pixels per point, capped to
/// [`crate::MAX_RENDER_DIMENSION`]). Returns the image and the page's native
/// size in points.
fn render_page<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
    scale: f32,
) -> (image::RgbaImage, (f32, f32)) {
    let (width_pts, height_pts) = page.render_dimensions();
    let scale = cap_scale(width_pts, height_pts, scale);
    let pixmap = hayro::render(
        page,
        cache,
        settings,
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: None,
            height: None,
            bg_color: WHITE,
        },
    );
    let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    // Premultiplied == straight here: the white base layer makes every pixel
    // fully opaque (see module docs).
    let image = image::RgbaImage::from_raw(w, h, pixmap.data_as_u8_slice().to_vec())
        .expect("pixmap buffer matches its own dimensions");
    (image, (width_pts, height_pts))
}

pub fn render_pdf(path: &Path, zoom: f32) -> Result<Vec<RenderedPage>> {
    let pdf = open_file(path)?;
    let cache = RenderCache::new();
    let settings = interpreter_settings();
    // TODO(M1): text layer via a recording Device — char_boxes is empty until
    // then (upstream tracking: LaurenzV/hayro#452, #1049).
    log::warn!(
        "hayro backend: text layer extraction not implemented yet; char_boxes will be empty"
    );
    Ok(pdf
        .pages()
        .iter()
        .map(|page| {
            let (image, size_pts) = render_page(page, &cache, &settings, zoom);
            RenderedPage {
                image,
                size_pts,
                char_boxes: Vec::<CharBox>::new(),
            }
        })
        .collect())
}

pub fn render_page_bitmap(
    path: &Path,
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    render_doc_page(&open_file(path)?, page_index, scale)
}

pub fn render_page_bitmap_from_bytes(
    bytes: &[u8],
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    render_doc_page(&open(bytes.to_vec())?, page_index, scale)
}

pub fn page_count(path: &Path) -> Result<usize> {
    Ok(open_file(path)?.pages().len())
}

fn render_doc_page(
    pdf: &Pdf,
    page_index: usize,
    scale: f32,
) -> Result<(image::RgbaImage, (f32, f32))> {
    let pages = pdf.pages();
    let page = pages.get(page_index).ok_or(Error::PageOutOfRange {
        index: page_index,
        count: pages.len(),
    })?;
    Ok(render_page(
        page,
        &RenderCache::new(),
        &interpreter_settings(),
        scale,
    ))
}
