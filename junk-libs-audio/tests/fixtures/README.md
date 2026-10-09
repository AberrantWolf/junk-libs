# Independent FLAC vector

ramp-1001.flac is a generated stereo signed 16-bit ramp, 1001 frames at 44100 Hz.
Frame i has left=i-500 and right=500-i. The WAV input was generated directly from
this formula and encoded with the installed FFmpeg FLAC encoder, independently of
Symphonia. It contains no third-party recorded media. Tests check every decoded
sample, exact seeking and truncated input. Sequence tests separately construct WAV
bytes and compare split versus contiguous samples through the shared converter.
