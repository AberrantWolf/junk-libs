# Independent purchase vectors — 2026-10-19

Generated test material, no purchased or copyrighted recordings. FLAC files were
encoded with the independent `flac` encoder from little-endian signed PCM:
`int(sin(frame / 7) * 2**(bits - 2))`, with the negated signal in stereo channel 2,
and exactly `rate / 100` frames. The tests check native properties and sought PCM
against that independent formula. Rates: 44100, 48000, 88200, 96000, 176400, 192000;
16/24-bit, mono/stereo.

MP3 files were independently encoded using ffmpeg/libmp3lame from a 0.2-second
mono and stereo tones at 8000, 16000, and 44100 Hz (MPEG-2.5, MPEG-2, MPEG-1), in CBR, VBR,
and without Xing headers. Unicode title `音楽` is encoded in ID3. Tests construct
independent ID3v1 and ID3v2.2/2.3/2.4 envelopes over these real MPEG payloads,
including whole-tag unsynchronization, PIC/APIC and hostile lengths/counts.
Known encoder trims are tested separately from headerless decoded duration;
there is no sample-accurate/gapless playback qualification claim for MP3.
