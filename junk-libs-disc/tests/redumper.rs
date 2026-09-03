//! Integration tests for the redumper sidecar module.
//!
//! Synthetic fixtures cover the parser's happy paths and a few known
//! edge cases (EAC sniffer rejection, CUE-only directories, CD-TEXT
//! without a response header). Real-rip fixtures drop into
//! `tests/fixtures/redumper/` — see the README there.

use std::fs;
use std::path::PathBuf;

use junk_libs_disc::{
    read_cue_layout,
    redumper::{self, CdTextBlock, DriveInfo, RedumperLog, Ripper},
};

// ---------------------------------------------------------------------
// find_sidecars + sniffer
// ---------------------------------------------------------------------

#[test]
fn find_sidecars_finds_all_present_variants() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("album");

    for ext in ["cue", "log", "cdtext", "toc", "fulltoc", "subcode", "atip"] {
        fs::write(base.with_extension(ext), b"").unwrap();
    }

    let sc = redumper::find_sidecars(&base.with_extension("cue"));
    assert!(sc.log.is_some(), "log should be found");
    assert!(sc.cdtext.is_some(), "cdtext should be found");
    assert!(sc.toc.is_some(), "toc should be found");
    assert!(sc.fulltoc.is_some(), "fulltoc should be found");
    assert!(sc.subcode.is_some(), "subcode should be found");
    assert!(sc.atip.is_some(), "atip should be found");
}

#[test]
fn find_sidecars_returns_none_for_missing_extensions() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("album");

    // Only the CUE and log exist.
    fs::write(base.with_extension("cue"), b"").unwrap();
    fs::write(base.with_extension("log"), b"redumper v1\n").unwrap();

    let sc = redumper::find_sidecars(&base.with_extension("cue"));
    assert!(sc.log.is_some());
    assert!(sc.cdtext.is_none());
    assert!(sc.toc.is_none());
}

#[test]
fn find_sidecars_handles_basenames_with_embedded_dots() {
    // Regression: basenames containing dots (e.g. rip tools that hash
    // the disc ID into the filename) used to confuse `set_extension`,
    // which rewrites everything after the *last* dot in the stem. A
    // cue named `hP..j.BAR-.cue` would look up sidecars as
    // `hP..j.log` instead of `hP..j.BAR-.log`.
    let td = tempfile::tempdir().unwrap();
    let stem = "hP..j.HpM38lmnMLVXn9WDkjJxc-";
    let cue = td.path().join(format!("{stem}.cue"));
    fs::write(&cue, b"").unwrap();
    for ext in ["log", "cdtext", "toc", "fulltoc", "subcode"] {
        fs::write(td.path().join(format!("{stem}.{ext}")), b"").unwrap();
    }

    let sc = redumper::find_sidecars(&cue);
    assert!(
        sc.log.is_some(),
        "log should be discovered even with embedded dots in the stem"
    );
    assert!(sc.cdtext.is_some());
    assert!(sc.toc.is_some());
    assert!(sc.fulltoc.is_some());
    assert!(sc.subcode.is_some());
    // Discovered paths should point at the real files, not truncated ones.
    assert!(
        sc.log
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&format!("{stem}.log"))
    );
}

#[test]
fn find_sidecars_on_cue_only_is_empty() {
    let td = tempfile::tempdir().unwrap();
    let cue = td.path().join("album.cue");
    fs::write(&cue, b"").unwrap();

    let sc = redumper::find_sidecars(&cue);
    assert!(sc.is_empty());
}

#[test]
fn sniff_distinguishes_redumper_and_eac() {
    let td = tempfile::tempdir().unwrap();
    let rdl = td.path().join("rd.log");
    let eac = td.path().join("eac.log");
    fs::write(&rdl, b"redumper v2024.03.01 build_1\n").unwrap();
    fs::write(
        &eac,
        b"Exact Audio Copy V1.5 from 23. September 2018\n\nEAC extraction logfile\n",
    )
    .unwrap();

    assert_eq!(redumper::sniff_ripper(&rdl).unwrap(), Ripper::Redumper);
    assert_eq!(redumper::sniff_ripper(&eac).unwrap(), Ripper::Eac);
}

#[test]
fn sniff_xld_and_unknown() {
    let td = tempfile::tempdir().unwrap();
    let xld = td.path().join("x.log");
    let unk = td.path().join("u.log");
    fs::write(&xld, b"X Lossless Decoder version 20210113 (153.3)\n").unwrap();
    fs::write(&unk, b"something completely different\n").unwrap();

    assert_eq!(redumper::sniff_ripper(&xld).unwrap(), Ripper::Xld);
    assert_eq!(redumper::sniff_ripper(&unk).unwrap(), Ripper::Unknown);
}

// ---------------------------------------------------------------------
// parse_log
// ---------------------------------------------------------------------

// Exact non-content-bearing excerpt from the provenance-recorded Cryo dump
// documented in tests/fixtures/redumper/README.md (redumper build 709).
const REAL_BUILD_709_LOG_EXCERPT: &str = "\
=== 2026-07-17 20:09:59 ========================================================
redumper (build: 709)

drive information
  path: /dev/sg2
  inquiry: HL-DT-ST - BD-RE BU40N (revision level: 1.00, vendor specific: N003103MODPC1D5923)
  configuration: MTK8B (read offset: +6, C2 shift: 0, pre-gap start: -135, read method: BE, sector order: DATA_C2_SUB)

disc write offset: +2
";

#[test]
fn parse_real_build_709_log_shape() {
    let log = redumper::parse_log_text(REAL_BUILD_709_LOG_EXCERPT);

    assert_eq!(log.version.as_deref(), Some("(build: 709)"));
    assert_eq!(log.rip_date.as_deref(), Some("2026-07-17 20:09:59"));
    assert_eq!(
        log.drive,
        Some(DriveInfo {
            vendor: "HL-DT-ST".into(),
            product: "BD-RE BU40N".into(),
            firmware: Some("1.00".into()),
        })
    );
    assert_eq!(log.read_offset, Some(6));
    assert_eq!(log.disc_write_offset, Some(2));
    assert!(log.mcn.is_none());
    assert!(log.isrcs.is_empty());
    assert!(!log.raw.is_empty());
}

#[test]
fn parse_log_extracts_mcn_from_embedded_cue_block() {
    // Redumper doesn't emit a dedicated `MCN:` line — it inlines the
    // generated CUE sheet into its log via `LOG("CUE [...]:")`, and the
    // MCN appears there as `CATALOG <value>` (see
    // cd_split.ixx:948 + toc.ixx::printCUE in the redumper source). The
    // parser must catch that shape or every redumper-ripped disc stays
    // effectively barcode-less and Discogs never gets dispatched.
    let text = "\
redumper v2024.03.01 build_20240301

2024-03-02 10:00:00

drive: PIONEER - BD-RW BDR-S09 (fw 1.50)
read offset: +6

disc TOC:
  track 01 { audio, pre-emphasis }
    index 01 { LBA: [     0 ..   12345], length:  12346, MSF: 00:00:00-02:46:45 }

CUE [album]:
REM GENRE \"Electronic\"
REM DATE \"1996\"
CATALOG 0727361234567
FILE \"album.bin\" BINARY
  TRACK 01 AUDIO
    ISRC USRC17654321
    INDEX 01 00:00:00

done.
";
    let log = redumper::parse_log_text(text);
    assert_eq!(log.mcn.as_deref(), Some("0727361234567"));
    assert_eq!(log.isrcs.get(&1).map(String::as_str), Some("USRC17654321"));
}

#[test]
fn parse_log_extracts_mcn_in_cue_block_tolerates_quotes_and_spaces() {
    let text = "\
redumper v2024.03.01

CUE [image]:
CATALOG \"0123456789012\"
FILE \"image.bin\" BINARY
";
    let log = redumper::parse_log_text(text);
    assert_eq!(log.mcn.as_deref(), Some("0123456789012"));
}

#[test]
fn parse_log_handles_inline_drive_with_dash() {
    let text = "\
redumper v2024.03.01

2024-03-02 10:00:00

drive: PLEXTOR - PX-712A (fw 1.07)
read offset: -30

MCN: none
";
    let log = redumper::parse_log_text(text);
    assert_eq!(
        log.drive,
        Some(DriveInfo {
            vendor: "PLEXTOR".into(),
            product: "PX-712A".into(),
            firmware: Some("1.07".into()),
        })
    );
    assert_eq!(log.read_offset, Some(-30));
    assert!(log.mcn.is_none(), "MCN 'none' should map to None");
}

#[test]
fn parse_log_without_mcn_or_isrc() {
    let text = "\
redumper v1.0

2024-03-02 10:00:00

drive:
  vendor: LG
  product: BH16NS40

read offset: 0
";
    let log = redumper::parse_log_text(text);
    assert_eq!(log.read_offset, Some(0));
    assert!(log.mcn.is_none());
    assert!(log.isrcs.is_empty());
    assert_eq!(log.drive.as_ref().map(|d| d.firmware.clone()), Some(None));
}

#[test]
fn parse_log_tolerates_empty_input() {
    let log = redumper::parse_log_text("");
    let RedumperLog {
        version,
        drive,
        read_offset,
        mcn,
        isrcs,
        rip_date,
        ..
    } = log;
    assert!(version.is_none());
    assert!(drive.is_none());
    assert!(read_offset.is_none());
    assert!(mcn.is_none());
    assert!(isrcs.is_empty());
    assert!(rip_date.is_none());
}

#[test]
fn parse_log_on_eac_file_rejects_with_unsupported_variant() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("eac.log");
    fs::write(&path, b"Exact Audio Copy V1.5\nlog...\n").unwrap();

    let err = redumper::parse_log(&path).expect_err("EAC log should reject");
    let msg = err.to_string();
    assert!(
        msg.contains("not redumper"),
        "error message should name the mismatch: {msg}"
    );
}

// ---------------------------------------------------------------------
// parse_cdtext
// ---------------------------------------------------------------------

/// Build a single 18-byte CD-TEXT pack.
fn pack(ty: u8, track: u8, seq: u8, block: u8, dbcs: bool, data: [u8; 12]) -> [u8; 18] {
    let mut p = [0u8; 18];
    p[0] = ty;
    p[1] = track;
    p[2] = seq;
    p[3] = ((block & 0x07) << 4) | (if dbcs { 0x80 } else { 0 });
    p[4..16].copy_from_slice(&data);
    // CRC bytes left as zero — we don't verify.
    p
}

fn pack_text(ty: u8, track: u8, seq: u8, block: u8, text: &[u8]) -> [u8; 18] {
    let mut data = [0u8; 12];
    let n = text.len().min(12);
    data[..n].copy_from_slice(&text[..n]);
    pack(ty, track, seq, block, false, data)
}

#[test]
fn parse_cdtext_empty_input_yields_empty() {
    let ct = redumper::parse_cdtext_bytes(&[]).unwrap();
    assert!(ct.blocks.is_empty());
}

#[test]
fn parse_cdtext_album_and_two_tracks_single_block() {
    // Pack type 0x80 = title. Text stream across sequence-ordered packs,
    // null-separated: "Album\0T1\0T2\0".
    // Fits in two 12-byte packs.
    let mut payload: Vec<u8> = Vec::new();
    payload.extend_from_slice(b"Album\0Track ");
    payload.extend_from_slice(b"One\0Track Tw");
    payload.extend_from_slice(b"o\0");

    // Pad to 36 bytes (3 packs' worth).
    while payload.len() < 36 {
        payload.push(0);
    }

    let p0 = pack_text(0x80, 0, 0, 0, &payload[0..12]);
    let p1 = pack_text(0x80, 1, 1, 0, &payload[12..24]);
    let p2 = pack_text(0x80, 2, 2, 0, &payload[24..36]);

    // Performer: "Artist\0Artist\0Artist\0" (same artist for all).
    let mut perf: Vec<u8> = Vec::new();
    perf.extend_from_slice(b"Artist\0Artis");
    perf.extend_from_slice(b"t\0Artist\0\0\0\0");
    while perf.len() < 24 {
        perf.push(0);
    }
    let q0 = pack_text(0x81, 0, 0, 0, &perf[0..12]);
    let q1 = pack_text(0x81, 0, 1, 0, &perf[12..24]);

    // Block size info pack (0x8F) — charset=0 (ISO 8859-1) in sequence 0.
    let mut info0 = [0u8; 12];
    info0[0] = 0x00; // charset
    info0[1] = 1; // first track
    info0[2] = 2; // last track
    let r0 = pack(0x8F, 0, 0, 0, false, info0);

    let mut bytes = Vec::new();
    for p in [p0, p1, p2, q0, q1, r0] {
        bytes.extend_from_slice(&p);
    }

    let ct = redumper::parse_cdtext_bytes(&bytes).unwrap();
    assert_eq!(ct.blocks.len(), 1);
    let b: &CdTextBlock = ct.primary().unwrap();
    assert_eq!(b.block, 0);
    assert_eq!(b.charset_code, 0x00);
    assert_eq!(b.title.as_deref(), Some("Album"));
    assert_eq!(b.performer.as_deref(), Some("Artist"));
    assert_eq!(b.tracks.len(), 2);
    assert_eq!(b.tracks[0].track_number, 1);
    assert_eq!(b.tracks[0].title.as_deref(), Some("Track One"));
    assert_eq!(b.tracks[0].performer.as_deref(), Some("Artist"));
    assert_eq!(b.tracks[1].title.as_deref(), Some("Track Two"));
}

#[test]
fn parse_cdtext_extracts_upc_and_isrc() {
    // Pack 0x8E at track 0 = UPC; at track N = ISRC. UPC/EAN is 13 chars
    // so it spans two packs (sequence 0 carries 12 bytes, sequence 1
    // carries the 13th). ISRC is 12 chars and fits in one pack.
    let upc_p0 = pack_text(0x8E, 0, 0, 0, b"072736123456");
    let upc_p1 = pack_text(0x8E, 0, 1, 0, b"7");
    let isrc1 = pack_text(0x8E, 1, 2, 0, b"USRC17654321");
    let isrc2 = pack_text(0x8E, 2, 3, 0, b"USRC17654322");

    // Pair with a title pack so the block has at least one text field,
    // otherwise nothing assembles into the output block.
    let mut title_buf = [0u8; 12];
    title_buf[..6].copy_from_slice(b"Album\0");
    let title = pack(0x80, 0, 0, 0, false, title_buf);

    let mut bytes = Vec::new();
    for p in [title, upc_p0, upc_p1, isrc1, isrc2] {
        bytes.extend_from_slice(&p);
    }

    let ct = redumper::parse_cdtext_bytes(&bytes).unwrap();
    let b = ct.primary().unwrap();
    assert_eq!(b.upc_ean.as_deref(), Some("0727361234567"));
    assert_eq!(b.tracks.len(), 2);
    assert_eq!(b.tracks[0].isrc.as_deref(), Some("USRC17654321"));
    assert_eq!(b.tracks[1].isrc.as_deref(), Some("USRC17654322"));
}

#[test]
fn parse_cdtext_strips_4byte_response_header() {
    // One pack preceded by a 4-byte READ-TOC response header.
    let mut buf = [0u8; 12];
    buf[..6].copy_from_slice(b"Album\0");
    let p = pack(0x80, 0, 0, 0, false, buf);

    let mut bytes = vec![0x00, 0x10, 0x00, 0x00]; // fake length + reserved
    bytes.extend_from_slice(&p);

    let ct = redumper::parse_cdtext_bytes(&bytes).unwrap();
    assert_eq!(ct.primary().and_then(|b| b.title.as_deref()), Some("Album"));
}

#[test]
fn parse_cdtext_rejects_non_pack_aligned_input() {
    // 17 bytes — not a multiple of 18 and not 18N+4 either.
    let err = redumper::parse_cdtext_bytes(&[0u8; 17]).expect_err("should reject");
    let msg = err.to_string();
    assert!(
        msg.contains("not a multiple"),
        "expected alignment error: {msg}"
    );
}

// ---------------------------------------------------------------------
// Provenance-recorded real-package checks (explicitly invoked)
// ---------------------------------------------------------------------

#[test]
#[ignore = "set JUNK_LIBS_REDUMPER_PREFIX to a provenance-recorded real package"]
fn real_fixture_log_matches_build_709_oracle() {
    let prefix = real_fixture_prefix();
    let path = prefix.with_extension("log");
    let log = redumper::parse_log(&path).expect("real log should parse");
    assert_eq!(log.version.as_deref(), Some("(build: 709)"));
    assert_eq!(log.read_offset, Some(6));
    assert_eq!(log.disc_write_offset, Some(2));
}

#[test]
#[ignore = "set JUNK_LIBS_REDUMPER_PREFIX to a provenance-recorded real package"]
fn real_fixture_raw_structure_matches_build_709_oracle() {
    let prefix = real_fixture_prefix();
    let sidecars = redumper::find_sidecars(&prefix.with_extension("cue"));
    let structure = redumper::validate_current_cd_raw(&sidecars).expect("valid raw package");
    assert_eq!(structure.sample_frames, 130_612_434);
    assert_eq!(structure.subcode_frames, 222_130);
    assert_eq!(structure.sample_frame_delta, -6);
}

#[test]
#[ignore = "set JUNK_LIBS_REDUMPER_AUDIO_PREFIX to the Octopath build b736 package"]
fn real_audio_fixture_matches_redumper_boundaries_and_subchannel_metadata() {
    let prefix = real_audio_fixture_prefix();
    let log = redumper::parse_log(&prefix.with_extension("log")).expect("real log should parse");
    assert_eq!(log.version.as_deref(), Some("(build: b736)"));
    assert_eq!(log.read_offset, Some(6));
    assert_eq!(log.disc_write_offset, Some(0));
    assert_eq!(log.mcn.as_deref(), Some("4988601471916"));
    assert_eq!(log.isrcs.len(), 18);
    assert_eq!(log.isrcs.get(&1).map(String::as_str), Some("JPA842501138"));
    assert_eq!(log.isrcs.get(&18).map(String::as_str), Some("JPA842501155"));

    // INDEX 01 LBAs copied from this package's Redumper `final TOC`.
    // The CUE expresses the same positions file-relatively: every BIN after
    // track 1 begins with its own INDEX 00 region. The layout must still
    // recover these disc-absolute boundaries exactly.
    let index01_lbas = [
        0, 13_920, 29_892, 44_301, 58_689, 77_998, 95_493, 109_967, 125_151, 143_111, 161_550,
        178_404, 179_012, 195_891, 213_120, 229_089, 232_701, 250_359,
    ];
    let leadout_lba = 268_234;
    let layout = read_cue_layout(&prefix.with_extension("cue")).expect("real CUE should parse");
    assert_eq!(layout.len(), index01_lbas.len());
    for (idx, track) in layout.iter().enumerate() {
        let next_lba = index01_lbas.get(idx + 1).copied().unwrap_or(leadout_lba);
        assert_eq!(usize::from(track.number), idx + 1);
        assert_eq!(track.absolute_offset, 150 + index01_lbas[idx]);
        assert_eq!(track.length_sectors, next_lba - index01_lbas[idx]);
    }

    let sidecars = redumper::find_sidecars(&prefix.with_extension("cue"));
    let structure = redumper::validate_current_cd_raw(&sidecars).expect("valid raw package");
    assert_eq!(structure.sample_frames, 184_328_586);
    assert_eq!(structure.subcode_frames, 313_484);
    assert_eq!(structure.sample_frame_delta, -6);
}

fn real_fixture_prefix() -> PathBuf {
    std::env::var_os("JUNK_LIBS_REDUMPER_PREFIX")
        .map(PathBuf::from)
        .expect("JUNK_LIBS_REDUMPER_PREFIX must name the package prefix without an extension")
}

fn real_audio_fixture_prefix() -> PathBuf {
    std::env::var_os("JUNK_LIBS_REDUMPER_AUDIO_PREFIX")
        .map(PathBuf::from)
        .expect("JUNK_LIBS_REDUMPER_AUDIO_PREFIX must name the package prefix without an extension")
}
