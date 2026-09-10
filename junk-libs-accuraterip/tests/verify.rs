//! Pure verification-logic tests for the unlabeled dBAR primary checksum.

use junk_libs_accuraterip::{
    ChecksumVersion, DbarFile, DbarResponse, ExpectedChecksum, TrackCrc, verify_disc, verify_track,
};

fn dbar_with_responses(responses: Vec<DbarResponse>) -> DbarFile {
    DbarFile { responses }
}

fn entry(confidence: u8, checksum: u32, checksum_450: u32) -> ExpectedChecksum {
    ExpectedChecksum {
        confidence,
        checksum,
        checksum_450,
    }
}

fn response(entries: &[(u8, u32, u32)]) -> DbarResponse {
    DbarResponse {
        track_count: entries.len() as u8,
        ar_id1: 0,
        ar_id2: 0,
        cddb_id: 0,
        tracks: entries
            .iter()
            .map(|&(confidence, checksum, checksum_450)| entry(confidence, checksum, checksum_450))
            .collect(),
    }
}

#[test]
fn primary_checksum_can_be_arv2() {
    let dbar = dbar_with_responses(vec![
        response(&[(5, 0x1111, 0xaaaa), (5, 0x3333, 0xbbbb)]),
        response(&[(9, 0xaaaa, 0xcccc), (9, 0xdddd, 0xeeee)]),
    ]);
    let got = verify_track(
        &dbar,
        2,
        TrackCrc {
            v1: 0xcccc,
            v2: 0xdddd,
        },
    );
    assert!(got.is_verified());
    assert!(got.v1_matches.is_empty());
    assert_eq!(got.v2_matches.len(), 1);
    assert_eq!(got.v2_matches[0].pressing, 1);
    assert_eq!(got.v2_matches[0].version, ChecksumVersion::V2);
    assert_eq!(got.status_string(), "v2 confidence 9");
}

#[test]
fn primary_checksum_can_be_arv1() {
    let dbar = dbar_with_responses(vec![response(&[(3, 0xdead_beef, 0xcafe_f00d)])]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0xdead_beef,
            v2: 0x0000_ffff,
        },
    );
    assert!(got.is_verified());
    assert_eq!(got.v1_matches.len(), 1);
    assert!(got.v2_matches.is_empty());
    assert_eq!(got.status_string(), "v1 confidence 3 (v2 no match)");
}

#[test]
fn frame_450_equality_alone_never_verifies() {
    let dbar = dbar_with_responses(vec![response(&[(7, 0x1111_1111, 0x2222_2222)])]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0x2222_2222,
            v2: 0,
        },
    );
    assert!(!got.is_verified());
    assert_eq!(got.status_string(), "no match");
}

#[test]
fn a_zero_primary_is_a_real_checksum() {
    let dbar = dbar_with_responses(vec![response(&[(4, 0, 0xdead_beef)])]);
    let got = verify_track(&dbar, 1, TrackCrc { v1: 0, v2: 42 });
    assert!(got.is_verified());
    assert_eq!(got.v1_matches[0].version, ChecksumVersion::V1);
}

#[test]
fn equal_local_versions_are_reported_as_both() {
    let dbar = dbar_with_responses(vec![response(&[(4, 0xabcd, 0)])]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0xabcd,
            v2: 0xabcd,
        },
    );
    assert_eq!(got.v1_matches[0].version, ChecksumVersion::Both);
    assert_eq!(got.v2_matches[0].version, ChecksumVersion::Both);
}

#[test]
fn best_confidence_picks_highest_across_versions() {
    let dbar = dbar_with_responses(vec![
        response(&[(2, 0xaaaa, 0)]),
        response(&[(8, 0xcccc, 0)]),
        response(&[(5, 0xbbbb, 0)]),
    ]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0xaaaa,
            v2: 0xbbbb,
        },
    );
    assert_eq!(got.v1_matches.len(), 1);
    assert_eq!(got.v2_matches.len(), 1);
    assert_eq!(got.best_confidence(), Some(5));
}

#[test]
fn verify_disc_handles_multi_track_in_one_pass() {
    let dbar = dbar_with_responses(vec![response(&[
        (3, 0x1111, 0),
        (3, 0x4444, 0),
        (3, 0x5555, 0),
    ])]);
    let got = verify_disc(
        &dbar,
        &[
            (
                1,
                TrackCrc {
                    v1: 0x1111,
                    v2: 0x2222,
                },
            ),
            (
                2,
                TrackCrc {
                    v1: 0x3333,
                    v2: 0x4444,
                },
            ),
            (3, TrackCrc { v1: 0x5555, v2: 0 }),
        ],
    );
    assert_eq!(got.len(), 3);
    assert!(got[0].is_verified() && got[0].v2_matches.is_empty());
    assert!(got[1].is_verified() && got[1].v1_matches.is_empty());
    assert!(got[2].is_verified() && got[2].v2_matches.is_empty());
}
