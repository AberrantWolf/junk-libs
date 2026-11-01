# Symphonia MP3 zero-length channel correction

Source: crates.io symphonia-bundle-mp3 0.5.5 (MPL-2.0, original notices retained).
One production change: backport upstream PR #528, commit
9668f8cfd93213f29ae74d29981deee1c7b8e455:
https://github.com/pdeljanov/Symphonia/pull/528
Allow the bit reader to start at the reservoir end. Empty channels then decode
without skipping frames; nonempty reads still fail on missing bits.

The shared audio adapter selects this decoder for MP3. Other codecs and the
Symphonia 0.5 API remain unchanged. Remove this backport when the shared adapter
moves to a Symphonia release containing the correction.
