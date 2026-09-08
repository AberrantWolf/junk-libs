# Disc playback APIs

`cue_audio::CueAudioDisc` builds a playback timeline for standard BINARY CUE sheets. `track_reader(number)` returns signed 16-bit stereo frames with exact frame seeks. Its layout preserves INDEX positions and PRE/DCP flags, distinguishes synthetic PREGAP/POSTGAP lengths from stored gaps, and exposes a separate hidden first-track range. Normal tracks run INDEX 01 to the next audio INDEX 01; an audio track before a data track stops before that data track's stored or synthetic pregap. No data extents can be opened through `range_reader`.

The older `cue`, `layout`, and `pcm::TrackPcmReader` APIs retain their existing layout/hash semantics. Consumers doing playback should use the new API. Nonstandard CDRWin playback is rejected rather than converting away flags or placement directives. The old compatibility and hash APIs remain available.

`redumper::pcm::read_audio_layout` and `RawAudioReader::open` support current `.scram/.state/.subcode/.toc` packages for single-session, two-channel, all-audio discs. They do not need a CUE sheet or split BIN files. A present `.fulltoc` is checked for a single session. Legacy `.scrap`, mixed-mode raw images, multichannel and multisession images are rejected. The format-0 TOC gives INDEX 01 boundaries and the lead-out; raw INDEX 00/subchannel reconstruction remains follow-up work. `.subcode` is structurally validated, not treated as decoded Q metadata.

`RawAudioReader::from_sources` accepts caller-provided `Read + Seek + Send` streams for an explicitly validated audio range. `from_signed_sources` additionally supports negative disc positions for a caller-validated HTOA/pregap range. This lets a future range-backed network source supply bytes without adding transport dependencies to the reader. Neither low-level constructor determines whether its range contains audio: use the validated package constructor when that evidence is not already available.

All new reader positions and lengths use **stereo sample frames**, not bytes, sectors, or individual channel samples. One CD sector contains 588 stereo frames; each frame is four little-endian bytes, left i16 followed by right i16. Each read returns at most 588 frames; a short nonzero read is not EOF. Readers retain bounded buffers, and sequential reads retain buffering instead of seeking for every sector. Seeking to the exact end is legal. Out-of-range seeks, truncated sources, and unknown state values return errors. Read errors leave the logical cursor and caller's output unchanged.

## Redumper format evidence

Behavior was checked against upstream redumper commit `f01a78657c7aa73ae0c82adfee98944445c1b2e1`, using the source as a format reference; no upstream implementation was copied.

- [cd_common.ixx](https://github.com/superg/redumper/blob/f01a78657c7aa73ae0c82adfee98944445c1b2e1/cd/cd_common.ixx): raw origin LBA -45150; `.toc` uses MMC format 0 with LBA addresses, `.fulltoc` uses format 2.
- [cd_dump.ixx](https://github.com/superg/redumper/blob/f01a78657c7aa73ae0c82adfee98944445c1b2e1/cd/cd_dump.ixx): LBA 0 is byte offset `0x6545fa0` in `.scram`; drive read offset is applied while writing the raw timeline.
- [common.ixx](https://github.com/superg/redumper/blob/f01a78657c7aa73ae0c82adfee98944445c1b2e1/common.ixx): `.state` is one byte per stereo frame: 0 skipped, 1 C2 error, 2 successful with C2 disabled, 3 successful with SCSI checking disabled, 4 successful.
- [cd_split.ixx](https://github.com/superg/redumper/blob/f01a78657c7aa73ae0c82adfee98944445c1b2e1/cd/cd_split.ixx): splitting may apply a separate disc-write offset. The raw reader deliberately retains the recording timeline, does not infer offset from file lengths, and never reapplies the drive read offset. Audio is not descrambled.

For a disc-relative stereo frame `f`, the raw byte offset is `(45150 * 588 + f) * 4`. The corresponding state byte offset is `45150 * 588 + f`. A six-frame-short file tail from a +6 drive read offset does not change this origin.

The caller explicitly selects Reject, Silence, or Preserve for states 0/1. States 2/3 remain separately reported; they are not relabeled as fully checked samples. Unknown states always fail. PRE is metadata: these readers do not apply de-emphasis, resampling, normalization, or other effects.

## Validation and remaining work

Synthetic tests cover sparse raw sources, independent channel/sample patterns, raw-versus-BIN playback, cross-track boundaries, non-sector seeks, EOF, repeated seeking, quality-policy behavior, truncated input, malformed TOCs, CUE stored/synthetic gaps, first-track hidden audio, and audio-to-data boundaries. No copyrighted payloads are checked in.

The ignored `real_audio_raw_matches_split_bin_on_the_recording_timeline` test uses the existing Octopath b736 fixture described in `tests/fixtures/redumper/README.md`. It requires `JUNK_LIBS_REDUMPER_AUDIO_PREFIX` and verifies every sample of all 18 tracks. That fixture's known disc-write offset is zero; a nonzero offset must never be hidden by an automatically adjusted oracle. This new full comparison has not yet been run because the fixture path has not been supplied.

Remaining raw work: CRC-validated Q/index reconstruction, raw HTOA range discovery, and safe mixed-mode/multisession layout support (including disc-write/variable-offset boundaries). Those cases fail explicitly or require a caller-validated low-level range; they are not advertised as supported automatic playback. Real-dump and captured-device gaplessness validation remain necessary before release.
