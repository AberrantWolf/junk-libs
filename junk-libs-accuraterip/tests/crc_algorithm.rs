//! Algorithmic tests for AccurateRip CRC v1 and v2.
//!
//! Pure-in-memory tests against hand-computed expected values so they run
//! offline. The live ARver cross-check lives in `arver_live.rs` behind
//! `#[ignore]`.

use junk_libs_accuraterip::{
    PcmSector, SAMPLES_PER_SECTOR, SKIP_SAMPLES, TrackCrc, TrackPosition, skip_bounds,
    track_crc_samples, track_crc_streaming,
};
use junk_libs_core::AnalysisError;

/// Build `n_sectors` of all-zero PCM sectors wrapped as `Result`s.
fn zero_sectors(n_sectors: u32) -> Vec<Result<PcmSector, AnalysisError>> {
    (0..n_sectors)
        .map(|_| Ok([0u32; SAMPLES_PER_SECTOR]))
        .collect()
}

/// Build one sector that contains exactly one non-zero sample at
/// `index_within_track` (0-indexed across the whole track). The sample
/// value lives in sector `index_within_track / 588` at entry
/// `index_within_track % 588`. Returns `(sectors, total_samples)`.
fn sectors_with_one_sample(
    n_sectors: u32,
    index_within_track: u32,
    sample: u32,
) -> Vec<Result<PcmSector, AnalysisError>> {
    let target_sector = (index_within_track / SAMPLES_PER_SECTOR as u32) as usize;
    let target_slot = (index_within_track % SAMPLES_PER_SECTOR as u32) as usize;
    (0..n_sectors as usize)
        .map(|s| {
            let mut sector = [0u32; SAMPLES_PER_SECTOR];
            if s == target_sector {
                sector[target_slot] = sample;
            }
            Ok(sector)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// skip_bounds
// ---------------------------------------------------------------------------

#[test]
fn skip_bounds_middle_track_covers_full_range() {
    assert_eq!(skip_bounds(TrackPosition::Middle, 10_000), (1, 10_000));
}

#[test]
fn skip_bounds_first_track_includes_position_2940() {
    // ARver's C reference uses `multiplier >= skip_frames`, so position
    // 2940 is the *first included* position, not excluded.
    assert_eq!(
        skip_bounds(TrackPosition::First, 10_000),
        (SKIP_SAMPLES, 10_000)
    );
    assert_eq!(SKIP_SAMPLES, 2940);
}

#[test]
fn skip_bounds_last_track_includes_position_total_minus_2940() {
    assert_eq!(
        skip_bounds(TrackPosition::Last, 10_000),
        (1, 10_000 - SKIP_SAMPLES)
    );
}

#[test]
fn skip_bounds_only_track_applies_both_skips() {
    assert_eq!(
        skip_bounds(TrackPosition::Only, 10_000),
        (SKIP_SAMPLES, 10_000 - SKIP_SAMPLES)
    );
}

#[test]
fn skip_bounds_short_only_track_yields_empty_window() {
    // When total < 2 * SKIP_SAMPLES the window becomes empty (end < start)
    // and downstream accumulation produces 0 rather than an error.
    let (start, end) = skip_bounds(TrackPosition::Only, 2 * SKIP_SAMPLES - 10);
    assert!(end < start);
}

// ---------------------------------------------------------------------------
// track_crc_streaming
// ---------------------------------------------------------------------------

#[test]
fn zero_stream_produces_zero_crc() {
    // 10 sectors of silence through a middle track.
    let sectors = zero_sectors(10);
    let total = 10 * SAMPLES_PER_SECTOR as u32;
    let crc = track_crc_streaming(sectors, total, TrackPosition::Middle).unwrap();
    assert_eq!(crc, TrackCrc { v1: 0, v2: 0 });
}

#[test]
fn v1_base_formula_matches_position_times_sample() {
    // Put a single sample of value 3 at position 5 (0-indexed index 4).
    // Expected v1 accumulator for a middle track = 5 * 3 = 15.
    let sectors = sectors_with_one_sample(10, 4, 3);
    let total = 10 * SAMPLES_PER_SECTOR as u32;
    let crc = track_crc_streaming(sectors, total, TrackPosition::Middle).unwrap();
    assert_eq!(crc.v1, 15);
    // Small product fits in 32 bits -> v2 hi = 0, so v2 == v1 here.
    assert_eq!(crc.v2, 15);
}

#[test]
fn v2_folds_64_bit_product_hi_and_lo() {
    // Choose position * sample > u32::MAX so hi is non-zero.
    // One sample at index 0 (position 1) of value 0xFFFF_FFFF. The v1
    // wrapping product = 0xFFFF_FFFF (same as the sample). v2's 64-bit
    // product = 0x0000_0000_FFFF_FFFF -> hi = 0, lo = 0xFFFF_FFFF.
    // Use position 2 instead: product = 2 * 0xFFFF_FFFF = 0x1_FFFF_FFFE.
    //   hi = 1, lo = 0xFFFF_FFFE. v2 = 1 + 0xFFFF_FFFE = 0xFFFF_FFFF.
    //   v1 = (2 * 0xFFFF_FFFF) truncated to 32 bits = 0xFFFF_FFFE.
    let sectors = sectors_with_one_sample(10, 1, 0xFFFF_FFFF);
    let total = 10 * SAMPLES_PER_SECTOR as u32;
    let crc = track_crc_streaming(sectors, total, TrackPosition::Middle).unwrap();
    assert_eq!(crc.v1, 0xFFFF_FFFE);
    assert_eq!(crc.v2, 0xFFFF_FFFF);
}

#[test]
fn materialized_and_streaming_paths_share_crc_math() {
    let sectors = sectors_with_one_sample(10, 1, 0xFFFF_FFFF);
    let samples: Vec<u32> = sectors
        .iter()
        .flat_map(|sector| sector.as_ref().unwrap().iter().copied())
        .collect();
    let streamed =
        track_crc_streaming(sectors, samples.len() as u32, TrackPosition::Middle).unwrap();

    assert_eq!(track_crc_samples(&samples, TrackPosition::Middle), streamed);
}

#[test]
fn first_track_skip_boundary_is_inclusive_at_2940() {
    let total = 10 * SAMPLES_PER_SECTOR as u32;

    // Sample at position 2939 (index 2938) is the last EXCLUDED position.
    let sectors_pre = sectors_with_one_sample(10, SKIP_SAMPLES - 2, 0xDEAD_BEEF);
    let pre = track_crc_streaming(sectors_pre, total, TrackPosition::First).unwrap();
    assert_eq!(
        pre,
        TrackCrc { v1: 0, v2: 0 },
        "position 2939 must be skipped for First"
    );

    // Sample at position 2940 (index 2939) is the first INCLUDED position.
    let sectors_boundary = sectors_with_one_sample(10, SKIP_SAMPLES - 1, 0xDEAD_BEEF);
    let boundary = track_crc_streaming(sectors_boundary, total, TrackPosition::First).unwrap();
    let expected_v1 = SKIP_SAMPLES.wrapping_mul(0xDEAD_BEEF);
    assert_eq!(boundary.v1, expected_v1);
}

#[test]
fn first_track_skips_position_1() {
    // Sample at position 1 (index 0) contributes nothing for First.
    let sectors = sectors_with_one_sample(10, 0, 0xDEAD_BEEF);
    let total = 10 * SAMPLES_PER_SECTOR as u32;
    let got = track_crc_streaming(sectors, total, TrackPosition::First).unwrap();
    assert_eq!(got, TrackCrc { v1: 0, v2: 0 });
}

#[test]
fn last_track_skips_trailing_2940_positions() {
    // Put a sample at the last position -> must be skipped for Last.
    let total_samples = 10 * SAMPLES_PER_SECTOR as u32;
    let last_index = total_samples - 1;
    let sectors_last = sectors_with_one_sample(10, last_index, 0xABCD_1234);
    let last = track_crc_streaming(sectors_last, total_samples, TrackPosition::Last).unwrap();
    assert_eq!(last, TrackCrc { v1: 0, v2: 0 });

    // Put a sample at `total - 2940` (position total - 2939, the last
    // included position) -> contributes.
    let included_index = total_samples - SKIP_SAMPLES - 1;
    let sectors_in = sectors_with_one_sample(10, included_index, 0xABCD_1234);
    let included = track_crc_streaming(sectors_in, total_samples, TrackPosition::Last).unwrap();
    let position = included_index + 1;
    let expected_v1 = position.wrapping_mul(0xABCD_1234);
    assert_eq!(included.v1, expected_v1);
}

#[test]
fn only_track_applies_both_skips_simultaneously() {
    // Need strictly more than 2 * SKIP_SAMPLES samples for a valid Only
    // track window; 15 sectors = 8820 samples clears the bound.
    const SECTORS: u32 = 15;
    let total_samples = SECTORS * SAMPLES_PER_SECTOR as u32;

    // Position 1 skipped.
    let leading = sectors_with_one_sample(SECTORS, 0, 0x1111_1111);
    assert_eq!(
        track_crc_streaming(leading, total_samples, TrackPosition::Only).unwrap(),
        TrackCrc { v1: 0, v2: 0 }
    );

    // Position total_samples skipped (tail).
    let trailing = sectors_with_one_sample(SECTORS, total_samples - 1, 0x1111_1111);
    assert_eq!(
        track_crc_streaming(trailing, total_samples, TrackPosition::Only).unwrap(),
        TrackCrc { v1: 0, v2: 0 }
    );

    // A sample in the middle still contributes.
    let middle_idx = total_samples / 2;
    let middle = sectors_with_one_sample(SECTORS, middle_idx, 0x1111_1111);
    let crc = track_crc_streaming(middle, total_samples, TrackPosition::Only).unwrap();
    let expected_v1 = (middle_idx + 1).wrapping_mul(0x1111_1111);
    assert_eq!(crc.v1, expected_v1);
}

#[test]
fn mismatched_sample_count_errors() {
    // 5 sectors emitted but total claimed as 10.
    let sectors = zero_sectors(5);
    let err = track_crc_streaming(sectors, 10 * 588, TrackPosition::Middle).unwrap_err();
    assert!(format!("{}", err).contains("iterator produced"));
}

#[test]
fn multi_sample_linear_accumulation_middle_track() {
    // Fill 2 sectors with samples where sector 0 slot i = i+1 (1..=588)
    // and sector 1 slot i = 0. For a middle track, v1 = sum_{k=1..=588} k^2.
    let mut s0 = [0u32; SAMPLES_PER_SECTOR];
    for (i, slot) in s0.iter_mut().enumerate() {
        *slot = (i + 1) as u32;
    }
    let s1 = [0u32; SAMPLES_PER_SECTOR];
    let sectors = vec![Ok(s0), Ok(s1)];
    let total = 2 * SAMPLES_PER_SECTOR as u32;
    let crc = track_crc_streaming(sectors, total, TrackPosition::Middle).unwrap();

    let expected: u32 = (1u32..=588).fold(0u32, |acc, k| acc.wrapping_add(k.wrapping_mul(k)));
    assert_eq!(crc.v1, expected);
}
