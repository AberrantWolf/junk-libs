//! Offset-aware AccurateRip verification over a reconstructed disc stream.
//!
//! ARv1 and frame-450 candidates use cumulative sample and weighted-sample
//! sums, making the exhaustive shift scan O(PCM + tracks × shifts). ARv2 is
//! evaluated only for the small candidate frontier unlocked by a full ARv1
//! or frame-450 match because its per-product high-word fold is not a linear
//! weighted sum. This mirrors the role of frame-450 in CUETools: acceleration
//! and evidence, never proof.

use crate::crc::{TrackCrc, TrackPosition, skip_bounds};
use crate::dbar::DbarFile;
use crate::verify::{ChecksumVersion, CrcMatch, TrackVerification, TrackVerificationStatus};

pub const DEFAULT_MAX_SAMPLE_SHIFT: i32 = 2939;
const SAMPLES_PER_FRAME: usize = 588;
const FRAME_450_START: usize = 450 * SAMPLES_PER_FRAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationOptions {
    pub max_sample_shift: i32,
}

impl Default for VerificationOptions {
    fn default() -> Self {
        Self {
            max_sample_shift: DEFAULT_MAX_SAMPLE_SHIFT,
        }
    }
}

/// PCM for one physical audio track, in disc order. Each `u32` packs a
/// little-endian stereo sample as `L | (R << 16)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscTrackSamples {
    pub position: u8,
    pub samples: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetCandidate {
    pub sample_shift: i32,
    pub full_matches: usize,
    pub minimum_confidence: u8,
    pub total_confidence: u32,
    pub frame_450_matches: usize,
}

impl OffsetCandidate {
    fn rank(self) -> (usize, u8, u32, usize) {
        (
            self.full_matches,
            self.minimum_confidence,
            self.total_confidence,
            self.frame_450_matches,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStatus {
    Verified,
    Mismatched,
    NoData,
    AmbiguousOffsets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationSummary {
    pub status: VerificationStatus,
    pub chosen_sample_shift: Option<i32>,
    pub tracks: Vec<TrackVerification>,
    /// Every candidate tied for the winning evidence rank. Empty unless the
    /// result is ambiguous.
    pub ambiguous_offsets: Vec<OffsetCandidate>,
}

#[derive(Debug)]
struct PrefixSums {
    samples: Vec<u32>,
    weighted: Vec<u32>,
}

impl PrefixSums {
    fn new(samples: &[u32]) -> Self {
        let mut sums = Vec::with_capacity(samples.len() + 1);
        let mut weighted = Vec::with_capacity(samples.len() + 1);
        sums.push(0u32);
        weighted.push(0u32);
        for (index, &sample) in samples.iter().enumerate() {
            sums.push(sums[index].wrapping_add(sample));
            weighted.push(weighted[index].wrapping_add((index as u32).wrapping_mul(sample)));
        }
        Self {
            samples: sums,
            weighted,
        }
    }

    /// Weighted sum for source `[start, end)`, with the first source sample
    /// assigned multiplier `first_multiplier`.
    fn weighted_range(&self, start: usize, end: usize, first_multiplier: u32) -> u32 {
        let sample_sum = self.samples[end].wrapping_sub(self.samples[start]);
        let global_weighted = self.weighted[end].wrapping_sub(self.weighted[start]);
        let adjustment = first_multiplier.wrapping_sub(start as u32);
        global_weighted.wrapping_add(adjustment.wrapping_mul(sample_sum))
    }
}

#[derive(Debug)]
struct TrackWindow {
    position: u8,
    disc_position: TrackPosition,
    start: usize,
    len: usize,
}

/// Search source shifts inclusively. Positive shifts select later samples
/// from the reconstructed disc stream. Internal boundaries naturally borrow
/// from adjacent tracks; first/last disc exclusions keep the requested range
/// inside the available stream for the default ±2939 search.
pub fn verify_with_offsets(
    dbar: &DbarFile,
    tracks: &[DiscTrackSamples],
    options: VerificationOptions,
) -> VerificationSummary {
    if tracks.is_empty() || dbar.responses.is_empty() {
        return VerificationSummary {
            status: VerificationStatus::NoData,
            chosen_sample_shift: None,
            tracks: Vec::new(),
            ambiguous_offsets: Vec::new(),
        };
    }

    let max_shift = options.max_sample_shift.clamp(0, DEFAULT_MAX_SAMPLE_SHIFT);
    let mut disc = Vec::new();
    let mut windows = Vec::with_capacity(tracks.len());
    for (index, track) in tracks.iter().enumerate() {
        let start = disc.len();
        disc.extend_from_slice(&track.samples);
        windows.push(TrackWindow {
            position: track.position,
            disc_position: disc_position(index, tracks.len()),
            start,
            len: track.samples.len(),
        });
    }
    let prefix = PrefixSums::new(&disc);

    // The linear first pass discovers all ARv1 and partial-checksum evidence.
    let mut frontier = Vec::new();
    for shift in -max_shift..=max_shift {
        let mut has_evidence = shift == 0;
        for window in &windows {
            let v1 = shifted_v1(&prefix, window, shift);
            let frame_450 = shifted_frame_450(&prefix, window, shift);
            if primary_matches(dbar, window.position, v1)
                || frame_450.is_some_and(|crc| frame_450_matches(dbar, window.position, crc))
            {
                has_evidence = true;
                break;
            }
        }
        if has_evidence {
            frontier.push(shift);
        }
    }

    let mut evaluated = Vec::with_capacity(frontier.len());
    for shift in frontier {
        let (candidate, track_results) = evaluate_candidate(dbar, &disc, &prefix, &windows, shift);
        evaluated.push((candidate, track_results));
    }

    let Some(best_rank) = evaluated
        .iter()
        .map(|(candidate, _)| candidate.rank())
        .max()
    else {
        return VerificationSummary {
            status: VerificationStatus::Mismatched,
            chosen_sample_shift: None,
            tracks: Vec::new(),
            ambiguous_offsets: Vec::new(),
        };
    };
    if best_rank == (0, 0, 0, 0) {
        let mut tracks = evaluated
            .into_iter()
            .find(|(candidate, _)| candidate.sample_shift == 0)
            .map(|(_, tracks)| tracks)
            .unwrap_or_default();
        for track in &mut tracks {
            track.sample_shift = None;
        }
        return VerificationSummary {
            status: VerificationStatus::Mismatched,
            chosen_sample_shift: None,
            tracks,
            ambiguous_offsets: Vec::new(),
        };
    }
    let best_indices: Vec<usize> = evaluated
        .iter()
        .enumerate()
        .filter_map(|(index, (candidate, _))| (candidate.rank() == best_rank).then_some(index))
        .collect();

    if best_indices.len() != 1 {
        let ambiguous_offsets = best_indices
            .iter()
            .map(|&index| evaluated[index].0)
            .collect();
        let mut tracks = evaluate_candidate(dbar, &disc, &prefix, &windows, 0).1;
        for track in &mut tracks {
            track.sample_shift = None;
            track.status = TrackVerificationStatus::Ambiguous;
        }
        return VerificationSummary {
            status: VerificationStatus::AmbiguousOffsets,
            chosen_sample_shift: None,
            tracks,
            ambiguous_offsets,
        };
    }

    let (candidate, tracks) = evaluated.swap_remove(best_indices[0]);
    let status = if candidate.full_matches > 0 {
        VerificationStatus::Verified
    } else {
        VerificationStatus::Mismatched
    };
    VerificationSummary {
        status,
        chosen_sample_shift: Some(candidate.sample_shift),
        tracks,
        ambiguous_offsets: Vec::new(),
    }
}

fn evaluate_candidate(
    dbar: &DbarFile,
    disc: &[u32],
    prefix: &PrefixSums,
    windows: &[TrackWindow],
    shift: i32,
) -> (OffsetCandidate, Vec<TrackVerification>) {
    let mut results = Vec::with_capacity(windows.len());
    let mut confidences = Vec::new();
    let mut frame_matches = 0;
    for window in windows {
        let v1 = shifted_v1(prefix, window, shift);
        let v2 = shifted_v2(disc, window, shift);
        let frame_450 = shifted_frame_450(prefix, window, shift);
        let frame_support =
            frame_450.is_some_and(|crc| frame_450_matches(dbar, window.position, crc));
        if frame_support {
            frame_matches += 1;
        }
        let mut v1_matches = Vec::new();
        let mut v2_matches = Vec::new();
        for (pressing, expected) in dbar.entries_for_track(window.position) {
            let matches_v2 = expected.checksum == v2;
            let matches_v1 = expected.checksum == v1;
            let version = match (matches_v1, matches_v2) {
                (true, true) => Some(ChecksumVersion::Both),
                (true, false) => Some(ChecksumVersion::V1),
                (false, true) => Some(ChecksumVersion::V2),
                (false, false) => None,
            };
            let Some(version) = version else { continue };
            let matched = CrcMatch {
                pressing,
                confidence: expected.confidence,
                checksum: expected.checksum,
                version,
            };
            if matches_v2 {
                v2_matches.push(matched);
            }
            if matches_v1 {
                v1_matches.push(matched);
            }
        }
        let status = if v1_matches.is_empty() && v2_matches.is_empty() {
            TrackVerificationStatus::Mismatched
        } else {
            let confidence = v2_matches
                .iter()
                .chain(v1_matches.iter())
                .map(|matched| matched.confidence)
                .max()
                .unwrap_or(0);
            confidences.push(confidence);
            TrackVerificationStatus::Verified
        };
        results.push(TrackVerification {
            position: window.position,
            computed: TrackCrc { v1, v2 },
            v1_matches,
            v2_matches,
            sample_shift: Some(shift),
            frame_450_support: frame_support,
            status,
        });
    }

    let candidate = OffsetCandidate {
        sample_shift: shift,
        full_matches: confidences.len(),
        minimum_confidence: confidences.iter().copied().min().unwrap_or(0),
        total_confidence: confidences.iter().copied().map(u32::from).sum(),
        frame_450_matches: frame_matches,
    };
    (candidate, results)
}

fn shifted_v1(prefix: &PrefixSums, window: &TrackWindow, shift: i32) -> u32 {
    let (first, last) = skip_bounds(window.disc_position, window.len as u32);
    if first > last {
        return 0;
    }
    shifted_weighted(
        prefix,
        window,
        shift,
        first as usize - 1,
        last as usize,
        first,
    )
    .unwrap_or(0)
}

fn shifted_v2(disc: &[u32], window: &TrackWindow, shift: i32) -> u32 {
    let (first, last) = skip_bounds(window.disc_position, window.len as u32);
    if first > last {
        return 0;
    }
    let Some(start) = source_index(window.start + first as usize - 1, shift, disc.len()) else {
        return 0;
    };
    let count = (last - first + 1) as usize;
    if start + count > disc.len() {
        return 0;
    }
    disc[start..start + count]
        .iter()
        .enumerate()
        .fold(0u32, |crc, (index, &sample)| {
            let multiplier = first as u64 + index as u64;
            let product = multiplier * sample as u64;
            crc.wrapping_add(product as u32)
                .wrapping_add((product >> 32) as u32)
        })
}

fn shifted_frame_450(prefix: &PrefixSums, window: &TrackWindow, shift: i32) -> Option<u32> {
    let end = FRAME_450_START + SAMPLES_PER_FRAME;
    (end <= window.len)
        .then(|| shifted_weighted(prefix, window, shift, FRAME_450_START, end, 1).unwrap_or(0))
}

fn shifted_weighted(
    prefix: &PrefixSums,
    window: &TrackWindow,
    shift: i32,
    local_start: usize,
    local_end: usize,
    first_multiplier: u32,
) -> Option<u32> {
    let start = source_index(window.start + local_start, shift, prefix.samples.len() - 1)?;
    let count = local_end.checked_sub(local_start)?;
    let end = start.checked_add(count)?;
    (end < prefix.samples.len()).then(|| prefix.weighted_range(start, end, first_multiplier))
}

fn source_index(base: usize, shift: i32, disc_len: usize) -> Option<usize> {
    let shifted = base as i64 + i64::from(shift);
    (shifted >= 0 && shifted <= disc_len as i64).then_some(shifted as usize)
}

fn primary_matches(dbar: &DbarFile, position: u8, checksum: u32) -> bool {
    dbar.entries_for_track(position)
        .any(|(_, expected)| expected.checksum == checksum)
}

fn frame_450_matches(dbar: &DbarFile, position: u8, checksum: u32) -> bool {
    dbar.entries_for_track(position)
        .any(|(_, expected)| expected.checksum_450 != 0 && expected.checksum_450 == checksum)
}

fn disc_position(index: usize, len: usize) -> TrackPosition {
    match (index, len) {
        (_, 1) => TrackPosition::Only,
        (0, _) => TrackPosition::First,
        (index, len) if index + 1 == len => TrackPosition::Last,
        _ => TrackPosition::Middle,
    }
}
