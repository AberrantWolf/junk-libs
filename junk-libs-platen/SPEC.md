# junk-libs-platen — engine-agnostic PDF rendering

*(Moved from the retired standalone `platen_junk`/`prendrs_junk` project on
2026-06-11, after deciding the right shape is a junk-libs crate, not a
standalone library. "Platen": the press plate that pushes the paper onto the
type.)*

**Mission:** replace `junk-libs-pdfium` as the rendering engine behind
print-junk and expat-junk, eliminating the PDFium binary download/bundling
story (`build.rs` fetch, per-platform `.so` shipping next to the executable,
Gatekeeper/VC++ caveats in INSTALL docs) and its process-global render lock.
The engine is a feature-flagged implementation detail; the interface is the
five-function surface the consumers already use.

Research summary and upstream facts were verified 2026-06-11.

## 1. Why hayro, why not a new engine

A from-scratch PDF renderer is multi-engineer-year work (pdf.js: ~100–150k
lines, years of Mozilla team time; MuPDF: ~250–400k lines of C). The last 5%
of wild-PDF fidelity (broken xrefs, font repair, knockout groups, JBIG2/JPX,
mesh shadings) historically costs as much as the first 95%.

[hayro](https://github.com/LaurenzV/hayro) (LaurenzV, typst/krilla/resvg
ecosystem) is the first permissively-licensed (MIT/Apache-2.0) pure-Rust
engine to credibly climb that hill: 1000+ document regression corpus scraped
largely from pdf.js/PDFBOX suites, rasterizing via `vello_cpu`. Alternatives
all fail a requirement: MuPDF is AGPL, Poppler is GPL + system C++ deps,
`pdf_render` is incomplete, pdfium is the binary-acquisition pain we're
leaving.

hayro specifics (as of 0.7.1, 2026-06-05):
- Passwords: RC4-40/128, AES-128, AES-256 (R1–R6) via `Pdf::new_with_password`
  (merged 2025-12-16; the crate docs' "no password support" blurb is stale).
  Upstream TODOs: PDFDocEncoding conversion, SASLprep for non-ASCII R5/R6
  passwords.
- Known fidelity gaps: knockout groups, blending/isolation edge cases,
  color-key masking, non-embedded CID fonts. `warning_sink` reports only
  `UnsupportedFont` and `ImageDecodeFailure` today — coverage is thin (§5).
- No public text-extraction API yet; glyph→unicode landed internally (PR
  #457), structured extraction in flight (#452, #1049).

Risk posture: hayro is pre-1.0 with effectively one maintainer. We pin the
exact version (`=0.7.1`), keep hayro types out of this crate's public API
(cheap here — the API is five functions over `image`/`std` types), and keep
the pdfium backend as an A/B oracle until trust is earned. Bump hayro
deliberately and re-run the A/B comparison when we do.

## 2. Architecture (as built)

```
junk-libs-platen/
  src/lib.rs             public API: render_pdf, render_page_bitmap[_from_bytes],
                         page_count, quantize_zoom, CharBox, RenderedPage, Error
  src/hayro_backend.rs   default engine (feature backend-hayro)
  src/pdfium_backend.rs  legacy engine (feature backend-pdfium), delegates to
                         junk-libs-pdfium; migration oracle only
  tests/hayro_render.rs  end-to-end render tests w/ self-built PDF fixture
```

Decisions baked in:
- **No global state.** `instance()` and the `&Pdfium` parameter are gone;
  every call is self-contained. Parallel rendering from multiple threads is
  tested (`renders_in_parallel`). hayro's `RenderCache` is `Rc`-based, so it's
  per-call (shared across pages within one `render_pdf` call, where reuse
  actually pays).
- **Typed errors** (`Error`), so the viewer can distinguish
  `PasswordRequired` / `IncorrectPassword` / `Invalid` / `PageOutOfRange`
  instead of parsing anyhow strings. Converts into `anyhow::Error` at call
  sites like before.
- **Opaque white background**, matching pdfium's paper-white pages — which
  also makes hayro's premultiplied output byte-identical to straight alpha.
- **`MAX_RENDER_DIMENSION` = 4096** cap (same as junk-libs-pdfium), applied by
  clamping scale. `quantize_zoom` carried over so consumers can drop the
  junk-libs-pdfium dependency entirely.
- hayro warnings forwarded to `log::warn!` — fidelity problems visible, not
  silent. (Full machine-readable diagnostics were a standalone-library
  feature; internally, logs + the A/B harness are what we need.)
- Both backends can be enabled at once; top-level functions dispatch to hayro
  when present, and each engine is addressable via `hayro_backend` /
  `pdfium_backend` for A/B work.

## 3. Replacement contract

| `junk-libs-pdfium` | `junk-libs-platen` | status |
|---|---|---|
| `instance()` + `&Pdfium` args | gone — plain calls | done |
| global `render_lock()` | gone — parallel renders tested | done |
| `render_page_bitmap(pdfium, path, idx, scale)` | `render_page_bitmap(path, idx, scale)` | done |
| `render_page_bitmap_from_bytes(pdfium, bytes, idx, scale)` | same minus `pdfium` | done |
| `page_count(pdfium, path)` | `page_count(path)` | done |
| `render_pdf(pdfium, path, zoom)` incl. `char_boxes` | `render_pdf(path, zoom)` — **`char_boxes` empty on hayro until M1** | bitmap done, text layer pending |
| `CharBox` (top-left points, loose bounds) | same shape, same semantics | type done |
| `quantize_zoom` | carried over verbatim | done |
| pdfium PDF-creation API (used only by its tests) | not replicated; fixtures are self-built or committed PDFs | n/a |

## 4. Text layer plan (M1 — the blocking work)

The GUI viewer's snap-to-text selection needs per-character boxes. hayro has
the pieces (glyph→unicode, PR #457) but no public API yet. Plan: a recording
`hayro_interpret::Device` that ignores paint and captures `draw_glyph` —
unicode, text-space transform, advance/bbox from font data — projected to
top-left page points with synthesized loose bounds (full cell from
ascent/descent, width from advance) to match pdfium's `loose_bounds()`
tiling. Validate against pdfium's boxes via the A/B harness (identical
strings, positions within tolerance). If upstream #1049 lands first, use it
and delete ours.

Open question: can loose-bounds synthesis match pdfium closely enough that
selection blocks tile the same? Decide tolerance empirically.

## 5. Testing & migration

- **In-crate**: self-built minimal-PDF fixture; asserts raster dimensions,
  actual pixel content, opacity, typed errors, zoom cap, parallel rendering.
- **A/B harness (M2 gate)**: an `xtask` renders the same documents through
  both backends, pixel-diffs (perceptual threshold) and, once M1 lands,
  diffs text layers. Corpus: print-junk imposition inputs/outputs, typeset
  output, a batch of arXiv PDFs from the expat-junk pipeline. Cutover
  criterion: no regressions the eye cares about.
- **Encrypted fixtures**: add a couple of committed password-protected PDFs
  (RC4-128, AES-256) once the viewer grows a password prompt; hayro's
  `PasswordRequired` mapping is in place either way.
- **Warning audit**: hayro's `warning_sink` only covers two cases; documents
  that exercise its known gaps (knockout groups, color-key masks) should be
  checked for *silent* divergence in the A/B diff — findings filed upstream.

## 6. Milestones

- **M0 — Core render layer.** ✅ Done 2026-06-11 (this crate as built; tests
  green).
- **M1 — Text layer.** Recording-`Device` char-box extraction (§4); A/B
  validation against pdfium boxes. *Exit: viewer selection works identically
  on a hayro render.*
- **M2 — Migration.** Cut over `junk-libs-egui-pdfdoc`, print-junk-gui's
  viewer handlers, and pdf-import's hires probe to `junk-libs-platen`; run
  the A/B harness across the real corpora; then delete the PDFium
  download/bundle machinery from release packaging and demote
  `backend-pdfium` to dev-only (or delete it). *Exit: print-junk and
  expat-junk ship with no PDFium anywhere.*
- **M3 — Hardening & upstream.** Fuzz `open`, file upstream PRs (password
  normalization, warning coverage, text-layer findings to #1049), perf pass
  (parallel prefetch in the viewer now that the engine allows it).
- **M4 — GPU (optional, unscheduled).** A `hayro_interpret::Device` emitting
  a vello scene (`vello_hybrid`: CPU geometry, GPU fills). Deferred: nothing
  is CPU-raster-bound once the global lock is gone, and the viewer is
  texture-upload-bound anyway. Revisit if a consumer hits a real wall — or
  for the fun of it.

## 7. Non-goals

- Editing, form filling, signing, accessibility trees.
- JavaScript execution (low single-digit % of benign PDFs; doesn't affect
  static page rendering — appearance streams are pre-baked) and XFA (<1%,
  deprecated in PDF 2.0). Worth a `log::warn!` on detection someday.
- C ABI / FFI, bevy adapters, publishing to crates.io — this is junk-libs
  infrastructure for our own tools. The highest-leverage public contribution
  is upstreaming fixes to hayro, not wrapping it.
