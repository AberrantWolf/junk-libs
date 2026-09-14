use junk_libs_accuraterip::{
    ChecksumVersion, DbarFile, DbarResponse, DiscTrackSamples, ExpectedChecksum,
    VerificationOptions, VerificationStatus, verify_with_offsets,
};

const SKIP: usize = 5 * 588;

fn samples(seed: u32, len: usize) -> Vec<u32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        })
        .collect()
}

fn bounds(index: usize, count: usize, len: usize) -> (usize, usize) {
    let start = if index == 0 { SKIP - 1 } else { 0 };
    let end = if index + 1 == count { len - SKIP } else { len };
    (start, end)
}

fn crc_at_shift(
    disc: &[u32],
    track_start: usize,
    track_len: usize,
    track_index: usize,
    track_count: usize,
    shift: i32,
) -> (u32, u32) {
    let (local_start, local_end) = bounds(track_index, track_count, track_len);
    let source_start = (track_start as i64 + local_start as i64 + i64::from(shift)) as usize;
    disc[source_start..source_start + local_end - local_start]
        .iter()
        .enumerate()
        .fold((0u32, 0u32), |(v1, v2), (index, &sample)| {
            let multiplier = (local_start + index + 1) as u64;
            let product = multiplier * sample as u64;
            (
                v1.wrapping_add(product as u32),
                v2.wrapping_add(product as u32)
                    .wrapping_add((product >> 32) as u32),
            )
        })
}

fn frame_450_at_shift(disc: &[u32], track_start: usize, shift: i32) -> u32 {
    let start = (track_start as i64 + (450 * 588) as i64 + i64::from(shift)) as usize;
    disc[start..start + 588]
        .iter()
        .enumerate()
        .fold(0u32, |crc, (index, &sample)| {
            crc.wrapping_add((index as u32 + 1).wrapping_mul(sample))
        })
}

fn dbar(primary: &[u32], checksum_450: &[u32]) -> DbarFile {
    DbarFile {
        responses: vec![DbarResponse {
            track_count: primary.len() as u8,
            ar_id1: 1,
            ar_id2: 2,
            cddb_id: 3,
            tracks: primary
                .iter()
                .zip(checksum_450)
                .map(|(&checksum, &checksum_450)| ExpectedChecksum {
                    confidence: 10,
                    checksum,
                    checksum_450,
                })
                .collect(),
        }],
    }
}

fn assert_three_track_shift(shift: i32) {
    let tracks = vec![
        DiscTrackSamples {
            position: 1,
            samples: samples(1, 9_000),
        },
        DiscTrackSamples {
            position: 2,
            samples: samples(2, 9_000),
        },
        DiscTrackSamples {
            position: 3,
            samples: samples(3, 9_000),
        },
    ];
    let disc: Vec<u32> = tracks
        .iter()
        .flat_map(|track| track.samples.iter().copied())
        .collect();
    let starts = [0, 9_000, 18_000];
    let primary: Vec<u32> = starts
        .iter()
        .enumerate()
        .map(|(index, &start)| crc_at_shift(&disc, start, 9_000, index, 3, shift).0)
        .collect();
    let summary = verify_with_offsets(
        &dbar(&primary, &[0, 0, 0]),
        &tracks,
        VerificationOptions {
            max_sample_shift: 2939,
        },
    );
    assert_eq!(summary.status, VerificationStatus::Verified);
    assert_eq!(summary.chosen_sample_shift, Some(shift));
    assert!(summary.tracks.iter().all(|track| track.is_verified()));
}

#[test]
fn finds_negative_edge_shift_across_track_boundaries() {
    assert_three_track_shift(-2939);
}

#[test]
fn finds_small_negative_shift_across_track_boundaries() {
    assert_three_track_shift(-6);
}

#[test]
fn finds_zero_shift() {
    assert_three_track_shift(0);
}

#[test]
fn finds_small_positive_shift_across_track_boundaries() {
    assert_three_track_shift(6);
}

#[test]
fn finds_positive_edge_shift_across_track_boundaries() {
    assert_three_track_shift(2939);
}

#[test]
fn frame_450_accelerates_an_arv2_offset_but_does_not_replace_full_match() {
    let shift = 6;
    let tracks = vec![DiscTrackSamples {
        position: 1,
        samples: samples(7, 270_000),
    }];
    let disc = tracks[0].samples.clone();
    let (_, v2) = crc_at_shift(&disc, 0, disc.len(), 0, 1, shift);
    let partial = frame_450_at_shift(&disc, 0, shift);
    let summary = verify_with_offsets(
        &dbar(&[v2], &[partial]),
        &tracks,
        VerificationOptions {
            max_sample_shift: 20,
        },
    );
    assert_eq!(summary.chosen_sample_shift, Some(shift));
    assert_eq!(summary.status, VerificationStatus::Verified);
    assert!(summary.tracks[0].frame_450_support);
    assert_eq!(
        summary.tracks[0].best_match().unwrap().version,
        ChecksumVersion::V2
    );

    let only_partial = verify_with_offsets(
        &dbar(&[v2.wrapping_add(1)], &[partial]),
        &tracks,
        VerificationOptions {
            max_sample_shift: 20,
        },
    );
    assert_eq!(only_partial.chosen_sample_shift, Some(shift));
    assert_eq!(only_partial.status, VerificationStatus::IncompleteSearch);
    assert!(!only_partial.tracks[0].is_verified());
}

#[test]
fn silent_disc_reports_ambiguous_offsets_instead_of_guessing() {
    let tracks = vec![DiscTrackSamples {
        position: 1,
        samples: vec![0; 8_000],
    }];
    let summary = verify_with_offsets(
        &dbar(&[0], &[0]),
        &tracks,
        VerificationOptions {
            max_sample_shift: 6,
        },
    );
    assert_eq!(summary.status, VerificationStatus::AmbiguousOffsets);
    assert_eq!(summary.chosen_sample_shift, None);
    assert_eq!(summary.ambiguous_offsets.len(), 13);
}

#[test]
fn empty_pcm_or_database_is_no_data() {
    let summary = verify_with_offsets(&DbarFile::default(), &[], VerificationOptions::default());
    assert_eq!(summary.status, VerificationStatus::NoData);
}

#[test]
fn no_checksum_or_partial_evidence_does_not_invent_zero_shift() {
    let tracks = vec![DiscTrackSamples {
        position: 1,
        samples: samples(11, 9_000),
    }];
    let summary = verify_with_offsets(
        &dbar(&[0x1234_5678], &[0]),
        &tracks,
        VerificationOptions {
            max_sample_shift: 20,
        },
    );
    assert_eq!(summary.status, VerificationStatus::IncompleteSearch);
    assert_eq!(summary.chosen_sample_shift, None);
    assert_eq!(summary.tracks[0].sample_shift, None);
}
