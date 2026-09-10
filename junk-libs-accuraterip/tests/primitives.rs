use junk_libs_accuraterip::{
    DbarFile, DbarResponse, DiscTrackSamples, ExpectedChecksum, TrackPosition, VerificationOptions,
    VerificationStatus, track_crc_samples, verify_with_offsets,
};

#[test]
fn crc_versions_preserve_the_reference_overflow_difference() {
    let crc = track_crc_samples(&[0, u32::MAX], TrackPosition::Middle);
    assert_eq!(crc.v1, 0xffff_fffe);
    assert_eq!(crc.v2, 0xffff_ffff);
}

#[test]
fn dbar_parser_rejects_a_truncated_track_entry() {
    let bytes = [1, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 7, 4];
    assert!(DbarFile::parse(&bytes).is_err());
}

#[test]
fn offset_search_borrows_samples_across_track_boundaries() {
    let tracks = vec![
        DiscTrackSamples {
            position: 1,
            samples: pseudo_random(1, 9_000),
        },
        DiscTrackSamples {
            position: 2,
            samples: pseudo_random(2, 9_000),
        },
        DiscTrackSamples {
            position: 3,
            samples: pseudo_random(3, 9_000),
        },
    ];
    let disc: Vec<u32> = tracks
        .iter()
        .flat_map(|track| track.samples.iter().copied())
        .collect();
    let shift = 6_i32;
    let checksums = [
        shifted_v1(&disc, 0, 9_000, 2_939, 9_000, shift),
        shifted_v1(&disc, 9_000, 9_000, 0, 9_000, shift),
        shifted_v1(&disc, 18_000, 9_000, 0, 6_060, shift),
    ];
    let dbar = DbarFile {
        responses: vec![DbarResponse {
            track_count: 3,
            ar_id1: 1,
            ar_id2: 2,
            cddb_id: 3,
            tracks: checksums
                .into_iter()
                .map(|checksum| ExpectedChecksum {
                    confidence: 10,
                    checksum,
                    checksum_450: 0,
                })
                .collect(),
        }],
    };
    let result = verify_with_offsets(
        &dbar,
        &tracks,
        VerificationOptions {
            max_sample_shift: 20,
        },
    );
    assert_eq!(result.status, VerificationStatus::Verified);
    assert_eq!(result.chosen_sample_shift, Some(shift));
}

fn pseudo_random(seed: u32, len: usize) -> Vec<u32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        })
        .collect()
}

fn shifted_v1(
    disc: &[u32],
    track_start: usize,
    _track_len: usize,
    local_start: usize,
    local_end: usize,
    shift: i32,
) -> u32 {
    let source_start = (track_start as i64 + local_start as i64 + i64::from(shift)) as usize;
    disc[source_start..source_start + local_end - local_start]
        .iter()
        .enumerate()
        .fold(0_u32, |crc, (index, &sample)| {
            crc.wrapping_add(((local_start + index + 1) as u32).wrapping_mul(sample))
        })
}
