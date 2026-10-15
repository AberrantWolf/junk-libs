# PDF document models

`Document` retains the existing eager API. `LazyDocument` is an additive option
for large files and implements the same `PageModel` interface:

- Opening reads source bytes, displayed page dimensions and page labels, without
  allocating any PDF page rasters or extracting their text.
- `rerender_page` requests a page. One worker runs at a time and a single pending
  request is replaced by newer navigation/zoom requests. Poll `poll()` in the host
  update loop and request repaint while `is_rendering()` is true.
- The cache defaults to at most three pages and 64 MiB of bitmap/character-cell
  data. Least recently requested pages are evicted. Large rasters are downscaled
  to reserve budget for text; a page whose text still exceeds the budget returns
  an error. Encoded source bytes, metadata, the active render/parser, host GPU
  textures and standalone export/translation workers are outside that cache bound.
- `region_input` captures an immutable source and region. Resolve it on a worker
  at a fixed scale to obtain both a crop and selected source text, independently
  of viewer zoom and whether the page is cached. `resolve_with_direction` supports
  overrides for ambiguous writing directions.
- `snapshot` shares source bytes/image pixels and copies geometry, with an empty
  cache and no pending render. `render_page` streams transient pages for export.

Selection reconstructs text with platen's spatial reading-order helper instead
of relying on PDF content emission order. It separates continuous columns,
lines, paragraphs and inferred word gaps, with RTL runs and vertical Japanese.
Automatic reconstruction of untagged PDFs is heuristic: complex tables, mixed
writing orientations and unusual positioned/ligature text still need source
review. Direction overrides resolve orientation ambiguity, not arbitrary layouts.

Regions remain geometry only, measured in page points with a top-left origin.
Application translation results/styles remain in the host, keyed by RegionId.
