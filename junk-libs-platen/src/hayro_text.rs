//! Text-layer extraction for the hayro backend: a recording
//! [`Device`] that ignores paint and captures `draw_glyph` calls as
//! [`CharBox`]es (SPEC.md §4 / M1).
//!
//! hayro has no public text-extraction API yet (upstream #452/#1049), but it
//! gives us the pieces: `draw_glyph` fires for every shown glyph — including
//! invisible text, which is how OCR'd scans carry their text layer — with the
//! page CTM and the glyph transform, and `Glyph::as_unicode()` resolves the
//! character via ToUnicode/glyph-name/uniXXXX fallbacks.
//!
//! Box synthesis: pdfium's `loose_bounds` reports the full character cell
//! (advance wide, ascent-to-descent tall) so adjacent characters tile into
//! selection blocks. hayro doesn't expose font ascent/descent, so we use a
//! nominal cell of −200..+800 in 1000-upem glyph space — a typical Latin
//! ascent/descent split. Cell heights won't match pdfium's to the point, but
//! they tile the same way, which is what selection needs. The A/B harness
//! decides whether this is close enough (SPEC.md open question).

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page,
};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_syntax::page::Page;
use hayro::vello_cpu::kurbo::{Affine, BezPath, Rect, Shape};

use crate::CharBox;

/// Nominal character cell in 1000-upem glyph space (y up): x spans the
/// advance width, y spans descent..ascent.
const CELL_ASCENT: f64 = 800.0;
const CELL_DESCENT: f64 = -200.0;

/// Interpret `page` and return its text layer in top-left page-point space —
/// the same space as a scale-1.0 render, so boxes overlay the raster.
pub(crate) fn extract_char_boxes<'a>(
    page: &'a Page<'a>,
    cache: &InterpreterCache<'a>,
    settings: &InterpreterSettings,
) -> Vec<CharBox> {
    let initial_transform = page.initial_transform(true).to_kurbo();
    let (width, height) = page.render_dimensions();
    let mut ctx = Context::new(
        initial_transform,
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        cache,
        page.xref(),
        settings.clone(),
    );
    let mut recorder = TextRecorder::default();
    interpret_page(page, &mut ctx, &mut recorder);
    recorder.boxes
}

/// Synthesize a space when the gap between two glyph cells on the same line
/// exceeds this fraction of the cell height (≈ font size). Word gaps in PDFs
/// that position words via TJ kerning (most LaTeX output — no space glyphs at
/// all) are typically 0.2–0.35 em; inter-letter kerning stays well under 0.1.
const SPACE_GAP_FRACTION: f64 = 0.18;

#[derive(Default)]
struct TextRecorder {
    boxes: Vec<CharBox>,
    /// Cell of the last *glyph* recorded (synthetic whitespace excluded):
    /// dedup reference for fill+stroke double-reports and anchor for
    /// whitespace synthesis.
    last_cell: Option<(char, Rect)>,
}

impl TextRecorder {
    /// Append `text` spread evenly across `cell` (a device-space rect in
    /// top-left page points), synthesizing whitespace relative to the
    /// previous glyph. Like pdfium's text API, we emit characters PDFs often
    /// don't contain: a space for a word-sized horizontal gap, a newline on
    /// baseline change. Multi-char entries come from ligatures and
    /// multi-codepoint ToUnicode mappings.
    fn push(&mut self, text: &BfString, cell: Rect) {
        let mut buf = [0u8; 4];
        let chars: &str = match text {
            BfString::Char(c) => c.encode_utf8(&mut buf),
            BfString::String(s) => s,
        };
        let n = chars.chars().count();
        if n == 0 || cell.width() <= 0.0 {
            return;
        }

        // Fill+stroke render modes report the same glyph twice; collapse the
        // exact repeat.
        let first = chars.chars().next().expect("checked non-empty");
        if let Some((last_ch, last)) = self.last_cell
            && last_ch == first
            && (last.x0 - cell.x0).abs() < 1e-3
            && (last.y0 - cell.y0).abs() < 1e-3
        {
            return;
        }

        self.synthesize_whitespace(cell);
        let slice_w = cell.width() / n as f64;
        for (i, ch) in chars.chars().enumerate() {
            self.boxes.push(CharBox {
                ch,
                x: (cell.x0 + slice_w * i as f64) as f32,
                y: cell.y0 as f32,
                w: slice_w as f32,
                h: cell.height() as f32,
            });
        }
        self.last_cell = Some((first, cell));
    }

    /// Emit a synthetic ' ' or '\n' between the previous glyph and `cell`
    /// when their spacing implies one. Skipped when the document carries a
    /// real space glyph (it arrives as a normal cell and resets the anchor).
    fn synthesize_whitespace(&mut self, cell: Rect) {
        let Some((_, last)) = self.last_cell else {
            return;
        };
        let same_line = {
            let overlap = last.y1.min(cell.y1) - last.y0.max(cell.y0);
            overlap > 0.5 * last.height().min(cell.height())
        };
        let ref_h = last.height().max(cell.height());

        if !same_line {
            // Zero-width box at the end of the previous glyph; consumers
            // treat zero-area whitespace boxes via center-point tests.
            self.boxes.push(CharBox {
                ch: '\n',
                x: last.x1 as f32,
                y: last.y0 as f32,
                w: 0.0,
                h: last.height() as f32,
            });
            return;
        }

        let gap = cell.x0 - last.x1;
        if gap > SPACE_GAP_FRACTION * ref_h {
            // Word gap (or a column jump — either way a separator). The box
            // spans the gap so region selection picks it up like pdfium's
            // synthetic spaces.
            self.boxes.push(CharBox {
                ch: if gap > 2.0 * ref_h { '\n' } else { ' ' },
                x: last.x1 as f32,
                y: cell.y0.min(last.y0) as f32,
                w: gap as f32,
                h: cell.height().max(last.height()) as f32,
            });
        }
    }
}

impl<'a> Device<'a> for TextRecorder {
    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        _draw_mode: &GlyphDrawMode,
    ) {
        let Some(unicode) = glyph.as_unicode() else {
            return;
        };
        let to_device = transform * glyph_transform;

        let cell = match glyph {
            Glyph::Outline(outline) => {
                // Prefer the advance width (full cell, like pdfium's loose
                // bounds); fall back to the outline's own extent for fonts
                // with broken width tables.
                let advance = outline
                    .advance_width()
                    .map(f64::from)
                    .filter(|adv| *adv > 0.0)
                    .or_else(|| {
                        let bbox = outline.outline().bounding_box();
                        (bbox.x1 > 0.0).then_some(bbox.x1)
                    });
                let Some(advance) = advance else {
                    return;
                };
                to_device.transform_rect_bbox(Rect::new(0.0, CELL_DESCENT, advance, CELL_ASCENT))
            }
            Glyph::Type3(type3) => {
                // No public metrics for Type3 fonts; interpret the glyph's
                // content stream against a bounds collector instead. Tight
                // bounds, not a cell — acceptable for a rare font kind.
                let mut bounds = BoundsDevice::default();
                type3.interpret(&mut bounds, transform, glyph_transform, paint);
                let Some(bbox) = bounds.bbox else {
                    return;
                };
                bbox
            }
        };

        self.push(&unicode, cell);
    }

    fn set_soft_mask(&mut self, _mask: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _blend_mode: BlendMode) {}
    fn draw_path(
        &mut self,
        _path: &BezPath,
        _transform: Affine,
        _paint: &Paint<'a>,
        _draw_mode: &PathDrawMode,
    ) {
    }
    fn push_clip_path(&mut self, _clip_path: &ClipPath) {}
    fn push_transparency_group(
        &mut self,
        _opacity: f32,
        _mask: Option<SoftMask<'a>>,
        _blend_mode: BlendMode,
    ) {
    }
    fn draw_image(&mut self, _image: Image<'a, '_>, _transform: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Accumulates the device-space bounding box of everything drawn into it.
/// Used to measure Type3 glyphs, whose shapes are PDF content streams.
#[derive(Default)]
struct BoundsDevice {
    bbox: Option<Rect>,
}

impl BoundsDevice {
    fn add(&mut self, rect: Rect) {
        self.bbox = Some(match self.bbox {
            Some(prev) => prev.union(rect),
            None => rect,
        });
    }
}

impl<'a> Device<'a> for BoundsDevice {
    fn draw_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        _paint: &Paint<'a>,
        _draw_mode: &PathDrawMode,
    ) {
        self.add(transform.transform_rect_bbox(path.bounding_box()));
    }

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _paint: &Paint<'a>,
        _draw_mode: &GlyphDrawMode,
    ) {
        // Type3 glyphs may nest text; measure outlines, skip deeper nesting.
        if let Glyph::Outline(outline) = glyph {
            self.add(
                (transform * glyph_transform).transform_rect_bbox(outline.outline().bounding_box()),
            );
        }
    }

    fn set_soft_mask(&mut self, _mask: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _blend_mode: BlendMode) {}
    fn push_clip_path(&mut self, _clip_path: &ClipPath) {}
    fn push_transparency_group(
        &mut self,
        _opacity: f32,
        _mask: Option<SoftMask<'a>>,
        _blend_mode: BlendMode,
    ) {
    }
    fn draw_image(&mut self, _image: Image<'a, '_>, _transform: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}
