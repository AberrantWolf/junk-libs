use crate::chd::*;

#[test]
fn test_parse_meta_field_basic() {
    let text = "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:229020 PREFRAMES:150";
    assert_eq!(parse_meta_field(text, "TRACK"), Some("1"));
    assert_eq!(parse_meta_field(text, "TYPE"), Some("MODE2_RAW"));
    assert_eq!(parse_meta_field(text, "FRAMES"), Some("229020"));
    assert_eq!(parse_meta_field(text, "PREFRAMES"), Some("150"));
    assert_eq!(parse_meta_field(text, "SUBTYPE"), Some("NONE"));
}

#[test]
fn test_parse_meta_field_missing() {
    let text = "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:18995";
    assert_eq!(parse_meta_field(text, "POSTGAP"), None);
    assert_eq!(parse_meta_field(text, "PREGAP"), None);
}

#[test]
fn test_parse_current_mame_track_metadata() {
    let track = parse_chd_track_text(
        "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:5 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0",
    )
    .unwrap();
    assert_eq!(track.track_number, 1);
    assert_eq!(track.track_type, "MODE1_RAW");
    assert_eq!(track.frames, 5);
}

#[test]
fn test_malformed_chd_track_metadata_fails_closed() {
    for text in [
        "TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:5",
        "TRACK:x TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:5",
        "TRACK:0 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:5",
        "TRACK:100 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:5",
        "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE",
        "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:0",
        "TRACK:1 TYPE:UNKNOWN SUBTYPE:NONE FRAMES:5",
    ] {
        assert!(parse_chd_track_text(text).is_err(), "accepted {text}");
    }
}

#[test]
fn test_chd_track_info_is_data() {
    let mode1 = ChdTrackInfo {
        track_number: 1,
        track_type: "MODE1_RAW".to_string(),
        frames: 19560,
        start_sector: 0,
    };
    let mode2 = ChdTrackInfo {
        track_number: 2,
        track_type: "MODE2_RAW".to_string(),
        frames: 78407,
        start_sector: 19560,
    };
    let audio = ChdTrackInfo {
        track_number: 3,
        track_type: "AUDIO".to_string(),
        frames: 906,
        start_sector: 97967,
    };
    assert!(mode1.is_data());
    assert!(mode2.is_data());
    assert!(!audio.is_data());
}

#[test]
fn test_select_largest_data_track_multi_data() {
    // Saturn-style: MODE1 boot + MODE2 main data
    let tracks = vec![
        ChdTrackInfo {
            track_number: 1,
            track_type: "MODE1_RAW".to_string(),
            frames: 19560,
            start_sector: 0,
        },
        ChdTrackInfo {
            track_number: 2,
            track_type: "MODE2_RAW".to_string(),
            frames: 78407,
            start_sector: 19560,
        },
        ChdTrackInfo {
            track_number: 3,
            track_type: "AUDIO".to_string(),
            frames: 906,
            start_sector: 97967,
        },
    ];
    let selected = select_largest_data_track(&tracks).unwrap();
    assert_eq!(selected.track_number, 2);
    assert_eq!(selected.frames, 78407);
    assert_eq!(selected.start_sector, 19560);
}

#[test]
fn test_select_largest_data_track_single_data() {
    // PS1-style: single MODE2 data track + audio
    let tracks = vec![
        ChdTrackInfo {
            track_number: 1,
            track_type: "MODE2_RAW".to_string(),
            frames: 229020,
            start_sector: 0,
        },
        ChdTrackInfo {
            track_number: 2,
            track_type: "AUDIO".to_string(),
            frames: 18995,
            start_sector: 229020,
        },
    ];
    let selected = select_largest_data_track(&tracks).unwrap();
    assert_eq!(selected.track_number, 1);
    assert_eq!(selected.frames, 229020);
    assert_eq!(selected.start_sector, 0);
}

#[test]
fn test_select_largest_data_track_no_data() {
    let tracks = vec![ChdTrackInfo {
        track_number: 1,
        track_type: "AUDIO".to_string(),
        frames: 5000,
        start_sector: 0,
    }];
    assert!(select_largest_data_track(&tracks).is_none());
}

#[test]
fn test_select_largest_data_track_empty() {
    let tracks: Vec<ChdTrackInfo> = vec![];
    assert!(select_largest_data_track(&tracks).is_none());
}

/// Cached reads through [`ChdHunkCache`] must return the same bytes as the
/// one-shot [`read_chd_raw_sector`], including after moving between hunks.
/// Set `JUNK_LIBS_CHD_FIXTURE` to a CD CHD and run with `--ignored`.
#[test]
#[ignore = "set JUNK_LIBS_CHD_FIXTURE to a CD CHD"]
fn chd_hunk_cache_matches_one_shot_reads() {
    use std::fs::File;
    use std::io::BufReader;

    let path = std::env::var("JUNK_LIBS_CHD_FIXTURE").expect("JUNK_LIBS_CHD_FIXTURE");
    let mut cache =
        ChdHunkCache::open(BufReader::new(File::open(&path).expect("open CHD fixture")))
            .expect("open cache");

    for sector in [0, 1, 10_000, 0] {
        let cached = cache.read_raw_sector(sector).expect("cached read");
        let mut reader = BufReader::new(File::open(&path).expect("reopen CHD fixture"));
        let one_shot = read_chd_raw_sector(&mut reader, sector).expect("one-shot read");
        assert_eq!(cached, one_shot, "sector {sector}");
    }
}
