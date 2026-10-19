# junk-libs-audio


## Bounded purchased-file inspection — 2026-10-19

`inspect_file_audio` (decode-compressed) validates and decodes FLAC/MP3 from a
caller-authorized reader with cooperative checkpoints, returning native format,
exact consumed frames, known trims, bounded descriptive tags and encoded pictures.
`AudioDecoder.inspection` exposes native metadata for the ordinary playback path;
probing checks hostile internal metadata lengths before parser allocation.
Collection grouping, identities, privacy, provenance, Assets and publication stay
with callers. This is structural/decode inspection, not catalog verification.

Independent vectors in `tests/purchases.rs` and `tests/fixtures/purchases/README.md`
cover the initial Collector FLAC/MP3 matrix, tag versions, pictures, malformed
lengths/truncation and cancellation. The coordinated consumers are Collector and
Trash Amp; neither needs a CDDA conversion for these originals. No additional
codec/gapless qualification is implied. All-feature workspace tests, strict
all-target/all-feature Clippy and formatting passed for this delivery.
