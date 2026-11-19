# junk-libs-audio

Headless audio decoding over caller-supplied byte sources. Positions and
lengths are in PCM frames; samples are interleaved `f32`.

## Features

No features are enabled by default.

| Feature | Adds |
|---|---|
| `decode-wav` | `AudioDecoder` / `WavDecoder` for PCM WAV. |
| `decode-compressed` | Symphonia codecs (FLAC, MP3, AAC, …), `inspect_file_audio`, `inspect_file_audio_metadata`. |
| `decode-opus` | Ogg Opus (`.opus`, or Opus in `.ogg`) via bundled libopus. Needs a C toolchain and CMake. |
| `disc-readers` | `CuePcmDecoder` and `RedumperPcmDecoder` adapters over `junk-libs-disc` readers. |
| `resample` | `SincResampler`; with `decode-compressed`, `decode_sequence` converts a run of sources to stereo at one output rate, keeping resampler phase across compatible neighbours. |
| `derivative-validation` | `validation::validate_flac` (integer-PCM comparison) and `validation::inspect_opus`. |

`apply_gain` and `hard_clip` are always available.

## Decoding

`AudioDecoder::open` takes a `ByteSource` (any `Read + Seek + Send` plus an
optional extension/media-type hint) and exposes:

- `format()` and `total_frames()` — sample rate, channels and the playable
  length after known encoder delay and padding are trimmed.
- `read_frames(max)` — the next chunk, or `None` at the end. A stream that
  ends before its declared length is an error, not a clean end.
- `seek_exact(frame)` — the next read starts exactly at `frame`. Formats that
  cannot seek exactly return `ExactSeekUnavailable` rather than landing nearby.

### Ogg Opus

Pre-skip, end trim to the final granule position, header output gain and
seek pre-roll (80 ms) follow RFC 7845. Only channel mapping family 0
(mono/stereo) is supported; other families fail with
`UnsupportedOpusChannelMapping`, and chained or multiplexed streams fail with
`UnsupportedOggLayout`. Without the feature, Opus input fails with
`UnsupportedCodec`.

### MP3

MP3 uses a vendored copy of `symphonia-bundle-mp3` 0.5.5 with one upstream fix
backported (see [BACKPORT.md](../vendor/symphonia-bundle-mp3/BACKPORT.md)).
Decoding and seeking are limited to the audio between any leading ID3v2 tag
and trailing APEv2/ID3v1 tags.

## File inspection

`inspect_file_audio` decodes a whole FLAC or MP3 file and returns its native
format, exact decoded frame count, known trims, descriptive tags and embedded
pictures. `inspect_file_audio_metadata` returns the same tags and pictures
without decoding the audio. Both check tag lengths before allocating and call the
caller's checkpoint callback so long reads can be cancelled.

Tag precedence for MP3 is ID3v2, then APEv2, then ID3v1. Encoded bytes are
never rewritten.
