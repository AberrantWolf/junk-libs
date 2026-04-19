//! Absolute-sector layout tests for CUE and CHD sources.
//!
//! Fixture values come from ARver's test suite and the MusicBrainz DiscID
//! spec so the expected absolute-offset sequences match authoritative
//! DiscID reference values.
//!
//! Sources:
//! - <https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py>
//! - <https://musicbrainz.org/doc/Disc_ID_Calculation>

use crate::chd::{ChdTrackInfo, compute_chd_layout};
use crate::cue::{compute_cue_layout, parse_cue};
use crate::layout::{LEAD_IN_FRAMES, TrackKind, classify_mode};
use crate::sector::RAW_SECTOR_SIZE;

// ---------------------------------------------------------------------------
// classify_mode
// ---------------------------------------------------------------------------

#[test]
fn classify_mode_audio() {
    assert_eq!(classify_mode("AUDIO"), TrackKind::Audio);
    assert_eq!(classify_mode("audio"), TrackKind::Audio);
}

#[test]
fn classify_mode_data_mode1_and_mode2() {
    assert_eq!(classify_mode("MODE1/2048"), TrackKind::Data);
    assert_eq!(classify_mode("MODE1/2352"), TrackKind::Data);
    assert_eq!(classify_mode("MODE2/2336"), TrackKind::Data);
    assert_eq!(classify_mode("MODE2/2352"), TrackKind::Data);
    assert_eq!(classify_mode("mode2_raw"), TrackKind::Data);
}

#[test]
fn classify_mode_unknown() {
    assert_eq!(classify_mode(""), TrackKind::Unknown);
    assert_eq!(classify_mode("CDG"), TrackKind::Unknown);
}

// ---------------------------------------------------------------------------
// compute_cue_layout — single-FILE CUE
// ---------------------------------------------------------------------------

// Reproduces the ARver 3-track fixture:
//   tracks = [75258, 54815, 205880], pregap = 0, data = 0
// → absolute offsets [150, 75408, 130223], leadout 336103.
// Source: https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py
#[test]
fn compute_cue_layout_single_file_arver_3track() {
    // within-BIN offsets: 0, 75258, 130073; total BIN = 335953 sectors.
    let cue = r#"FILE "disc.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    INDEX 01 16:43:33
  TRACK 03 AUDIO
    INDEX 01 28:54:23
"#;
    let sheet = parse_cue(cue).unwrap();
    let total_bytes = 335953u64 * RAW_SECTOR_SIZE;
    let layout = compute_cue_layout(&sheet, |_| Ok(total_bytes)).unwrap();

    assert_eq!(layout.len(), 3);
    assert_eq!(layout[0].absolute_offset, 150);
    assert_eq!(layout[1].absolute_offset, 75408);
    assert_eq!(layout[2].absolute_offset, 130223);
    assert_eq!(layout[0].length_sectors, 75258);
    assert_eq!(layout[1].length_sectors, 54815);
    assert_eq!(layout[2].length_sectors, 205880);
    assert!(layout.iter().all(|t| t.kind == TrackKind::Audio));
    // Lead-out reconstruction:
    assert_eq!(
        layout.last().unwrap().absolute_offset + layout.last().unwrap().length_sectors,
        336103
    );
}

// Reproduces the ARver 4-track pregap fixture:
//   tracks = [107450, 71470, 105737, 71600], pregap = 33, data = 0
// → absolute offsets [183, 107633, 179103, 284840], leadout 356440.
// Verifies the HTOA-pregap path: track 1 starts at within-BIN 33, so its
// absolute offset is 150 + 33 = 183.
#[test]
fn compute_cue_layout_htoa_pregap_4track() {
    // within-BIN: 33, 107483, 178953, 284690; total = 356290 sectors.
    let cue = r#"FILE "disc.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:33
  TRACK 02 AUDIO
    INDEX 01 23:53:08
  TRACK 03 AUDIO
    INDEX 01 39:46:03
  TRACK 04 AUDIO
    INDEX 01 63:15:65
"#;
    let sheet = parse_cue(cue).unwrap();
    let total_bytes = 356290u64 * RAW_SECTOR_SIZE;
    let layout = compute_cue_layout(&sheet, |_| Ok(total_bytes)).unwrap();

    assert_eq!(
        layout.iter().map(|t| t.absolute_offset).collect::<Vec<_>>(),
        vec![183, 107633, 179103, 284840]
    );
    assert_eq!(
        layout.last().unwrap().absolute_offset + layout.last().unwrap().length_sectors,
        356440
    );
}

// ---------------------------------------------------------------------------
// compute_cue_layout — multi-FILE CUE
// ---------------------------------------------------------------------------

// Same ARver 3-track offsets via one BIN per track.
#[test]
fn compute_cue_layout_multi_file_arver_3track() {
    let cue = r#"FILE "t1.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
FILE "t2.bin" BINARY
  TRACK 02 AUDIO
    INDEX 01 00:00:00
FILE "t3.bin" BINARY
  TRACK 03 AUDIO
    INDEX 01 00:00:00
"#;
    let sheet = parse_cue(cue).unwrap();
    let layout = compute_cue_layout(&sheet, |name| {
        let sectors: u64 = match name {
            "t1.bin" => 75258,
            "t2.bin" => 54815,
            "t3.bin" => 205880,
            _ => return Err(junk_libs_core::AnalysisError::invalid_format("unknown BIN")),
        };
        Ok(sectors * RAW_SECTOR_SIZE)
    })
    .unwrap();

    assert_eq!(
        layout.iter().map(|t| t.absolute_offset).collect::<Vec<_>>(),
        vec![150, 75408, 130223]
    );
    assert_eq!(
        layout.iter().map(|t| t.length_sectors).collect::<Vec<_>>(),
        vec![75258, 54815, 205880]
    );
}

// ---------------------------------------------------------------------------
// compute_cue_layout — error paths
// ---------------------------------------------------------------------------

#[test]
fn compute_cue_layout_missing_bin_errors() {
    let cue = r#"FILE "missing.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
"#;
    let sheet = parse_cue(cue).unwrap();
    let res = compute_cue_layout(&sheet, |_| {
        Err(junk_libs_core::AnalysisError::other("BIN not found"))
    });
    assert!(res.is_err());
}

#[test]
fn compute_cue_layout_unaligned_bin_errors() {
    let cue = r#"FILE "disc.bin" BINARY
  TRACK 01 AUDIO
    INDEX 01 00:00:00
"#;
    let sheet = parse_cue(cue).unwrap();
    // 2352 * 100 + 7 — not a multiple of 2352.
    let res = compute_cue_layout(&sheet, |_| Ok(RAW_SECTOR_SIZE * 100 + 7));
    assert!(res.is_err());
}

#[test]
fn compute_cue_layout_missing_index01_errors() {
    // Only INDEX 00 given — no actual track start.
    let cue = r#"FILE "disc.bin" BINARY
  TRACK 01 AUDIO
    INDEX 00 00:00:00
"#;
    let sheet = parse_cue(cue).unwrap();
    let res = compute_cue_layout(&sheet, |_| Ok(100 * RAW_SECTOR_SIZE));
    assert!(res.is_err());
}

// ---------------------------------------------------------------------------
// compute_chd_layout
// ---------------------------------------------------------------------------

#[test]
fn compute_chd_layout_audio_only_arver_3track() {
    let tracks = vec![
        ChdTrackInfo {
            track_number: 1,
            track_type: "AUDIO".into(),
            frames: 75258,
            start_sector: 0,
        },
        ChdTrackInfo {
            track_number: 2,
            track_type: "AUDIO".into(),
            frames: 54815,
            start_sector: 75258,
        },
        ChdTrackInfo {
            track_number: 3,
            track_type: "AUDIO".into(),
            frames: 205880,
            start_sector: 75258 + 54815,
        },
    ];
    let layout = compute_chd_layout(&tracks);

    assert_eq!(
        layout.iter().map(|t| t.absolute_offset).collect::<Vec<_>>(),
        vec![150, 75408, 130223]
    );
    assert!(layout.iter().all(|t| t.kind == TrackKind::Audio));
    assert_eq!(
        layout.last().unwrap().absolute_offset + layout.last().unwrap().length_sectors,
        336103
    );
}

#[test]
fn compute_chd_layout_mixed_modes_classifies_correctly() {
    let tracks = vec![
        ChdTrackInfo {
            track_number: 1,
            track_type: "MODE1_RAW".into(),
            frames: 1000,
            start_sector: 0,
        },
        ChdTrackInfo {
            track_number: 2,
            track_type: "AUDIO".into(),
            frames: 2000,
            start_sector: 1000,
        },
    ];
    let layout = compute_chd_layout(&tracks);
    assert_eq!(layout[0].kind, TrackKind::Data);
    assert_eq!(layout[1].kind, TrackKind::Audio);
    assert_eq!(layout[0].absolute_offset, LEAD_IN_FRAMES);
    assert_eq!(layout[1].absolute_offset, LEAD_IN_FRAMES + 1000);
}
