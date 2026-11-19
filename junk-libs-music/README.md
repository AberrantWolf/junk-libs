# junk-libs-music

Audio fingerprinting, MusicBrainz/AcoustID response decoding and release
matching. The crate performs no network I/O, owns no paths or database, and
never writes metadata.

## Features

- `fingerprint` — bounded, cancellable Chromaprint (Test2) fingerprints of
  mono/stereo FLAC and MP3 from a caller-supplied `ByteSource`. The first 120
  seconds are analyzed; the reported duration covers the whole file (up to 24
  hours). `compute_window` decodes only the analyzed window when the total
  length is already known. Very short audio may yield no searchable fingerprint
  (`AudioFingerprint::usable`).
- `musicbrainz` — MusicBrainz URL construction and release decoding through
  `musicbrainz_rs` entity models, plus AcoustID response interpretation.
  Transport, rate limiting, credentials and caching are the caller's job.

## Matching

`matching::compare` and `matching::rank` assign local tracks to release
positions using recording evidence only: a maximum one-to-one assignment, with
missing coverage, ambiguity and duration conflicts reported. Disc/position hints
only order existing candidates. A tracklist is complete only when medium and
track positions are contiguous; this is re-checked when an `AlbumMatch` is
deserialized. Inputs are bounded by `MAX_TRACKS`, `MAX_RELEASES` and
`MAX_RECORDINGS_PER_TRACK`.

## Dependencies

`chromaprint-next` is a git dependency pinned to a revision that correctly
handles stereo frames split across feed calls (the published 0.1.0 does not).

## License

The crate source is MIT. Enabling `fingerprint` links LGPL-2.1-or-later
chromaprint-next, so the package is declared `MIT AND LGPL-2.1-or-later`;
applications must meet that license's distribution obligations.
