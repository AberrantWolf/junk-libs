# Shared Unicode text rendering

`TextRenderer` uses COSMIC Text 0.19 for shaping, bidi layout, Unicode line
breaking, font fallback and Swash rasterization. It has no egui or PDF dependency.
Retain a renderer per worker or rendering context to reuse font and glyph caches.

The bundled SIL OFL Noto fonts cover Latin, Cyrillic, Vietnamese, Arabic, Hebrew,
Devanagari and Thai. `with_fonts` adds host-provided CJK or other fonts to an
explicit database; it does not scan installed fonts. The matching notices are in
`fonts/*-OFL.txt` and must accompany distributed binaries embedding the fonts.

`TextStyle` specifies alignment, weight, optional maximum em size and padding in
raster pixels. To use page points, multiply size and padding by pixels-per-point.
Auto-fit shapes the full text and checks every line before bounded binary search;
`LayoutReport` exposes the chosen size, missing glyphs and overflow. Rotation and
background sampling remain host responsibilities. Cache eviction drops glyph
bitmaps after 4096 cached glyph images; caller-owned page/patch caches should also
be bounded.

Generate a visual sample with optional additional font files:

```sh
cargo run -p junk-libs-text --example showcase -- sample.png Japanese.ttf Korean.ttf
```
