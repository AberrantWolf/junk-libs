# junk-libs-egui-pdfdoc

Ready-made `PageModel` implementations for `junk-libs-egui-docview`, backed by
`junk-libs-platen` for PDFs and `image` for raster files. Regions are plain
geometry (page points, top-left origin); per-region application data stays in
the host, keyed by `RegionId`.

## Document models

- `Document` — eager: renders every page and its text layer on open.
- `LazyDocument` — for large files. Opening reads only source bytes, page sizes
  and page labels. Pages render on demand:
  - `rerender_page` requests a page; one worker runs at a time and newer
    requests replace a pending one. Call `poll()` from the host update loop and
    repaint while `is_rendering()` is true.
  - The page cache is bounded by `CacheLimits` (default three pages and 64 MiB of
    bitmap plus character data), evicting least recently requested pages. Large
    rasters are downscaled to fit; a page whose text alone exceeds the budget is
    an error. Source bytes, GPU textures and export work are outside the bound.
  - `region_input` captures a region and its source so `resolve` can produce a
    fixed-scale crop and the selected text on a worker, independent of viewer
    zoom or cache state.
  - `snapshot` shares the source with an empty cache; `render_page` renders a
    transient page for export.

## Region text

Selected text is rebuilt with `junk_libs_platen::reading_order` from glyph
positions rather than PDF content order: columns, lines, paragraphs, word gaps,
RTL runs and vertical Japanese. This is heuristic for untagged PDFs; complex
tables and mixed orientations may need review. `*_with_direction` variants
override the writing direction when it is ambiguous.
