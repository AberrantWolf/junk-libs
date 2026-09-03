# Redumper validation fixture

The Redumper parser and raw-package checks are validated against a real,
known-good build 709 package stored outside Git. The package is the same
mixed-mode PlayStation dump used by retro-junk's optical-disc validation
fixture. Its independently checked properties are:

- drive: `HL-DT-ST BD-RE BU40N`, firmware `1.00`
- drive read offset: `+6` four-byte stereo sample frames
- disc write offset: `+2` four-byte stereo sample frames
- `.scram`: 522,449,736 bytes
- `.state`: 130,612,434 bytes (one state per `.scram` sample frame)
- `.subcode`: 21,324,480 bytes (222,130 raw 96-byte subchannel frames)
- `.toc`: 44 bytes
- four split tracks whose size, CRC32, MD5, and SHA1 match both the
  Redumper log and the independent Redump DAT oracle

Set `JUNK_LIBS_REDUMPER_PREFIX` to the raw package prefix without an
extension, then explicitly run the two ignored `real_fixture_*` tests.
The test source includes a small exact, non-content-bearing excerpt of the
log so ordinary CI also guards the real build 709 grammar.

Do not add raw `.scram`, track BIN, or other copyrighted payload files to
this repository. A package found under a directory named `failed-dumps` is
not positive evidence and must not be used as the correctness oracle.

## Audio-CD layout oracle

The `real_audio_fixture_*` test is validated against an 18-track audio-CD
package produced by Redumper build b736. Set
`JUNK_LIBS_REDUMPER_AUDIO_PREFIX` to its package prefix without an extension.
Its independently cross-checked properties include:

- the `final TOC` INDEX 01 LBAs and lead-out recorded in the Redumper log
- 18 per-track BIN files whose lengths reconstruct those same boundaries
- Redumper's multi-BIN convention in which each BIN after track 1 begins at
  INDEX 00 and INDEX 01 marks the audible track start
- MCN `4988601471916` and 18 CUE-form ISRC entries
- `.subcode` covering 313,484 sectors and `.scram`/`.state` carrying six
  fewer stereo sample frames than `313,484 * 588`, matching this drive's
  reported `+6` read offset
- a `.toc` whose SCSI response length header agrees with its file length

The reconstructed INDEX 01-to-next-INDEX 01 PCM streams were also checked
against the live AccurateRip record for IDs
`018-0027c722-020f8093-e70df812`. All 18 tracks matched the primary database
checksum as AccurateRip v2, at confidence 8. This establishes the gap
ownership and PCM byte order for this fixture. It does not establish that
Redumper's individual split BIN files can be encoded independently: later
BINs begin with their own INDEX 00 regions and must be reassembled according
to the CUE boundaries first.

Gap boundaries are taken from subchannel-Q index information, not inferred
from PCM silence. This matches EAC's documented gap detection and XLD's
CRC-checked subchannel-Q implementation:

- <https://www.exactaudiocopy.de/gap-technology/>
- <https://sourceforge.net/p/xld/code/HEAD/tree/trunk/XLD/XLDCDDABackend.c>

The package has no `.cdtext` sidecar, and its CUE contains no TITLE or
PERFORMER directives. It is therefore evidence for audio layout and Redumper
log parsing, not for the CD-TEXT binary parser.
