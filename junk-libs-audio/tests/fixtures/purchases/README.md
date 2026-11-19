# Generated FLAC and MP3 vectors

Generated material; no recordings.

FLAC files (`flac-{rate}-{bits}-{channels}.flac`) were encoded with `flac` from
little-endian signed PCM `int(sin(frame / 7) * 2**(bits - 2))`, with the
negated signal in channel 2, and exactly `rate / 100` frames. Rates 44100,
48000, 88200, 96000, 176400 and 192000; 16/24-bit; mono/stereo.

MP3 files (`mp3-{rate}-{cbr,vbr,no-xing}[-stereo].mp3`) were encoded with
FFmpeg/libmp3lame from 0.2-second mono and stereo tones at 8000, 16000 and
44100 Hz (MPEG-2.5, MPEG-2, MPEG-1), as CBR, VBR and without a Xing header.
Each carries the ID3 title `音楽`. Tests build further ID3v1, ID3v2 and APEv2
tags around these payloads.
