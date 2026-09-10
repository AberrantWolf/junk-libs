//! dBAR binary parser tests.
//!
//! Uses hand-built byte blobs so the structure is auditable without
//! tools. The fixture layout mirrors ARver's
//! [`arver/disc/database.py`](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py)
//! parser and the format description in
//! Phono Junk's source-cited `docs/knowledge/disc-formats/accuraterip.md`.

use junk_libs_accuraterip::{AccurateRipError, DbarFile, DbarResponse, ExpectedChecksum};

/// Build one response as raw little-endian bytes.
fn build_response(
    track_count: u8,
    ar_id1: u32,
    ar_id2: u32,
    cddb_id: u32,
    entries: &[(u8, u32, u32)],
) -> Vec<u8> {
    assert_eq!(entries.len(), track_count as usize);
    let mut out = Vec::new();
    out.push(track_count);
    out.extend_from_slice(&ar_id1.to_le_bytes());
    out.extend_from_slice(&ar_id2.to_le_bytes());
    out.extend_from_slice(&cddb_id.to_le_bytes());
    for &(conf, v1, v2) in entries {
        out.push(conf);
        out.extend_from_slice(&v1.to_le_bytes());
        out.extend_from_slice(&v2.to_le_bytes());
    }
    out
}

#[test]
fn single_response_three_tracks_round_trips() {
    let bytes = build_response(
        3,
        0x0008_4264,
        0x001c_c184,
        0x1911_7f03,
        &[
            (5, 0xdead_beef, 0xcafe_f00d),
            (3, 0x1234_5678, 0x0000_0000),
            (12, 0xffff_ffff, 0x0000_0001),
        ],
    );

    let parsed = DbarFile::parse(&bytes).expect("parse");
    assert_eq!(parsed.responses.len(), 1);
    assert_eq!(
        parsed.responses[0],
        DbarResponse {
            track_count: 3,
            ar_id1: 0x0008_4264,
            ar_id2: 0x001c_c184,
            cddb_id: 0x1911_7f03,
            tracks: vec![
                ExpectedChecksum {
                    confidence: 5,
                    checksum: 0xdead_beef,
                    checksum_450: 0xcafe_f00d
                },
                ExpectedChecksum {
                    confidence: 3,
                    checksum: 0x1234_5678,
                    checksum_450: 0
                },
                ExpectedChecksum {
                    confidence: 12,
                    checksum: 0xffff_ffff,
                    checksum_450: 0x0000_0001
                },
            ],
        }
    );
}

#[test]
fn two_stacked_responses_parse_in_order() {
    let mut bytes = build_response(
        2,
        0x1111_1111,
        0x2222_2222,
        0x3333_3333,
        &[(10, 0xaaaa_aaaa, 0xbbbb_bbbb), (8, 0xcccc_cccc, 0)],
    );
    bytes.extend(build_response(
        2,
        0x4444_4444,
        0x5555_5555,
        0x6666_6666,
        &[(1, 0x0101_0101, 0x0202_0202), (2, 0x0303_0303, 0x0404_0404)],
    ));

    let parsed = DbarFile::parse(&bytes).expect("parse");
    assert_eq!(parsed.responses.len(), 2);
    assert_eq!(parsed.responses[0].ar_id1, 0x1111_1111);
    assert_eq!(parsed.responses[1].ar_id1, 0x4444_4444);
    assert_eq!(parsed.responses[0].tracks[1].checksum_450, 0);
    assert_eq!(parsed.responses[1].tracks[0].confidence, 1);
}

#[test]
fn empty_buffer_parses_to_zero_responses() {
    let parsed = DbarFile::parse(&[]).expect("parse empty");
    assert!(parsed.responses.is_empty());
}

#[test]
fn truncated_header_errors() {
    let bytes = [0x03, 0x64, 0x42];
    match DbarFile::parse(&bytes) {
        Err(AccurateRipError::Parse(msg)) => {
            assert!(msg.contains("truncated header"), "msg was: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn truncated_entries_errors() {
    let mut bytes = build_response(
        3,
        0x0008_4264,
        0x001c_c184,
        0x1911_7f03,
        &[
            (5, 0xdead_beef, 0xcafe_f00d),
            (3, 0x1234_5678, 0),
            (12, 0xffff_ffff, 1),
        ],
    );
    bytes.truncate(bytes.len() - 3);
    match DbarFile::parse(&bytes) {
        Err(AccurateRipError::Parse(msg)) => {
            assert!(msg.contains("truncated entries"), "msg was: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn entries_for_track_yields_all_pressings_at_position() {
    let mut bytes = build_response(
        3,
        1,
        2,
        3,
        &[(5, 0xaa, 0xbb), (5, 0xcc, 0xdd), (5, 0xee, 0xff)],
    );
    bytes.extend(build_response(
        3,
        1,
        2,
        3,
        &[(2, 0x11, 0x22), (2, 0x33, 0x44), (2, 0x55, 0x66)],
    ));

    let parsed = DbarFile::parse(&bytes).unwrap();
    let hits: Vec<_> = parsed.entries_for_track(2).collect();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].0, 0);
    assert_eq!(hits[0].1.checksum, 0xcc);
    assert_eq!(hits[1].0, 1);
    assert_eq!(hits[1].1.checksum, 0x33);
}

#[test]
fn request_validation_rejects_wrong_header_or_track_count() {
    let bytes = build_response(2, 1, 2, 3, &[(5, 0xaa, 0xbb), (5, 0xcc, 0xdd)]);
    let parsed = DbarFile::parse(&bytes).unwrap();
    parsed.validate_request(2, 1, 2, 3).unwrap();
    let error = parsed.validate_request(3, 1, 2, 3).unwrap_err();
    assert!(error.to_string().contains("does not match request"));
    let error = parsed.validate_request(2, 9, 2, 3).unwrap_err();
    assert!(error.to_string().contains("does not match request"));
}

#[test]
fn entries_for_track_skips_out_of_range_positions() {
    let bytes = build_response(2, 1, 2, 3, &[(5, 0xaa, 0xbb), (5, 0xcc, 0xdd)]);
    let parsed = DbarFile::parse(&bytes).unwrap();
    assert_eq!(parsed.entries_for_track(0).count(), 0);
    assert_eq!(parsed.entries_for_track(3).count(), 0);
    assert_eq!(parsed.entries_for_track(1).count(), 1);
}
