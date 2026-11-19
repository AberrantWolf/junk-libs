# junk-libs-text

GUI-independent Unicode text rendering into `image::RgbaImage`: shaping, bidi,
line breaking, font fallback and rasterization via COSMIC Text.

## Usage

`TextRenderer::render` lays out text in a pixel box and returns the raster plus
a `LayoutReport` (chosen size, line/glyph counts, missing glyphs, overflow).
`TextStyle` sets alignment, weight, padding and an optional maximum em size in
raster pixels; without a size the text is auto-fit so every line fits. To work
in page points, multiply size and padding by pixels-per-point.

Keep one renderer per worker or rendering context so font and glyph caches are
reused. Rotation and background sampling are left to the caller.

## Fonts

Bundled Noto fonts (`BUNDLED_FONTS`) cover Latin, Cyrillic, Vietnamese, Arabic,
Hebrew, Devanagari and Thai. `TextRenderer::with_fonts` adds caller-supplied
fonts (e.g. CJK); installed system fonts are never scanned. The fonts are SIL
OFL licensed; ship `fonts/*-OFL.txt` with binaries that embed them.

## Example

```sh
cargo run -p junk-libs-text --example showcase -- sample.png [extra-font.ttf ...]
```
