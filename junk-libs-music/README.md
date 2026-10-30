# junk-libs-music

Shared mechanisms for Collector and Trash Amp. No HTTP client, paths, database,
UI or automatic metadata acceptance is included.

Enable `fingerprint` for bounded, cancellable mono/stereo FLAC/MP3 decoding and
Chromaprint Test2 fingerprints over the first 120 seconds. Duration and decoded
coordinates cover the complete file (maximum 24 hours). Consumers supply a
bounded cancellable `ByteSource`; short audio can produce no searchable fingerprint.
The exact corrected chromaprint-next revision is pinned because published 0.1.0
mishandles stereo frames split across feed calls. Chromaprint compatibility does
not establish recording or release identity.

Enable `musicbrainz` for MusicBrainz URL construction and decoding through
musicbrainz_rs 0.14 entity models, plus bounded AcoustID response interpretation.
Transport, contact headers, rate limits, credentials, caching and durable work
remain consumer responsibilities. URL query pairs avoid the community API
builder's ampersand escaping defect.

`matching::rank` requires recording evidence for each edge, finds a maximum
one-to-one assignment and reports missing coverage, ambiguity and duration
conflicts. Hints only order existing edges. The ranking never authorizes metadata
writes or calls a candidate search exhaustive. Matching is bounded to 256 files,
128 releases and 16 recording suggestions per file. Provider decoding also
supports larger legacy release lists; callers must cap matching candidates.

The wrapper source is MIT; enabling fingerprinting includes LGPL-2.1-or-later
chromaprint-next, so this package declares `MIT AND LGPL-2.1-or-later`. Retain the
upstream license and comply with its distribution obligations in applications.

Run `cargo test -p junk-libs-music --all-features` and package Clippy with all
features/targets. Real generated FLAC/MP3 fixtures exercise decoding, cancellation,
truncation and inconsistent lengths. Provider and assignment tests are offline.

## Review follow-up — 2026-10-30

Completeness requires contiguous medium/track positions, and duplicate or zero
medium positions fail closed. Provider decoding rejects malformed track identities,
positions and text, and successful AcoustID envelopes must include a results array.
An explicitly empty track credit falls back to the recording credit. Searchability
also requires a valid fingerprint envelope. Exhaustive independent enumeration of
all 512 three-by-three recording graphs checks cardinality and ambiguity.
