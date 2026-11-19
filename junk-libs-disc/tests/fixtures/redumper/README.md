# Redumper real-package fixtures

The ignored `real_*` tests run against real redumper packages kept outside
Git. Point each environment variable at the package prefix (the path without
an extension), then run the tests with `--ignored`. Never add `.scram`, track
BIN or other copyrighted payload files to this repository, and do not use a
failed or partial dump as a fixture.

## `JUNK_LIBS_REDUMPER_PREFIX`: build 709 mixed-mode package

Used by `real_fixture_log_matches_build_709_oracle` and
`real_fixture_raw_structure_matches_build_709_oracle`. A mixed-mode
PlayStation disc dumped with redumper build 709:

- drive read offset `+6`, disc write offset `+2`
- `.scram` 522,449,736 bytes; `.state` 130,612,434 bytes
- `.subcode` 21,324,480 bytes (222,130 sectors); `.toc` 44 bytes
- split tracks whose sizes and hashes match the Redump DAT

## `JUNK_LIBS_REDUMPER_AUDIO_PREFIX`: build b736 audio CD

Used by `real_audio_fixture_matches_redumper_boundaries_and_subchannel_metadata`
and `real_audio_raw_matches_split_bin_on_the_recording_timeline`. An 18-track
audio CD dumped with redumper build b736:

- drive read offset `+6`, disc write offset `0`
- MCN `4988601471916` and 18 ISRCs
- `.subcode` covering 313,484 sectors; `.scram`/`.state` six frames short of
  `313,484 * 588`
- a multi-BIN CUE where each BIN after track 1 begins at its INDEX 00
- INDEX 01-to-next-INDEX 01 track streams that match AccurateRip
  (v2, confidence 8)

The tests check the `final TOC` INDEX 01 LBAs and lead-out from the log
against both the CUE layout and the raw TOC.
