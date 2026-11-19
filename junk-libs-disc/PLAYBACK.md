# Disc playback APIs

## Reader contract

`cue_audio::CueAudioReader`, `redumper::pcm::RawAudioReader` and
`pcm::TrackPcmFrameReader` all implement `pcm::CdPcmReader`.

- Positions and lengths are **stereo sample frames**, not bytes, sectors or
  channel samples. One CD sector holds 588 frames; each frame is four
  little-endian bytes, left `i16` then right `i16`.
- `read_frames` may return fewer frames than requested; a short nonzero read is
  not end of stream. Zero frames (for a nonempty buffer) means end of range.
- `seek_frame` to exactly `total_frames()` is legal; beyond it is an error.
- Truncated sources and out-of-range seeks return errors; missing samples are
  never padded. A failed `CueAudioReader` or `RawAudioReader` read leaves the
  cursor and the caller's buffer unchanged.
- Readers never apply de-emphasis, resampling or normalization. `PRE` is
  reported as metadata only.

## CUE playback: `cue_audio::CueAudioDisc`

`CueAudioDisc::open` builds a playback timeline for standard `BINARY` CUE
sheets. CDRWin-style sheets and unknown `FLAGS` (for example `4CH`) are
rejected rather than converted, because they can change audio placement.

- Each track records its INDEX positions, `PRE`/`DCP` flags, and synthetic
  `PREGAP`/`POSTGAP` lengths. Synthetic gaps play as silence; stored INDEX 00
  gaps play from the file.
- `track_reader(n)` spans INDEX 01 to the next audio track's INDEX 01, so it
  includes its own postgap and the following track's pregap. An audio track
  followed by a data track stops before that data track's pregap.
- `hidden_audio` is the first-track audio before INDEX 01 (hidden track one
  audio). It is not part of track 1; read it with `range_reader`.
- `range_reader` rejects any range that touches a data track.

`CueAudioDisc::from_text` builds the same layout from CUE text and a callback
giving each FILE's byte length, without opening anything. Read it with
`range_reader_with`, which asks a callback for a seekable source per CUE FILE
name. Only files overlapping the range are opened, so data tracks never are.
Each source must still have its recorded length. `open`/`range_reader` are the
filesystem versions of these.

`pcm::PcmSequenceReader` plays several `CdPcmReader`s back to back as one
timeline, for example a run of tracks converted together.

## Sector and hash APIs: `cue`, `layout`, `pcm::TrackPcmReader`

These describe tracks in absolute sectors (lead-in of 150 applied) with
INDEX 01-to-next-INDEX 01 lengths, the convention used for DiscID and
AccurateRip. Synthetic gaps are not included. CDRWin sheets are converted
automatically. `TrackPcmFrameReader` gives exact frame seeking over a
`TrackPcmReader`. Use `cue_audio` for playback.

## Redumper raw packages: `redumper::pcm`

`read_audio_layout` and `RawAudioReader::open` read current redumper
`.scram`/`.state`/`.subcode`/`.toc` packages without a CUE or split BIN files.
Supported: single-session discs where every track is two-channel audio. A
`.fulltoc`, when present, must describe a single session. Legacy `.scrap`
packages, data tracks, four-channel audio and multisession discs are rejected.

Track boundaries come from the format-0 TOC (INDEX 01 positions and lead-out);
INDEX 00 is not reconstructed from subchannel. `.subcode` is only checked for
length.

`RawAudioReader::from_sources` takes caller-supplied `Read + Seek + Send`
streams for a range the caller has already established is audio.
`from_signed_sources` also accepts negative disc frames, for a validated
hidden-track/pregap range before LBA 0.

### Sample quality

Each read returns a `SampleQuality` with per-state counts. For states 0
(skipped) and 1 (C2 error) the caller picks an `InvalidSamplePolicy`:
`Reject` fails the read, `Silence` outputs zeros, `Preserve` outputs the stored
samples. States 2 (C2 disabled) and 3 (SCSI checks disabled) are delivered but
counted separately from 4 (fully verified). Any other state value is an error.

### Format notes

Format reference: the [redumper source](https://github.com/superg/redumper).
No upstream code is copied.

- The raw timeline starts at LBA -45150. For disc frame `f`, the `.scram` byte
  offset is `(45150 * 588 + f) * 4` and the `.state` byte offset is
  `45150 * 588 + f`.
- `.state` holds one byte per stereo frame: 0 skipped, 1 C2 error, 2 read with
  C2 disabled, 3 read with SCSI checks disabled, 4 verified.
- `.toc` is an MMC READ TOC format 0 response with LBA addresses; `.fulltoc`
  is format 2.
- The drive read offset is already applied in `.scram`. A disc write offset
  (which redumper's split may apply) is **not** applied: the reader keeps the
  recording timeline and never infers an offset from file lengths. Audio is
  not descrambled. A `+6` read offset leaves the files six frames short of
  `.subcode` sectors × 588; the origin is unchanged.

## Tests

Synthetic tests cover the contracts above; no copyrighted payloads are checked
in. The ignored test
`real_audio_raw_matches_split_bin_on_the_recording_timeline` compares every
sample of every track against redumper's split BINs. Set
`JUNK_LIBS_REDUMPER_AUDIO_PREFIX` as described in
`tests/fixtures/redumper/README.md`. The comparison is only valid for a
package with a zero disc write offset, which the test asserts.
