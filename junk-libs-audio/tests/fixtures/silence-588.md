588 stereo frames of zero signed 16-bit little-endian PCM at 44100 Hz, encoded with FFmpeg/libopus:

```
ffmpeg -f s16le -ar 44100 -ac 2 -i silence.pcm -c:a libopus -frame_duration 20 -application audio -b:a 128k -f opus silence-588.opus
```

Per RFC 7845 the result has 960 coded frames, final granule 952, pre-skip 312, 640 playable frames and an end trim of 8. Tests depend on these values.
