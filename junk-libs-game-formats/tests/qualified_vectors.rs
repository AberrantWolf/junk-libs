use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use junk_libs_core::AnalysisError;
use junk_libs_game_formats::*;

fn formula(length: usize, multiplier: usize, addend: usize) -> Vec<u8> {
    (0..length)
        .map(|index| ((index * multiplier + addend) % 256) as u8)
        .collect()
}

#[test]
fn published_raw_vector_is_independent_of_reader_chunks() {
    let expected = hash_raw(&mut Cursor::new(b"123456789".to_vec())).unwrap();
    let mut chunked = Chunked::new(b"123456789".to_vec(), 2);
    let actual = hash_raw(&mut chunked).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.crc32, "cbf43926");
    assert_eq!(actual.sha1, "f7c3bc1d808e04732adf679965ccc34ca7ae3441");
    assert_eq!(
        actual.sha256,
        "15e2b0d3c33891ebb0f1ef609ec419420c20e320ce94c65fbc8c3312448eb225"
    );
    assert_eq!(actual.domain, RAW_DOMAIN);
}

#[test]
fn trainer_free_ines_and_nes2_hash_only_exact_declared_payloads() {
    let payload = formula(16 * 1024, 17, 3);
    let mut ines = vec![0; 16];
    ines[..4].copy_from_slice(b"NES\x1a");
    ines[4] = 1;
    ines.extend_from_slice(&payload);
    let (layout, hashes) = hash_ines_payload(&mut Cursor::new(ines.clone())).unwrap();
    assert_eq!(layout.version, InesVersion::INes);
    assert_eq!(hashes.bytes, 16 * 1024);
    assert_eq!(
        hashes.sha256,
        "3e2940176a2e2ff15403217462625c80949c1589415ecd70bef434a2662374d6"
    );

    let mut nes2 = ines;
    nes2[7] = 0x08;
    let (layout, hashes2) = hash_ines_payload(&mut Cursor::new(nes2)).unwrap();
    assert_eq!(layout.version, InesVersion::Nes2);
    assert_eq!(hashes2, hashes);
}

#[test]
fn nes2_exponent_multiplier_is_bytes_and_bad_lengths_fail_closed() {
    let mut rom = vec![0; 16];
    rom[..4].copy_from_slice(b"NES\x1a");
    rom[4] = (14 << 2) | 1; // 2^14 * 3 bytes = 48 KiB.
    rom[7] = 0x08;
    rom[9] = 0x0f;
    rom.extend(vec![0x5a; 3 * 16 * 1024]);
    assert_eq!(
        inspect_ines(&mut Cursor::new(rom.clone()))
            .unwrap()
            .prg_rom_bytes,
        49_152
    );

    rom.pop();
    assert!(matches!(
        inspect_ines(&mut Cursor::new(rom)),
        Err(AnalysisError::InvalidFormat(_))
    ));
}

#[test]
fn trainer_misc_rom_trailing_and_truncated_ines_are_not_qualified() {
    let payload = formula(16 * 1024, 17, 3);
    let mut trainer = vec![0; 16];
    trainer[..4].copy_from_slice(b"NES\x1a");
    trainer[4] = 1;
    trainer[6] = 0x04;
    trainer.extend(vec![0x55; 512]);
    trainer.extend_from_slice(&payload);
    assert!(matches!(
        hash_ines_payload(&mut Cursor::new(trainer)),
        Err(AnalysisError::UnsupportedVariant(_))
    ));

    let mut misc = vec![0; 16];
    misc[..4].copy_from_slice(b"NES\x1a");
    misc[4] = 1;
    misc[7] = 0x08;
    misc[14] = 1;
    misc.extend_from_slice(&payload);
    assert!(matches!(
        hash_ines_payload(&mut Cursor::new(misc)),
        Err(AnalysisError::UnsupportedVariant(_))
    ));

    let mut malformed = vec![0; 16];
    malformed[..4].copy_from_slice(b"NES\x1a");
    malformed[4] = 1;
    malformed.extend_from_slice(&payload[..payload.len() - 1]);
    assert!(hash_ines_payload(&mut Cursor::new(malformed)).is_err());
}

#[test]
fn all_n64_orders_normalize_across_hostile_chunk_boundaries() {
    let canonical = hex("803712400102030410203040aabbccdd");
    let cases = [
        (canonical.clone(), N64Format::Z64),
        (hex("378040120201040320104030bbaaddcc"), N64Format::V64),
        (hex("401237800403020140302010ddccbbaa"), N64Format::N64),
    ];
    for (input, expected_format) in cases {
        for chunk in 1..=7 {
            let (format, hashes) = hash_n64(&mut Chunked::new(input.clone(), chunk)).unwrap();
            assert_eq!(format, expected_format);
            assert_eq!(hashes.domain, N64_BIG_ENDIAN_DOMAIN);
            assert_eq!(
                hashes.sha256,
                "82db1a4c0cd44067c3fe9c438c0c3e4bffaed4106f9cb0a23df968f4d6c309d1"
            );
        }
    }
    let mut ragged = canonical;
    ragged.push(0);
    assert!(hash_n64(&mut Cursor::new(ragged)).is_err());
}

#[test]
fn explicit_snes_and_pce_layouts_never_rewrite_the_master() {
    let snes = formula(32 * 1024, 5, 11);
    let mut snes_headered = formula(512, 3, 9);
    snes_headered.extend_from_slice(&snes);
    let original = snes_headered.clone();
    let hashes =
        hash_snes_payload(&mut Cursor::new(snes_headered), CopierHeader::Present512).unwrap();
    assert_eq!(
        hashes.sha256,
        "f15a68d8106c43274943b31a148cd0f6a9cca229631d4374574786c1ec68e5cd"
    );
    assert_eq!(original[..512], formula(512, 3, 9));
    assert!(hash_snes_payload(&mut Cursor::new(original), CopierHeader::Absent).is_err());

    let pce = formula(8 * 1024, 13, 7);
    let mut pce_headered = formula(512, 19, 2);
    pce_headered.extend_from_slice(&pce);
    let hashes =
        hash_pce_payload(&mut Cursor::new(pce_headered), CopierHeader::Present512).unwrap();
    assert_eq!(
        hashes.sha256,
        "75f7effae2621302b632af488f0e7339e02258acbf917e39fcff0aa39a6960a6"
    );
}

#[test]
fn cue_hashes_every_raw_track_and_distinguishes_stored_from_synthetic_pregap() {
    let mut files = BTreeMap::from([(
        "disc.bin".to_string(),
        [vec![0x11; 2 * 2352], vec![0x22; 2 * 2352]].concat(),
    )]);
    let stored = "FILE \"disc.bin\" BINARY\n\
                  TRACK 01 MODE1/2352\nINDEX 01 00:00:00\n\
                  TRACK 02 AUDIO\nINDEX 00 00:00:02\nINDEX 01 00:00:03\n";
    let tracks = cue_hash(stored, &files).unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].hashes.bytes, 2 * 2352);
    assert_eq!(tracks[1].hashes.bytes, 2 * 2352);
    let first = tracks[0].hashes.sha256.clone();
    let second = tracks[1].hashes.sha256.clone();

    files.get_mut("disc.bin").unwrap()[3 * 2352] ^= 0xff;
    let altered = cue_hash(stored, &files).unwrap();
    assert_eq!(altered[0].hashes.sha256, first);
    assert_ne!(altered[1].hashes.sha256, second);

    let synthetic = "FILE \"disc.bin\" BINARY\n\
                     TRACK 01 MODE1/2352\nINDEX 01 00:00:00\n\
                     TRACK 02 AUDIO\nPREGAP 00:00:01\nINDEX 01 00:00:02\n";
    let tracks = cue_hash(synthetic, &files).unwrap();
    assert_eq!(tracks[1].source_byte_offset, 2 * 2352);
    assert_eq!(tracks[1].synthetic_pregap_frames, 1);
}

#[test]
fn cue_rejects_cooked_missing_reordered_and_mismatched_members() {
    let cooked = "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
    let files = BTreeMap::from([("disc.bin".to_string(), vec![0; 2048])]);
    assert!(matches!(
        cue_hash(cooked, &files),
        Err(AnalysisError::UnsupportedVariant(_))
    ));

    let missing = "FILE \"missing.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n";
    assert!(cue_hash(missing, &files).is_err());

    let reordered = "FILE \"disc.bin\" BINARY\n\
                     TRACK 02 MODE1/2352\nINDEX 01 00:00:00\n\
                     TRACK 01 AUDIO\nINDEX 01 00:00:01\n";
    let files = BTreeMap::from([("disc.bin".to_string(), vec![0; 2 * 2352])]);
    assert!(cue_hash(reordered, &files).is_err());

    let raw = "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n";
    let declared = BTreeMap::from([("disc.bin".to_string(), vec![0; 2352])]);
    let result = hash_cue_raw_2352(
        raw,
        |_| Ok(2352),
        |name| {
            let mut bytes = declared
                .get(name)
                .cloned()
                .ok_or_else(|| AnalysisError::invalid_format("missing"))?;
            bytes.pop();
            Ok(Box::new(Cursor::new(bytes)))
        },
    );
    assert!(matches!(result, Err(AnalysisError::InvalidFormat(_))));

    let result = hash_cue_raw_2352(
        raw,
        |_| Ok(2352),
        |_| Ok(Box::new(Cursor::new(vec![0; 2 * 2352]))),
    );
    assert!(matches!(result, Err(AnalysisError::InvalidFormat(_))));
}

fn cue_hash(
    cue: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<CueTrackHashes>, AnalysisError> {
    hash_cue_raw_2352(
        cue,
        |name| {
            files
                .get(name)
                .map(|bytes| bytes.len() as u64)
                .ok_or_else(|| AnalysisError::invalid_format(format!("missing {name}")))
        },
        |name| {
            files
                .get(name)
                .cloned()
                .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn junk_libs_core::ReadSeek>)
                .ok_or_else(|| AnalysisError::invalid_format(format!("missing {name}")))
        },
    )
}

fn hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

struct Chunked {
    inner: Cursor<Vec<u8>>,
    maximum: usize,
}

impl Chunked {
    fn new(bytes: Vec<u8>, maximum: usize) -> Self {
        Self {
            inner: Cursor::new(bytes),
            maximum,
        }
    }
}

impl Read for Chunked {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let length = buffer.len().min(self.maximum);
        self.inner.read(&mut buffer[..length])
    }
}

impl Seek for Chunked {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

struct EarlyEof {
    inner: Cursor<Vec<u8>>,
    limit: u64,
}
impl Read for EarlyEof {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.inner.position()) as usize;
        let size = remaining.min(out.len());
        self.inner.read(&mut out[..size])
    }
}
impl Seek for EarlyEof {
    fn seek(&mut self, offset: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(offset)
    }
}
#[test]
fn aligned_early_eof_is_not_a_complete_n64_hash() {
    let mut bytes = vec![0; 16];
    bytes[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
    let mut reader = EarlyEof {
        inner: Cursor::new(bytes),
        limit: 8,
    };
    assert!(hash_n64(&mut reader).is_err());
}
#[test]
fn cue_container_and_index_ambiguity_fail_before_hashing() {
    let invalid = [
        "FILE disc.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE ./disc.bin BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00",
        "FILE disc.bin WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00",
        "FILE disc.bin MOTOROLA\nTRACK 01 AUDIO\nINDEX 01 00:00:00",
        "FILE disc.bin BINARY\nTRACK 01 AUDIO\nINDEX 00 00:00:00",
        "FILE disc.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nINDEX 01 00:00:01",
        "FILE disc.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:04",
        "FILE disc.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:03\nTRACK 02 AUDIO\nINDEX 01 00:00:02",
        "FILE disc.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE disc.bin BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00",
    ];
    for cue in invalid {
        assert!(
            hash_cue_raw_2352(
                cue,
                |_| Ok(4 * 2352),
                |_| -> Result<Cursor<Vec<u8>>, AnalysisError> {
                    panic!("invalid layout opened payload")
                }
            )
            .is_err(),
            "{cue}"
        );
    }
}
