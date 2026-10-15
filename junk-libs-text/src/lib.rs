//! GUI-independent Unicode shaping, bidi layout, fallback and rasterization.
//! Fonts are explicit: the bundled OFL Noto faces cover Latin, Cyrillic, Arabic,
//! Hebrew, Devanagari and Thai. Hosts can add CJK or other fonts without relying
//! on installed system fonts. Retain one renderer per rendering worker/context.

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
    fontdb,
};
use image::{Rgba, RgbaImage};

pub const BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../fonts/NotoSans.ttf"),
    include_bytes!("../fonts/NotoSansArabic.ttf"),
    include_bytes!("../fonts/NotoSansHebrew.ttf"),
    include_bytes!("../fonts/NotoSansDevanagari.ttf"),
    include_bytes!("../fonts/NotoSansThai.ttf"),
];
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}
#[derive(Clone, Copy)]
pub struct TextStyle {
    pub alignment: Alignment,
    pub bold: bool,
    /// Maximum starting em size in raster pixels; None auto-fits the box.
    pub size_px: Option<f32>,
    pub padding_px: f32,
}
impl Default for TextStyle {
    fn default() -> Self {
        Self {
            alignment: Alignment::Left,
            bold: false,
            size_px: None,
            padding_px: 1.0,
        }
    }
}
#[derive(Debug, Default)]
pub struct LayoutReport {
    pub font_size: f32,
    pub lines: usize,
    pub glyphs: usize,
    pub missing_glyphs: usize,
    pub overflow: bool,
    pub rtl_runs: usize,
}
pub struct TextRaster {
    pub image: RgbaImage,
    pub layout: LayoutReport,
}

pub struct TextRenderer {
    fonts: FontSystem,
    swash: SwashCache,
}
impl Default for TextRenderer {
    fn default() -> Self {
        Self::with_fonts(std::iter::empty())
    }
}
impl TextRenderer {
    pub fn with_fonts(extra: impl IntoIterator<Item = Vec<u8>>) -> Self {
        let mut db = fontdb::Database::new();
        for data in BUNDLED_FONTS {
            db.load_font_data(data.to_vec());
        }
        for data in extra {
            db.load_font_data(data);
        }
        db.set_sans_serif_family("Noto Sans");
        Self {
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            swash: SwashCache::new(),
        }
    }
    fn buffer(&mut self, text: &str, width: f32, size: f32, style: TextStyle) -> Buffer {
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, size * 1.25));
        buffer.set_size(Some(width), None);
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_text(
            text,
            &Attrs::new()
                .family(Family::SansSerif)
                .weight(if style.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                }),
            Shaping::Advanced,
            None,
        );
        for line in &mut buffer.lines {
            line.set_align(Some(match style.alignment {
                Alignment::Left => Align::Left,
                Alignment::Center => Align::Center,
                Alignment::Right => Align::Right,
            }));
        }
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
    }
    /// Render a complete layout before testing fit, so off-screen lines cannot
    /// be mistaken for text that fits. Binary search is bounded to 12 passes.
    pub fn render(
        &mut self,
        text: &str,
        width: u32,
        height: u32,
        bg: [u8; 4],
        fg: [u8; 4],
        style: TextStyle,
    ) -> TextRaster {
        let mut image = RgbaImage::from_pixel(width.max(1), height.max(1), Rgba(bg));
        if text.trim().is_empty() {
            return TextRaster {
                image,
                layout: LayoutReport::default(),
            };
        }
        let pad = style
            .padding_px
            .max(0.0)
            .min((width.min(height) as f32) * 0.25);
        let max_w = (width as f32 - 2.0 * pad).max(0.1);
        let max_h = (height as f32 - 2.0 * pad).max(0.1);
        let high = style
            .size_px
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(max_h / 1.25)
            .min(max_h)
            .max(0.25);
        let fits = |b: &Buffer| {
            b.layout_runs()
                .all(|r| r.line_top + r.line_height <= max_h + 0.01 && r.line_w <= max_w + 0.01)
        };
        let mut buffer = self.buffer(text, max_w, high, style);
        let mut size = high;
        if !fits(&buffer) {
            let (mut lo, mut hi) = (0.25, high);
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                let candidate = self.buffer(text, max_w, mid, style);
                if fits(&candidate) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            size = lo;
            buffer = self.buffer(text, max_w, size, style);
        }
        let mut layout = LayoutReport {
            font_size: size,
            overflow: !fits(&buffer),
            ..LayoutReport::default()
        };
        for run in buffer.layout_runs() {
            layout.lines += 1;
            layout.rtl_runs += usize::from(run.rtl);
            for glyph in run.glyphs {
                layout.glyphs += 1;
                layout.missing_glyphs += usize::from(glyph.glyph_id == 0);
            }
        }
        buffer.draw(
            &mut self.fonts,
            &mut self.swash,
            Color::rgba(fg[0], fg[1], fg[2], fg[3]),
            |x, y, w, h, color| {
                let rgba = color.as_rgba();
                for dy in 0..h {
                    for dx in 0..w {
                        let (px, py) = (
                            x + dx as i32 + pad.round() as i32,
                            y + dy as i32 + pad.round() as i32,
                        );
                        if px >= 0
                            && py >= 0
                            && (px as u32) < image.width()
                            && (py as u32) < image.height()
                        {
                            let p = image.get_pixel_mut(px as u32, py as u32);
                            let a = rgba[3] as f32 / 255.0;
                            for c in 0..3 {
                                p[c] = (rgba[c] as f32 * a + p[c] as f32 * (1.0 - a)).round() as u8;
                            }
                        }
                    }
                }
            },
        );
        // Bound accumulated glyph bitmaps across many documents and zoom levels.
        if self.swash.image_cache.len() > 4096 {
            self.swash = SwashCache::new();
        }
        TextRaster { image, layout }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shapes_scripts_and_combining_marks_without_system_fonts() {
        let mut renderer = TextRenderer::default();
        for text in [
            "مرحبا بالعالم 123",
            "שלום עולם 123",
            "नमस्ते दुनिया",
            "สวัสดีชาวโลก",
            "Tiếng Việt",
            "A\u{301} e\u{301}",
        ] {
            let raster = renderer.render(
                text,
                400,
                100,
                [255; 4],
                [0, 0, 0, 255],
                TextStyle::default(),
            );
            assert_eq!(raster.layout.missing_glyphs, 0, "{text}");
            assert!(!raster.layout.overflow, "{text}");
            assert!(raster.image.pixels().any(|p| p[0] < 200), "{text}");
        }
    }
    #[test]
    fn arabic_uses_joined_glyphs_and_bidi() {
        let mut renderer = TextRenderer::default();
        let result = renderer.render(
            "لا 123",
            300,
            80,
            [255; 4],
            [0, 0, 0, 255],
            TextStyle::default(),
        );
        assert!(result.layout.rtl_runs > 0);
        let isolated = renderer.buffer("ب", 300.0, 30.0, TextStyle::default());
        let isolated_ids: Vec<_> = isolated
            .layout_runs()
            .flat_map(|r| r.glyphs.iter().map(|g| g.glyph_id))
            .collect();
        let joined = renderer.buffer("بب", 300.0, 30.0, TextStyle::default());
        assert!(
            joined
                .layout_runs()
                .flat_map(|r| r.glyphs)
                .any(|g| !isolated_ids.contains(&g.glyph_id)),
            "Arabic must use contextual forms"
        );
    }
    #[test]
    fn weight_alignment_and_scale_affect_the_shared_raster() {
        let mut renderer = TextRenderer::default();
        let style = TextStyle {
            size_px: Some(24.0),
            padding_px: 2.0,
            ..Default::default()
        };
        let left = renderer.render("Office AV", 200, 60, [255; 4], [0, 0, 0, 255], style);
        let right = renderer.render(
            "Office AV",
            200,
            60,
            [255; 4],
            [0, 0, 0, 255],
            TextStyle {
                alignment: Alignment::Right,
                ..style
            },
        );
        let bold = renderer.render(
            "Office AV",
            200,
            60,
            [255; 4],
            [0, 0, 0, 255],
            TextStyle {
                bold: true,
                ..style
            },
        );
        let min_x = |image: &RgbaImage| {
            image
                .enumerate_pixels()
                .filter(|(_, _, p)| p[0] < 100)
                .map(|(x, _, _)| x)
                .min()
                .unwrap()
        };
        assert!(min_x(&right.image) > min_x(&left.image) + 40);
        assert_ne!(left.image, bold.image);
        let scaled = renderer.render(
            "Office AV",
            400,
            120,
            [255; 4],
            [0, 0, 0, 255],
            TextStyle {
                size_px: Some(48.0),
                padding_px: 4.0,
                ..style
            },
        );
        assert_eq!(left.layout.lines, scaled.layout.lines);
        assert!((left.layout.font_size * 2.0 - scaled.layout.font_size).abs() < 0.01);
    }

    #[test]
    fn wraps_and_fits_every_line() {
        let mut renderer = TextRenderer::default();
        let result = renderer.render(
            "A long paragraph with several words that must wrap into a narrow box.",
            80,
            60,
            [255; 4],
            [0, 0, 0, 255],
            TextStyle {
                size_px: Some(40.0),
                ..Default::default()
            },
        );
        assert!(result.layout.lines > 1);
        assert!(!result.layout.overflow);
        assert!(result.layout.font_size < 40.0);
    }
}
