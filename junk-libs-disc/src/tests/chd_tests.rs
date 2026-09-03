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
fn test_parse_meta_field_audio_track() {
    let text = "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:18995 PREFRAMES:150";
    assert_eq!(parse_meta_field(text, "TRACK"), Some("2"));
    assert_eq!(parse_meta_field(text, "TYPE"), Some("AUDIO"));
    assert_eq!(parse_meta_field(text, "FRAMES"), Some("18995"));
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

/// Exercise [`ChdHunkCache`] against a real CHD file supplied via the
/// `JUNK_LIBS_CHD_FIXTURE` environment variable. Ignored by default so
/// `cargo test` stays hermetic; run with
///
/// ```sh
/// JUNK_LIBS_CHD_FIXTURE=/path/to/some.chd \
///     cargo test -p junk-libs-disc -- --ignored chd_hunk_cache
/// ```
///
/// The two assertions together pin down the cache's contract:
///
/// 1. Sequential reads of distinct sectors inside a single hunk must only
///    trigger one decompression.
/// 2. Crossing a hunk boundary must decompress the new hunk and leave the
///    previously-cached one.
/// 3. Every sector returned matches what the one-shot
///    [`read_chd_raw_sector`] returns for the same sector on a fresh
///    reader — regression guard for the refactor that now has both paths
///    share the cache code.
#[test]
#[ignore]
fn chd_hunk_cache_reuses_hunk_and_matches_one_shot() {
    use std::fs::File;
    use std::io::BufReader;

    let path = match std::env::var("JUNK_LIBS_CHD_FIXTURE") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("JUNK_LIBS_CHD_FIXTURE not set; skipping");
            return;
        }
    };

    let reader = BufReader::new(File::open(&path).expect("open CHD fixture"));
    let mut cache = ChdHunkCache::open(reader).expect("open cache");

    // Probe the cache for sector 0 and sector 1 — practically always the
    // same hunk given hunk_size >> unit_bytes for any real CHD.
    let s0 = cache.read_raw_sector(0).expect("sector 0");
    let after_first = cache.decompress_count();
    let s1 = cache.read_raw_sector(1).expect("sector 1");
    let after_second = cache.decompress_count();
    assert_eq!(
        after_first, after_second,
        "second sector in the same hunk must not trigger a decompression"
    );

    // Pick a sector guaranteed to live in a later hunk: sector N where
    // N * unit_bytes >= hunk_size. For a CHD with 75 sectors/hunk (typical
    // at 2352 unit bytes and a 176400-byte hunk), sector 75 crosses the
    // boundary. Use a conservative factor.
    let probe = 10_000u64;
    let _ = cache.read_raw_sector(probe).expect("far sector");
    let after_far = cache.decompress_count();
    assert!(
        after_far > after_second,
        "crossing a hunk boundary must trigger a decompression"
    );

    // Read sector 0 again — now it lives in a dropped hunk, so it should
    // re-decompress.
    let s0_again = cache.read_raw_sector(0).expect("sector 0 revisit");
    assert_eq!(s0, s0_again, "bytes must be stable across cache churn");
    assert!(
        cache.decompress_count() > after_far,
        "revisiting an evicted hunk must re-decompress"
    );

    // Byte-for-byte equivalence: the one-shot path and the cached path
    // must return identical bytes.
    let mut reader = BufReader::new(File::open(&path).expect("reopen CHD fixture"));
    let s0_one_shot = read_chd_raw_sector(&mut reader, 0).expect("one-shot read");
    assert_eq!(s0, s0_one_shot);
    let mut reader = BufReader::new(File::open(&path).expect("reopen CHD fixture"));
    let s1_one_shot = read_chd_raw_sector(&mut reader, 1).expect("one-shot read");
    assert_eq!(s1, s1_one_shot);
}
