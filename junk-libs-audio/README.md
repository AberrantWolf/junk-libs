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

## Ogg Opus decoding — 2026-11-19

`decode-opus` registers a libopus-backed codec with `AudioDecoder`'s codec
registry. Symphonia 0.5's Ogg reader does not apply Opus pre-skip, and on
single-page streams it trims the start rather than the end. So Ogg Opus is
read with Symphonia gapless trimming off, and `AudioDecoder` applies RFC 7845
timing from its own bounded scan: the `OpusHead` on the first page and the
final page's granule position. That timing covers pre-skip, end trim, Q7.8
output gain, `total_frames = granule - pre-skip`, and seeks that decode at
least 3840 frames before the target. Mapping families other than 0 fail with
`UnsupportedOpusChannelMapping`; chained or multiplexed streams fail with
`UnsupportedOggLayout`. Without the feature, Opus fails with `UnsupportedCodec`.
`tests/opus.rs` encodes its fixtures with the bundled libopus and compares
the output against a reference libopus decode.

