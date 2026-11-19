# junk-libs-platen

Engine-agnostic PDF rendering: rasterize pages to RGBA, extract the text layer
as per-character boxes, and read navigation metadata without rendering.

## API

- `render_pdf`, `render_page_bitmap[_from_bytes]`, `render_page_from_bytes`,
  `page_count`, `page_sizes_from_bytes` — rendering and page geometry.
  Renders are capped at `MAX_RENDER_DIMENSION` pixels per side and drawn on an
  opaque white background.
- `metadata::read[_from_bytes]` — outline (with destination positions where the
  PDF gives one), page labels and recoverable warnings, via `lopdf`.
- `reading_order::reconstruct` — rebuilds lines, columns, RTL and vertical text
  from selected `CharBox`es for untagged PDFs.
- `quantize_render_scale` — stable cache keys for render scales.
- `Error` is typed so callers can tell `PasswordRequired` / `IncorrectPassword`
  from `Invalid` or `PageOutOfRange`.

There is no global state: calls are self-contained and may run in parallel.

## Backends

- `backend-hayro` (default) — pure-Rust [hayro](https://github.com/LaurenzV/hayro).
  Rendering warnings are forwarded to `log::warn!`.
- `backend-pdfium` — delegates to `junk-libs-pdfium`. With both features on,
  `hayro_backend` and `pdfium_backend` are addressable directly, and
  `tests/ab_backends.rs` plus `examples/ab_compare.rs` compare the two:

  ```sh
  cargo run -p junk-libs-platen --features backend-pdfium \
      --example ab_compare -- [--dump <dir>] corpus/*.pdf
  ```

## Text layer (hayro)

Glyphs are captured through a recording interpreter device:

- Boxes are in top-left page points, matching a scale-1.0 render.
- Cells are advance-width wide with a nominal −200..+800/1000-em height, so
  adjacent characters tile for selection. Heights do not match pdfium exactly.
- Invisible text (render mode 3, e.g. OCR layers) is included; fill+stroke
  double reports are collapsed; ligatures split their cell evenly.
- Type3 glyphs get tight bounds measured from their content streams.
- Whitespace the PDF lacks (e.g. TJ-kerned LaTeX word gaps) is synthesized:
  a space for word-sized gaps, a newline on baseline change.

## Non-goals

Editing, forms, signing, JavaScript and XFA.
