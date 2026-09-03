use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::layout::{LEAD_IN_FRAMES, TrackKind};
use crate::pcm::{PCM_SAMPLES_PER_SECTOR, TrackPcmReader, sector_to_samples};
use crate::sector::RAW_SECTOR_SIZE;

// ---------------------------------------------------------------------------
// sector_to_samples unit tests
// ---------------------------------------------------------------------------

#[test]
fn sector_to_samples_packs_little_endian_u16_pairs() {
    let mut raw = [0u8; RAW_SECTOR_SIZE as usize];
    // Sample 0: L = 0x1234, R = 0x5678.
    raw[0] = 0x34;
    raw[1] = 0x12;
    raw[2] = 0x78;
    raw[3] = 0x56;
    // Sample 1: L = 0xFFFF (−1 signed), R = 0x8000 (min signed).
    raw[4] = 0xFF;
    raw[5] = 0xFF;
    raw[6] = 0x00;
    raw[7] = 0x80;

    let samples = sector_to_samples(&raw);
    assert_eq!(samples[0], 0x1234 | (0x5678u32 << 16));
    assert_eq!(samples[1], 0xFFFFu32 | (0x8000u32 << 16));
    for s in &samples[2..] {
        assert_eq!(*s, 0);
    }
}

#[test]
fn sector_to_samples_emits_exactly_588_samples() {
    let raw = [0u8; RAW_SECTOR_SIZE as usize];
    let samples = sector_to_samples(&raw);
    assert_eq!(samples.len(), PCM_SAMPLES_PER_SECTOR);
    assert_eq!(samples.len(), 588);
}

// ---------------------------------------------------------------------------
// CUE fixture helpers
// ---------------------------------------------------------------------------

/// Tag a sector's first four bytes with a recognisable marker so tests can
/// tell one sector apart from another. `marker` is written as the packed
/// left|right u32 value (little-endian): low 16 bits = left, high 16 = right.
fn marker_bytes(marker: u32) -> [u8; 4] {
    [
        (marker & 0xFF) as u8,
        ((marker >> 8) & 0xFF) as u8,
        ((marker >> 16) & 0xFF) as u8,
        ((marker >> 24) & 0xFF) as u8,
    ]
}

/// Build a fresh temp directory under the test-scoped tmpdir root with a
/// unique subdir name. Cleanup happens on drop via the returned handle.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("junk_libs_pcm_{tag}_{pid}_{nonce}"));
        std::fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Write a zero-filled BIN with per-sector markers. `marker_for(index)`
/// returns the `u32` value to stamp into sector `index`'s first sample.
fn write_marked_bin(path: &Path, sector_count: u32, mut marker_for: impl FnMut(u32) -> u32) {
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    f.set_len(u64::from(sector_count) * RAW_SECTOR_SIZE)
        .unwrap();
    for i in 0..sector_count {
        let marker = marker_for(i);
        f.seek(SeekFrom::Start(u64::from(i) * RAW_SECTOR_SIZE))
            .unwrap();
        f.write_all(&marker_bytes(marker)).unwrap();
    }
}

// ---------------------------------------------------------------------------
// from_cue: single-BIN whole-disc CUE
// ---------------------------------------------------------------------------

/// Build a single-BIN CUE with two tracks, each 10 sectors, and verify
/// `from_cue` picks the right sector for each track number.
#[test]
fn from_cue_single_bin_reads_each_track_at_its_offset() {
    let dir = TempDir::new("single_bin");
    let bin_path = dir.path.join("disc.bin");
    // 20 sectors total: track 1 = sectors 0..10, track 2 = sectors 10..20.
    // Marker in sector i = 0xDEAD_0000 | i.
    write_marked_bin(&bin_path, 20, |i| 0xDEAD_0000 | i);

    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"disc.bin\" BINARY\n  \
         TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  \
         TRACK 02 AUDIO\n    INDEX 01 00:00:10\n",
    )
    .unwrap();

    // Track 1 spans sectors 0..10 within the BIN. Sector 0's marker is
    // 0xDEAD_0000 (left=0, right=0xDEAD).
    let mut r1 = TrackPcmReader::from_cue(&cue_path, 1).unwrap();
    let first = r1.next().unwrap().unwrap();
    assert_eq!(first[0], 0xDEAD_0000);
    // Track 1 emits exactly 10 sectors (length = track 2.INDEX_01 - track 1.INDEX_01 = 10).
    assert_eq!(r1.count(), 9);

    // Track 2 spans sectors 10..20. Sector 10's marker = 0xDEAD_000A.
    let mut r2 = TrackPcmReader::from_cue(&cue_path, 2).unwrap();
    let first = r2.next().unwrap().unwrap();
    assert_eq!(first[0], 0xDEAD_000A);
    assert_eq!(r2.count(), 9);
}

// ---------------------------------------------------------------------------
// from_cue: multi-BIN per-track CUE (one BIN per track, no pregaps)
// ---------------------------------------------------------------------------

#[test]
fn from_cue_multi_bin_reads_each_files_contents() {
    let dir = TempDir::new("multi_bin_flat");
    // 3 tracks, each 5 sectors, each in its own BIN.
    // Marker in file N sector i = 0xN_0000_0000 | i (actually u32; use file_tag in low byte).
    write_marked_bin(&dir.path.join("t1.bin"), 5, |i| 0x0100_0000 | i);
    write_marked_bin(&dir.path.join("t2.bin"), 5, |i| 0x0200_0000 | i);
    write_marked_bin(&dir.path.join("t3.bin"), 5, |i| 0x0300_0000 | i);

    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"t1.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"t2.bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"t3.bin\" BINARY\n  TRACK 03 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();

    for (track, file_tag) in [(1u8, 0x01u32), (2, 0x02), (3, 0x03)] {
        let mut r = TrackPcmReader::from_cue(&cue_path, track).unwrap();
        let first = r.next().unwrap().unwrap();
        assert_eq!(
            first[0],
            file_tag << 24,
            "track {track} should start with file tag {file_tag:#x}",
        );
        assert_eq!(r.count(), 4, "track {track} length");
    }
}

// ---------------------------------------------------------------------------
// from_cue: multi-BIN with INDEX 00 pregap on later track
// ---------------------------------------------------------------------------

/// Our production bug case: track 3 has INDEX 00 00:00:00 + INDEX 01 00:01:33
/// (108-frame pregap at the start of track 3's BIN; CUE MSF frames are
/// 1/75s, so 1 second + 33 frames = 75 + 33 = 108). Expected behaviour:
///   - Track 3 starts at BIN 3 sector 108 (skips the pregap, plays music).
///   - Track 2 extends INTO the start of BIN 3 for 108 sectors, crossing
///     the file boundary (matches `compute_cue_layout`'s length math — AR
///     CRC relies on this).
#[test]
fn from_cue_multi_bin_track_spans_pregap_in_next_file() {
    let dir = TempDir::new("multi_bin_pregap");
    // Track 1 BIN: 5 sectors, markers 0x10..0x14.
    // Track 2 BIN: 5 sectors, markers 0x20..0x24.
    // Track 3 BIN: 200 sectors (108 pregap + 92 music), markers 0x30..0x30+199.
    write_marked_bin(&dir.path.join("t1.bin"), 5, |i| 0x10 + i);
    write_marked_bin(&dir.path.join("t2.bin"), 5, |i| 0x20 + i);
    write_marked_bin(&dir.path.join("t3.bin"), 200, |i| 0x30 + i);

    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"t1.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"t2.bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"t3.bin\" BINARY\n  TRACK 03 AUDIO\n    \
           INDEX 00 00:00:00\n    INDEX 01 00:01:33\n",
    )
    .unwrap();

    // Track 3 starts at BIN 3 sector 108 (pregap skipped).
    let mut r3 = TrackPcmReader::from_cue(&cue_path, 3).unwrap();
    let first = r3.next().unwrap().unwrap();
    assert_eq!(first[0], 0x30 + 108);
    // Length = 200 - 108 = 92 sectors, so 91 remain after the first next().
    assert_eq!(r3.count(), 91);

    // Track 2 crosses the file boundary. Per `compute_cue_layout`:
    //   track2.length = track3.disc_start - track2.disc_start
    //                 = (150 + 5 + 5 + 108) - (150 + 5)
    //                 = 113
    // First 5 sectors come from t2.bin; the next 108 come from t3.bin sectors 0..107.
    let r2 = TrackPcmReader::from_cue(&cue_path, 2).unwrap();
    let sectors: Vec<_> = r2.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(sectors.len(), 113);
    for (i, sector) in sectors.iter().take(5).enumerate() {
        assert_eq!(sector[0], 0x20 + i as u32, "t2.bin sector {i}");
    }
    for (i, sector) in sectors.iter().skip(5).take(108).enumerate() {
        assert_eq!(
            sector[0],
            0x30 + i as u32,
            "t3.bin sector {i} (as part of track 2)",
        );
    }
}

// ---------------------------------------------------------------------------
// from_cue: error paths
// ---------------------------------------------------------------------------

#[test]
fn from_cue_rejects_missing_track_number() {
    let dir = TempDir::new("missing_track");
    write_marked_bin(&dir.path.join("disc.bin"), 10, |_| 0);
    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let err = TrackPcmReader::from_cue(&cue_path, 99).expect_err("missing track must error");
    let msg = format!("{err}");
    assert!(msg.contains("no track 99"), "got: {msg}");
}

#[test]
fn from_cue_rejects_data_track() {
    let dir = TempDir::new("data_track");
    write_marked_bin(&dir.path.join("disc.bin"), 10, |_| 0);
    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let err = TrackPcmReader::from_cue(&cue_path, 1).expect_err("data track must error");
    assert!(format!("{err}").contains("rejects non-audio"));
}

#[test]
fn from_cue_rejects_missing_bin_file() {
    let dir = TempDir::new("missing_bin");
    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"ghost.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let err = TrackPcmReader::from_cue(&cue_path, 1).expect_err("missing BIN must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("ghost.bin"),
        "error should name the missing file: {msg}"
    );
}

#[test]
fn from_cue_rejects_misaligned_bin_size() {
    let dir = TempDir::new("misaligned");
    let bin_path = dir.path.join("odd.bin");
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&bin_path)
        .unwrap();
    f.set_len(RAW_SECTOR_SIZE * 5 + 100).unwrap();
    drop(f);

    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"odd.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let err = TrackPcmReader::from_cue(&cue_path, 1).expect_err("misaligned BIN must error");
    assert!(format!("{err}").contains("not a multiple of"));
}

// ---------------------------------------------------------------------------
// seek_to_sector
// ---------------------------------------------------------------------------

#[test]
fn seek_to_sector_mid_track_reads_expected_sector() {
    let dir = TempDir::new("seek_mid");
    write_marked_bin(&dir.path.join("disc.bin"), 10, |i| 0xAA00 | i);
    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();

    let mut r = TrackPcmReader::from_cue(&cue_path, 1).unwrap();
    // Burn two sectors, then seek forward past one, land at sector 5.
    let _ = r.next().unwrap().unwrap();
    let _ = r.next().unwrap().unwrap();
    r.seek_to_sector(5).unwrap();
    let s = r.next().unwrap().unwrap();
    assert_eq!(s[0], 0xAA00 | 5);
    assert_eq!(r.sector_offset(), 6);
}

#[test]
fn seek_to_sector_crosses_file_boundary_in_multi_bin() {
    let dir = TempDir::new("seek_multi");
    // Track 2 spans t2.bin (5 sectors) + first 108 sectors of t3.bin.
    write_marked_bin(&dir.path.join("t1.bin"), 5, |i| 0x10 + i);
    write_marked_bin(&dir.path.join("t2.bin"), 5, |i| 0x20 + i);
    write_marked_bin(&dir.path.join("t3.bin"), 200, |i| 0x30 + i);
    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"t1.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"t2.bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"t3.bin\" BINARY\n  TRACK 03 AUDIO\n    \
           INDEX 00 00:00:00\n    INDEX 01 00:01:33\n",
    )
    .unwrap();

    let mut r = TrackPcmReader::from_cue(&cue_path, 2).unwrap();
    // Sector 4 is still in t2.bin (marker 0x24).
    r.seek_to_sector(4).unwrap();
    let s = r.next().unwrap().unwrap();
    assert_eq!(s[0], 0x24);
    // Jump to sector 50 — lands at t3.bin sector (50 - 5) = 45.
    r.seek_to_sector(50).unwrap();
    let s = r.next().unwrap().unwrap();
    assert_eq!(s[0], 0x30 + 45);
    // Backward seek works too — reader is fully random-access.
    r.seek_to_sector(0).unwrap();
    let s = r.next().unwrap().unwrap();
    assert_eq!(s[0], 0x20, "seek(0) returns to t2.bin sector 0");
}

#[test]
fn seek_past_end_errors() {
    let dir = TempDir::new("seek_past_end");
    write_marked_bin(&dir.path.join("disc.bin"), 10, |_| 0);
    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let mut r = TrackPcmReader::from_cue(&cue_path, 1).unwrap();
    // total_sectors = 10. seek(10) is EOF (allowed, produces no more
    // sectors). seek(11) overshoots → error.
    r.seek_to_sector(10).unwrap();
    assert!(r.next().is_none());
    let err = r.seek_to_sector(11).expect_err("overshoot must error");
    assert!(format!("{err}").contains("exceeds track length"));
}

// ---------------------------------------------------------------------------
// total_samples sanity
// ---------------------------------------------------------------------------

#[test]
fn total_samples_matches_length_sectors_times_588() {
    let dir = TempDir::new("total_samples");
    write_marked_bin(&dir.path.join("disc.bin"), 10, |_| 0);
    let cue_path = dir.path.join("disc.cue");
    std::fs::write(
        &cue_path,
        "FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let reader = TrackPcmReader::from_cue(&cue_path, 1).unwrap();
    assert_eq!(reader.total_samples(), 10 * 588);
}

// ---------------------------------------------------------------------------
// from_chd: real-fixture integration (opt-in, unchanged)
// ---------------------------------------------------------------------------

/// Full-track PCM iteration via [`TrackPcmReader::from_chd`] must yield
/// the same raw bytes (after `sector_to_samples` packing) as calling
/// [`crate::chd::read_chd_raw_sector`] sector-by-sector on a fresh reader.
///
/// Ignored by default; set `JUNK_LIBS_CHD_FIXTURE` to a CHD file whose
/// layout contains at least one audio track. Run with
///
/// ```sh
/// JUNK_LIBS_CHD_FIXTURE=/path/to/some.chd \
///     cargo test -p junk-libs-disc -- --ignored chd_pcm_iterator
/// ```
#[test]
#[ignore]
fn chd_pcm_iterator_matches_read_chd_raw_sector() {
    use std::fs::File;
    use std::io::BufReader;

    use crate::chd::{read_chd_layout, read_chd_raw_sector};

    let path = match std::env::var("JUNK_LIBS_CHD_FIXTURE") {
        Ok(p) => p.into(),
        Err(_) => {
            eprintln!("JUNK_LIBS_CHD_FIXTURE not set; skipping");
            return;
        }
    };
    let path: std::path::PathBuf = path;

    let layouts = read_chd_layout(&path).expect("layout");
    let audio = layouts
        .iter()
        .find(|l| l.kind == TrackKind::Audio)
        .expect("fixture must contain an audio track");

    let mut iter = TrackPcmReader::from_chd(&path, audio.number).expect("open PCM reader");
    let mut reader = BufReader::new(File::open(&path).expect("reopen CHD"));

    let probes: Vec<u32> = (0..8u32).chain([audio.length_sectors - 1]).collect();
    let mut consumed_index = 0u32;
    for &want in &probes {
        while consumed_index < want {
            let _ = iter.next().expect("mid sector").expect("mid sector ok");
            consumed_index += 1;
        }
        let got = iter
            .next()
            .expect("target sector")
            .expect("target sector ok");
        consumed_index += 1;

        let bin_sector = (audio.absolute_offset + want - LEAD_IN_FRAMES) as u64;
        let raw = read_chd_raw_sector(&mut reader, bin_sector).expect("one-shot raw");
        assert_eq!(
            got,
            sector_to_samples(&raw),
            "iterator sector {want} must match one-shot raw read"
        );
    }
}
